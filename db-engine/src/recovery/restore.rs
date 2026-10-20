// Phase 4.1: Analysis pass — 从日志构建 TxnTable（赢家/输家 + 每事务 last_lsn）

use std::collections::HashMap;
use crate::recovery::log_manager::LogManager;
use crate::recovery::log_record::{LogRecord, TxnId};
use crate::recovery::lsn::{Lsn, INVALID_LSN};

use std::sync::Arc;
use crate::storage::buffer_pool::BufferPool;
use crate::storage::page::Page;
use crate::storage::page_header::PageHeader;
use crate::types::PageId;
/// 每事务的分析结果：赢家 or 输家 + 它自己的最后一条 LSN
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TxnStats {
    pub is_winner: bool,
    pub last_lsn: Lsn,
}

/// Analysis 的输出：一张事务表
pub type TxnTable = HashMap<TxnId, TxnStats>;
pub fn restore(
    log_path: &str,
    buffer_pool: &Arc<BufferPool>,
    log: &Arc<LogManager>,
) -> std::io::Result<()> {
    // ① Analysis：读日志 → 建 TxnTable
    let records = LogManager::read_all(log_path)?;
    let txn_table = analyze(&records);

    // ② Redo：把所有 Update.after 无差别重放到页上
    redo(&records, buffer_pool)?;

    // ③ Undo：沿输家链回滚，写 CLR + 把 before 写回页
    undo(&txn_table, &records, buffer_pool, log)?;

    Ok(())
}
/// 纯函数：把日志记录序列 → 事务表（方便单测直接喂 Vec，不碰文件）
pub fn analyze(records: &[(Lsn, LogRecord)]) -> TxnTable {
    let mut table: TxnTable = HashMap::new();
    for (lsn, rec) in records {
        match rec {
            LogRecord::Begin { txn_id } => {
                table.insert(*txn_id, TxnStats { is_winner: false, last_lsn: *lsn });
            },
            LogRecord::Commit { txn_id }=>{
                table.get_mut(txn_id).map(|stats| { stats.is_winner = true; stats.last_lsn = *lsn; });
            }
            LogRecord::Abort { txn_id }=>{
                table.get_mut(txn_id).map(|stats| { stats.last_lsn = *lsn; });

            }
            LogRecord::Update { txn_id, .. }=>{
                table.get_mut(txn_id).map(|stats| { stats.last_lsn = *lsn; });
            }


            _ => {} // Checkpoint / CLR 等先忽略
        }
    }
    table
}

