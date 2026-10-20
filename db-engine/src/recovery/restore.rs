// Phase 4.1: Analysis pass — 从日志构建 TxnTable（赢家/输家 + 每事务 last_lsn）

use std::collections::HashMap;
use crate::recovery::log_manager::LogManager;
use crate::recovery::log_record::{LogRecord, TxnId};
use crate::recovery::lsn::{Lsn};

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

            // TODO 你来写 4 个分支：Begin / Commit / Abort / Update
            //   Begin { txn_id }        → table.insert(*txn_id, TxnStats { is_winner: false, last_lsn: *lsn })
            //   Commit { txn_id }       → 拿到 entry，把 is_winner 改成 true，last_lsn 更新为 *lsn
            //   Abort { txn_id }        → 更新 last_lsn（赢家保持 false）
            //   Update { txn_id, .. }   → 只把 last_lsn 推进（不改胜负；用 if let Some 保护，因为可能这条记录属于 Begin 之前？其实不会）
            _ => {} // Checkpoint / CLR 等先忽略
        }
    }
    table
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

    pub fn apply_update_to_page(
        page: &mut Page,
        rec_lsn: Lsn,
        _page_id: PageId,   // 保留形参方便上层日志诊断；这里没用到
        offset: u16,
        after: &[u8],
    ) {
        // TODO 你来写 3 步：
        //   ① 若 PageHeader::read_page_lsn(page) >= rec_lsn，直接 return（幂等跳过）
        if PageHeader::read_page_lsn(page) >= rec_lsn { return; }

        page.write_data(offset as usize, after);

        PageHeader::write_page_lsn(page, rec_lsn);
    }

    fn test_apply_update_writes_after_image() {
        use crate::recovery::lsn::INVALID_LSN;
        let mut page = Page::new(42);
        // 初始：offset 100 处是几个 0xAA 字节，page_lsn 未点亮
        page.write_data(100, &[0xAA, 0xAA, 0xAA]);
        PageHeader::write_page_lsn(&mut page, INVALID_LSN);

        // 一条 Update：把这 3 字节改成 0x11 0x22 0x33，rec_lsn=100
        apply_update_to_page(&mut page, 100, 42, 100, &[0x11, 0x22, 0x33]);

        assert_eq!(&page.read_data(100, 3), &[0x11, 0x22, 0x33]);
        assert_eq!(PageHeader::read_page_lsn(&page), 100);
    }

    #[test]
    fn test_apply_update_is_idempotent() {
        // 已吸收过该 lsn（page_lsn >= rec_lsn）→ 不该重复写
        let mut page = Page::new(42);
        PageHeader::write_page_lsn(&mut page, 200);   // 页已经追到 200
        let snapshot = page.read_data(100, 3).to_vec();

        apply_update_to_page(&mut page, 100, 42, 100, &[0x99, 0x99, 0x99]);   // 试图重放 lsn=100

        // 应该跳过：数据不变、page_lsn 也不变
        assert_eq!(&page.read_data(100, 3), &snapshot[..]);

        assert_eq!(PageHeader::read_page_lsn(&page), 200);
    }


}