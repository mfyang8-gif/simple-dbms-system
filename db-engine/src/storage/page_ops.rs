// src/storage/page_ops.rs
//
// 【模块职责】
// Slotted Page 的页级 CRUD：在单个数据页内插入、读取、删除、扫描记录。
//
// 【WAL 集成（Slice B）】
// 凡是对页内字节的修改，都必须"先写一条等长物理 Update 日志 → 再改内存 → 最后点亮 pageLSN"。
// 一次高层操作会产生多条 Update，靠 prev_lsn 串成同一事务链：
//   插入 = 数据区 + 槽数组 + 页头逻辑字段（≈3 条，触发 compact 则更多）
//   删除 = 槽位置墓碑 + 页头逻辑字段（≈2 条）
// pageLSN 是"日志的元数据"，直接写、不记日志。
//
// 【Slotted Page 布局】
// ┌─ Page (4096 bytes) ─────────────────────────────────┐
// │ [0..8)        PageHeader 逻辑字段（记日志）           │
// │ [8..16)       page_lsn（不记日志）                    │
// │ [16..16+N*4)  Slot Array（向高地址增长）              │
// │ [..free..)    ↕ 空闲区 ↕                             │
// │ [..4096)      Record Data（向低地址增长）             │
// └─────────────────────────────────────────────────────┘

use crate::config::PAGE_SIZE;
use crate::record::Record;

use crate::recovery::log_manager::LogManager;
use crate::recovery::log_record::{LogRecord, TxnId};
use crate::recovery::lsn::{Lsn, INVALID_LSN};
use crate::storage::page::Page;
use crate::storage::page_header::{
    PageHeader, PAGE_HEADER_SIZE, LOGGED_HEADER_SIZE, SLOT_ENTRY_SIZE, DATA_START,
};
use crate::types::{PageId, SlotId};
use std::sync::Arc;


// ==================== WAL 事务上下文（Phase 3 事务管理器的雏形） ====================

/// 最小 WAL 上下文：负责给新日志填 `prev_lsn`、追加后推进 `last_lsn`。
///
/// Slice B 用它在一条语句内把多条 Update 串成链；Phase 3 会由真正的事务管理器创建/持有。
pub struct WalContext {
    log: Arc<LogManager>,
    txn_id: TxnId,
    last_lsn: Lsn,
}


impl WalContext {
    /// 用 TM 已分配的事务 id + Begin 的 lsn 起一个“Update 游标”
    pub fn with_txn(log: Arc<LogManager>, txn_id: TxnId, begin_lsn: Lsn) -> WalContext {
        Self { log, txn_id, last_lsn: begin_lsn }
    }

    /// 本上下文已产生的最后一条日志 LSN（= 下一条日志的 prev_lsn）
    pub fn last_lsn(&self) -> Lsn {
        self.last_lsn
    }

    /// 记录一次"页内字节区间写入"：before 必须已抓取，after 是即将写入的字节（二者等长）。
    fn log_write(&mut self, page_id: PageId, offset: usize, before: Vec<u8>, after: &[u8]) -> Lsn {
        debug_assert_eq!(before.len(), after.len(), "物理日志要求 before/after 等长");
        let rec = LogRecord::Update {
            txn_id: self.txn_id,
            prev_lsn: self.last_lsn,
            page_id,
            offset: offset as u16,
            before,
            after: after.to_vec(),
        };
        let lsn = self.log.append(&rec);
        self.last_lsn = lsn;
        lsn
    }
}
/// 记日志再写内存：先快照 [offset, offset+buf.len()) 的旧字节作为 before，append 一条 Update，
/// 再把新字节写进页（同时置脏）。
fn logged_write(page: &mut Page, wal: &mut WalContext, offset: usize, buf: &[u8]) {
    let before = page.read_data(offset, buf.len()).to_vec();
    wal.log_write(page.id, offset, before, buf);
    page.write_data(offset, buf);
}

// ==================== Slot Array 操作 ====================

fn slot_offset_in_page(slot: usize) -> usize {
    PAGE_HEADER_SIZE + slot * SLOT_ENTRY_SIZE
}

fn read_slot(page: &Page, slot: usize) -> (u16, u16) {
    let off = slot_offset_in_page(slot);
    let buf = page.read_data(off, SLOT_ENTRY_SIZE);
    let offset = u16::from_le_bytes(buf[0..2].try_into().unwrap());
    let length = u16::from_le_bytes(buf[2..4].try_into().unwrap());
    (offset, length)
}
fn find_reusable_slot(page: &Page, slot_count: usize) -> Option<usize> {

    for s in 0..slot_count {
        let (offset, _) = read_slot(page, s);
        if offset == 0 {
            return Some(s);
        }
    }
    None
}