/// 把一条 Update 的 before_image 写回页（撤销该次修改）。
/// 不做幂等检查（幂等由上层通过"是否已存在对应 CLR"来判断）。
pub fn apply_undo_to_page(
    page: &mut Page,
    rec_lsn: Lsn,
    _page_id: PageId,
    offset: u16,
    before: &[u8],
) {

    page.write_data(offset as usize, before);
    //   ② PageHeader::write_page_lsn(page, rec_lsn);
    PageHeader::write_page_lsn(page,rec_lsn);
}
pub fn apply_update_to_page(
    page: &mut Page,
    rec_lsn: Lsn,
    _page_id: PageId,   // 保留形参方便上层日志诊断；这里没用到
    offset: u16,
    after: &[u8],
) {

    if PageHeader::read_page_lsn(page) >= rec_lsn { return; }

    page.write_data(offset as usize, after);

    PageHeader::write_page_lsn(page, rec_lsn);
}
/// Redo pass：遍历日志中所有 Update 记录，通过 buffer_pool 拿到对应页，
/// 把 after_image 重放上去（幂等由 apply_update_to_page 内部保证）。
pub fn redo(records: &[(Lsn, LogRecord)], buffer_pool: &Arc<BufferPool>) -> std::io::Result<()> {
    for (lsn, rec) in records {

        if let LogRecord::Update { page_id, offset, after, .. } = rec {
            let page_ref = buffer_pool.fetch_page(*page_id)?;
            let mut page = page_ref.write().unwrap();
            apply_update_to_page(&mut page, *lsn, *page_id, *offset, after);
        }

    }
    Ok(())
}
/// Undo pass：遍历 TxnTable 中所有输家，沿 prev_lsn 链回滚其 Update。
/// 每撤一条：先写 CLR + flush（WAL 规则），再把 before 写回页。
pub fn undo(
    txn_table: &TxnTable,
    records: &[(Lsn, LogRecord)],
    buffer_pool: &Arc<BufferPool>,
    log: &Arc<LogManager>,
) -> std::io::Result<()> {
    // ① 建索引：lsn -> &LogRecord（O(1) 跳转）
    let by_lsn: HashMap<Lsn, &LogRecord> = records.iter().map(|(l, r)| (*l, r)).collect();

    // ② 对每个输家事务
    for (txn_id, stats) in txn_table {
        if stats.is_winner { continue; }

        let mut next_undo: Lsn = stats.last_lsn;

        // ③ 沿链往回走
        while next_undo != INVALID_LSN {
            let rec = match by_lsn.get(&next_undo) {
                Some(r) => *r,
                None => break,
            };
            match rec {
                LogRecord::Update { page_id, offset, before, prev_lsn, txn_id: rec_txn, .. } => {
                    if rec_txn != txn_id { break; }
                    let clr = LogRecord::Compensate {
                        txn_id: *txn_id,
                        prev_lsn: next_undo,
                        page_id: *page_id,
                        offset: *offset,
                        before: before.clone(),
                        undo_next_lsn: *prev_lsn,
                    };
                    let clr_lsn = log.append(&clr);
                    log.flush_to(clr_lsn).expect("flush CLR failed");
                    let page_ref = buffer_pool.fetch_page(*page_id)?;
                    let mut page = page_ref.write().unwrap();
                    apply_undo_to_page(&mut page, next_undo, *page_id, *offset, before);
                    next_undo = *prev_lsn;
                },
                LogRecord::Begin { .. } => break,
                _ => break,
            }


        }
    }
    Ok(())
}

/// 端到端入口：从日志文件路径读出记录 → 交给 analyze
pub fn analysis_from_log(path: &str) -> std::io::Result<TxnTable> {
    let records = LogManager::read_all(path)?;
    Ok(analyze(&records))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recovery::log_record::LogRecord;

    /// 纯单测：只喂记录，不碰磁盘
    #[test]
    fn test_analysis_winners_and_losers() {
        let recs = vec![
            (10, LogRecord::Begin { txn_id: 1 }),
            (11, LogRecord::Commit { txn_id: 1 }),   // txn1 赢家
            (12, LogRecord::Begin { txn_id: 2 }),    // txn2 只有 Begin → 输家
            (13, LogRecord::Begin { txn_id: 3 }),
            (14, LogRecord::Abort { txn_id: 3 }),    // txn3 显式 abort → 输家
        ];
        let t = analyze(&recs);
        assert_eq!(t.get(&1), Some(&TxnStats { is_winner: true,  last_lsn: 11 }));
        assert_eq!(t.get(&2), Some(&TxnStats { is_winner: false, last_lsn: 12 }));
        assert_eq!(t.get(&3), Some(&TxnStats { is_winner: false, last_lsn: 14 }));
    }

    #[test]
    fn test_apply_undo_writes_before_image() {
        let mut page = Page::new(42);
        // 模拟 redo 之后的状态：页上已是 after = [0x11, 0x22, 0x33]
        page.write_data(100, &[0x11, 0x22, 0x33]);
        PageHeader::write_page_lsn(&mut page, 50);

        // undo：把 before = [0xAA, 0xAA, 0xAA] 写回
        apply_undo_to_page(&mut page, 50, 42, 100, &[0xAA, 0xAA, 0xAA]);

        assert_eq!(page.read_data(100, 3), &[0xAA, 0xAA, 0xAA]);
        assert_eq!(PageHeader::read_page_lsn(&mut page), 50);
    }
    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::recovery::log_record::LogRecord;

        #[test]
        fn test_analysis_winners_and_losers() {  }        // ← 只留这一份



        #[test]
        fn test_apply_update_writes_after_image() {
            use crate::recovery::lsn::INVALID_LSN;
            let mut page = Page::new(42);
            page.write_data(100, &[0xAA, 0xAA, 0xAA]);
            PageHeader::write_page_lsn(&mut page, INVALID_LSN);
            apply_update_to_page(&mut page, 100, 42, 100, &[0x11, 0x22, 0x33]);
            assert_eq!(page.read_data(100, 3), &[0x11, 0x22, 0x33]);
            assert_eq!(PageHeader::read_page_lsn(&page), 100);
        }

        #[test]
        fn test_apply_update_is_idempotent() {
            let mut page = Page::new(42);
            PageHeader::write_page_lsn(&mut page, 200);
            let snapshot = page.read_data(100, 3).to_vec();
            apply_update_to_page(&mut page, 100, 42, 100, &[0x99, 0x99, 0x99]);
            assert_eq!(page.read_data(100, 3), &snapshot[..]);
            assert_eq!(PageHeader::read_page_lsn(&page), 200);
        }
        #[test]
        fn test_end_to_end_crash_recovery() {
            use std::fs::{self, OpenOptions};
            use crate::storage::disk_manager::DiskManager;
            use crate::config::PAGE_SIZE;
            use crate::recovery::log_manager::LogManager;
            use crate::recovery::log_record::LogRecord;
            use crate::recovery::lsn::INVALID_LSN;

            let db_path = "./data/test_restore.db";
            let log_path = "./data/test_restore.log";
            let _ = fs::remove_file(db_path);
            let _ = fs::remove_file(log_path);
            fs::create_dir_all("./data").unwrap();

            // ===== 步骤 1：写入初始页到磁盘 =====
            {
                let file = OpenOptions::new().read(true).write(true).create(true)
                    .open(db_path).unwrap();
                let mut dm = DiskManager::from_file(file);
                let mut page = Page::new(5);
                page.write_data(100, &[0xAA; 10]);
                dm.write_page(5, &page.data).unwrap();
            }

            // ===== 步骤 2-3：写日志（赢家 + 输家）=====
            {
                let lm = LogManager::from_path(log_path).unwrap();
                // 赢家 txn=1
                let l1 = lm.append(&LogRecord::Begin { txn_id: 1 });
                let l2 = lm.append(&LogRecord::Update {
                    txn_id: 1, prev_lsn: l1, page_id: 5, offset: 100,
                    before: vec![0xAA; 10], after: vec![0x11; 10],
                });
                let l3 = lm.append(&LogRecord::Commit { txn_id: 1 });
                lm.flush_to(l3).unwrap();  // commit 必须刷

                // 输家 txn=2（只有 Begin + Update，没有 Commit）
                let l4 = lm.append(&LogRecord::Begin { txn_id: 2 });
                let _l5 = lm.append(&LogRecord::Update {
                    txn_id: 2, prev_lsn: l4, page_id: 5, offset: 110,
                    before: vec![0x00; 10], after: vec![0x22; 10],
                });
                lm.flush_to(l4).unwrap();  // 模拟"日志部分落盘"（只刷到 Begin）
            }

            // ===== 步骤 4：崩溃 = 什么都不做（上面 block 结束，所有对象 drop）=====
            // page 5 在磁盘上仍然是步骤 1 的状态（offset 100 = [0xAA;10]，其余零）

            // ===== 步骤 5：重开 =====
            let file = OpenOptions::new().read(true).write(true).create(true)
                .open(db_path).unwrap();
            let dm = DiskManager::from_file(file);
            let lm = LogManager::from_path(log_path).unwrap();
            let lm = std::sync::Arc::new(lm);
            let bp = std::sync::Arc::new(BufferPool::new(dm, std::sync::Arc::clone(&lm)));

            // ===== 步骤 6：restore =====
            restore(log_path, &bp, &lm).unwrap();

            // ===== 步骤 7：验证 =====
            let page_ref = bp.fetch_page(5).unwrap();
            let page = page_ref.read().unwrap();

            assert_eq!(page.read_data(100, 10), &[0x11; 10][..],
                       "赢家数据应存活");

            // 输家：redo 写了 [0x22;10]，但 undo 把 before [0x00;10] 盖回去了
            assert_eq!(page.read_data(110, 10), &[0x00; 10][..],
                       "输家数据应被撤销");

            // 清理
            drop(page);
            drop(page_ref);
            let _ = fs::remove_file(db_path);
            let _ = fs::remove_file(log_path);
        }
    }
}