/// 组装一个槽条目字节（offset==0 表示墓碑）
fn slot_bytes(offset: u16, length: u16) -> [u8; SLOT_ENTRY_SIZE] {
    let mut buf = [0u8; SLOT_ENTRY_SIZE];
    buf[0..2].copy_from_slice(&offset.to_le_bytes());
    buf[2..4].copy_from_slice(&length.to_le_bytes());
    buf
}

/// 直接写槽（不记日志）：仅供 compact 内部构建新状态，改动由 compact 汇总成一条 Update 记录
fn write_slot(page: &mut Page, slot: usize, offset: u16, length: u16) {
    let off = slot_offset_in_page(slot);
    page.write_data(off, &slot_bytes(offset, length));
}

// ==================== CRUD 操作 ====================

/// 向页中插入一条记录（记 WAL：数据区 + 槽 + 页头三条 Update，最后点亮 pageLSN）
///
/// # Returns
/// - `Some(slot_id)` — 插入成功
/// - `None` — 空间不足（尝试压缩后仍不够则返回 None）
pub fn insert_record(page: &mut Page, record: &Record, wal: &mut WalContext) -> Option<SlotId> {
    let mut header = PageHeader::read_from_page(page);
    let record_data = record.data();
    let record_len = record_data.len() as u16;
    let needed = SLOT_ENTRY_SIZE as u16 + record_len;

    // 仅处理“空间不够”：先试 compact，仍不够就点亮 pageLSN 后返回 None（换页由上层负责）
    if header.free_space() < needed {
        if header.has_tombstones() {
            compact(page, wal);
            header = PageHeader::read_from_page(page);
        }
        if header.free_space() < needed {
            if wal.last_lsn() != INVALID_LSN {
                PageHeader::write_page_lsn(page, wal.last_lsn());
            }
            return None;
        }
    }

    // ① 数据区永远写到新的 free_space_start 下方（复用的老洞要等 compaction 才回收）
    let data_offset = header.free_space_start - record_len;
    logged_write(page, wal, data_offset as usize, record_data);

    // ② 选槽：优先复用墓碑槽，没有才追加到末尾
    let append_id = header.slot_count as usize;
    let slot_id = find_reusable_slot(page, append_id).unwrap_or(append_id);
    let is_reuse = slot_id < append_id;
    logged_write(page, wal, slot_offset_in_page(slot_id), &slot_bytes(data_offset, record_len));

    // ③ 页头字段：只有“追加新槽”才让槽数组变长；复用则不动 slot_count/free_space_end
    if !is_reuse {
        header.slot_count += 1;
        header.free_space_end += SLOT_ENTRY_SIZE as u16;
    }
    header.free_space_start = data_offset;
    header.increment_occupied();
    let hb = header.to_bytes();
    logged_write(page, wal, 0, &hb[..LOGGED_HEADER_SIZE]);

    // ④ 点亮 pageLSN
    PageHeader::write_page_lsn(page, wal.last_lsn());

    Some(slot_id as SlotId)
}
/// 从页中读取指定槽位的记录（只读，不产生日志）
pub fn read_record(page: &Page, slot: SlotId) -> std::io::Result<Record> {
    let header = PageHeader::read_from_page(page);
    let slot = slot as usize;
    if slot >= header.slot_count as usize {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("Slot {} out of range [0..{})", slot, header.slot_count),
        ));
    }

    let (offset, length) = read_slot(page, slot);
    if offset == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("Slot {} is a tombstone (deleted)", slot),
        ));
    }

    let data = page.read_data(offset as usize, length as usize).to_vec();
    Record::new(data)
}

/// 删除页中指定槽位的记录（记 WAL：槽置墓碑 + 页头两条 Update，最后点亮 pageLSN）
pub fn delete_record(page: &mut Page, slot: SlotId, wal: &mut WalContext) -> std::io::Result<()> {
    let mut header = PageHeader::read_from_page(page);
    let slot = slot as usize;
    if slot >= header.slot_count as usize {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("Slot {} out of range [0..{})", slot, header.slot_count),
        ));
    }

    let (offset, _) = read_slot(page, slot);
    if offset == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("Slot {} is already deleted", slot),
        ));
    }

    // ① 槽位置为墓碑
    logged_write(page, wal, slot_offset_in_page(slot), &slot_bytes(0, 0));

    // ② 页头 occupied_count
    header.decrement_occupied();
    let hb = header.to_bytes();
    logged_write(page, wal, 0, &hb[..LOGGED_HEADER_SIZE]);

    // ③ 点亮 pageLSN
    PageHeader::write_page_lsn(page, wal.last_lsn());

    Ok(())
}

/// 扫描页中所有存活的记录（只读，不产生日志）
pub fn scan_page(page: &Page) -> Vec<(SlotId, Record)> {
    let header = PageHeader::read_from_page(page);
    let mut records = Vec::new();

    for slot in 0..header.slot_count as usize {
        let (offset, length) = read_slot(page, slot);
        if offset == 0 {
            continue;
        }
        let data = page.read_data(offset as usize, length as usize).to_vec();
        if let Ok(record) = Record::new(data) {
            records.push((slot as SlotId, record));
        }
    }

    records
}

// ==================== Compaction（碎片压缩） ====================

/// 压缩页内碎片：把所有存活记录移到页尾，回收墓碑空间。
///
/// 教学简化：把整个"槽数组 + 数据区" [PAGE_HEADER_SIZE..PAGE_SIZE) 的新旧快照
/// 作为**一条**等长 Update 记录（before/after 各 4080 字节），再对页头单独记一条。
fn compact(page: &mut Page, wal: &mut WalContext) {
    let header = PageHeader::read_from_page(page);

    let body_off = PAGE_HEADER_SIZE;
    let body_len = PAGE_SIZE - PAGE_HEADER_SIZE;
    let body_before = page.read_data(body_off, body_len).to_vec();

    let mut live_slots: Vec<(usize, Vec<u8>)> = Vec::new();
    for slot in 0..header.slot_count as usize {
        let (offset, length) = read_slot(page, slot);
        if offset > 0 {
            let data = page.read_data(offset as usize, length as usize).to_vec();
            live_slots.push((slot, data));
        }
    }

    let mut new_free_start = DATA_START;
    for item in &live_slots {
        let data = &item.1;
        new_free_start -= data.len() as u16;
        page.write_data(new_free_start as usize, data);
    }

    let new_free_end = PAGE_HEADER_SIZE as u16 + header.slot_count as u16 * SLOT_ENTRY_SIZE as u16;

    for (i, item) in live_slots.iter().enumerate() {
        let data = &item.1;
        let mut cumulative = new_free_start;
        for j in 0..=i {
            cumulative += live_slots[j].1.len() as u16;
        }
        let this_offset = cumulative - data.len() as u16;
        write_slot(page, i, this_offset, data.len() as u16);
    }

    // 汇总整块 body 的新旧，作为一条 Update 记录
    let body_after = page.read_data(body_off, body_len);
    wal.log_write(page.id, body_off, body_before, body_after);

    // 页头逻辑字段
    let mut new_header = PageHeader::read_from_page(page);
    new_header.free_space_start = new_free_start;
    new_header.free_space_end = new_free_end;
    new_header.occupied_count = live_slots.len() as u16;
    let hb = new_header.to_bytes();
    logged_write(page, wal, 0, &hb[..LOGGED_HEADER_SIZE]);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_page() -> Page {
        let mut page = Page::new(1);
        let header = PageHeader::new();
        header.write_to_page(&mut page);
        page.is_dirty = false;
        page
    }

    /// 为单个测试创建独立日志文件，避免并行测试互相干扰
    fn test_wal(tag: &str) -> WalContext {
        let _ = std::fs::create_dir_all("./data");
        let path = format!("./data/test_page_ops_{}.log", tag);
        let _ = std::fs::remove_file(&path);
        let lm = Arc::new(LogManager::from_path(&path).unwrap());
        WalContext::with_txn(lm, 1, INVALID_LSN)
    }

    #[test]
    fn test_insert_and_read() {
        let mut page = make_test_page();
        let mut wal = test_wal("insert_and_read");
        let record = Record::new(b"Hello, World!".to_vec()).unwrap();

        let slot = insert_record(&mut page, &record, &mut wal).unwrap();
        assert_eq!(slot, 0);

        let restored = read_record(&page, slot).unwrap();
        assert_eq!(restored.data(), record.data());
    }

    #[test]
    fn test_insert_writes_update_log_and_page_lsn() {
        let mut page = make_test_page();
        let mut wal = test_wal("logged_insert");
        assert_eq!(PageHeader::read_page_lsn(&page), INVALID_LSN);

        let rec = Record::new(b"logged!".to_vec()).unwrap();
        let _slot = insert_record(&mut page, &rec, &mut wal).unwrap();

        // 产生了日志
        assert!(wal.last_lsn() > INVALID_LSN, "insert 必须 append Update 日志");
        // pageLSN 被点亮，等于本次操作最后一条日志的 LSN
        assert_eq!(PageHeader::read_page_lsn(&page), wal.last_lsn());
    }

    #[test]
    fn test_insert_multiple() {
        let mut page = make_test_page();
        let mut wal = test_wal("insert_multiple");

        let mut count = 0u32;
        loop {
            let data = format!("record-{}", count).into_bytes();
            let record = Record::new(data).unwrap();
            if insert_record(&mut page, &record, &mut wal).is_none() {
                break;
            }
            count += 1;
        }

        let header = PageHeader::read_from_page(&page);
        assert_eq!(header.occupied_count as u32, count);
        assert!(count > 100, "should fit over 100 small records, got {}", count);
    }

    #[test]
    fn test_delete_and_reinsert() {
        let mut page = make_test_page();
        let mut wal = test_wal("delete_and_reinsert");

        let r1 = Record::new(b"first".to_vec()).unwrap();
        let r2 = Record::new(b"second".to_vec()).unwrap();
        let s1 = insert_record(&mut page, &r1, &mut wal).unwrap();
        let _s2 = insert_record(&mut page, &r2, &mut wal).unwrap();

        delete_record(&mut page, s1, &mut wal).unwrap();
        assert!(read_record(&page, s1).is_err());

        let header = PageHeader::read_from_page(&page);
        assert_eq!(header.occupied_count, 1);

        let r3 = Record::new(b"third".to_vec()).unwrap();
        let s3 = insert_record(&mut page, &r3, &mut wal).unwrap();
        assert_eq!(s3, s1);

        let restored = read_record(&page, s3).unwrap();
        assert_eq!(restored.data(), r3.data());
    }

    #[test]
    fn test_scan_page() {
        let mut page = make_test_page();
        let mut wal = test_wal("scan_page");

        let r1 = Record::new(b"aaa".to_vec()).unwrap();
        let r2 = Record::new(b"bbb".to_vec()).unwrap();
        let r3 = Record::new(b"ccc".to_vec()).unwrap();
        insert_record(&mut page, &r1, &mut wal).unwrap();
        insert_record(&mut page, &r2, &mut wal).unwrap();
        insert_record(&mut page, &r3, &mut wal).unwrap();

        let results = scan_page(&page);
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].0, 0);
        assert_eq!(results[0].1.data(), b"aaa");
        assert_eq!(results[1].0, 1);
        assert_eq!(results[1].1.data(), b"bbb");
        assert_eq!(results[2].0, 2);
        assert_eq!(results[2].1.data(), b"ccc");
    }

    #[test]
    fn test_delete_nonexistent_slot() {
        let mut page = make_test_page();
        let mut wal = test_wal("delete_nonexistent");
        assert!(delete_record(&mut page, 3, &mut wal).is_err());
    }

    #[test]
    fn test_read_empty_slot() {
        let page = make_test_page();
        assert!(read_record(&page, 0).is_err());
    }

    #[test]
    fn test_compaction_reclaims_space() {
        let mut page = make_test_page();
        let mut wal = test_wal("compaction");

        let s0 = insert_record(&mut page, &Record::new(b"AAAA".to_vec()).unwrap(), &mut wal).unwrap();
        let s1 = insert_record(&mut page, &Record::new(b"BBBB".to_vec()).unwrap(), &mut wal).unwrap();
        let _s2 = insert_record(&mut page, &Record::new(b"CCCC".to_vec()).unwrap(), &mut wal).unwrap();

        let header_before = PageHeader::read_from_page(&page);
        let free_before = header_before.free_space();

        delete_record(&mut page, s0, &mut wal).unwrap();
        delete_record(&mut page, s1, &mut wal).unwrap();

        let big_data = vec![0xAB; 100];
        let big_record = Record::new(big_data.clone()).unwrap();
        let s3 = insert_record(&mut page, &big_record, &mut wal);
        assert!(s3.is_some(), "compaction should reclaim enough space");

        let header_after = PageHeader::read_from_page(&page);
        assert!(header_after.free_space() > free_before - 110);
        assert_eq!(header_after.occupied_count, 2);

        let results = scan_page(&page);
        assert_eq!(results.len(), 2);
    }
}