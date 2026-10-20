// src/storage/page_header.rs
//
// 【模块职责】
// Slotted Page 的页头，记录槽位数组、空闲区位置，以及 WAL 恢复所需的 pageLSN。
//
// 【Slotted Page 布局】
// ┌─ Page (4096 bytes) ─────────────────────────────────┐
// │ [0..8)        PageHeader 逻辑字段（参与 WAL 日志）    │
// │ [8..16)       page_lsn（不记日志，是日志的元数据）     │
// │ [16..16+N*4)  Slot Array（向高地址增长）              │
// │ [..free..)    ↕ 空闲区 ↕                             │
// │ [..4096)      Record Data（向低地址增长）             │
// └─────────────────────────────────────────────────────┘

use crate::config::PAGE_SIZE;
use crate::recovery::lsn::{Lsn, INVALID_LSN};

/// Slotted Page 页头大小：16 字节
pub const PAGE_HEADER_SIZE: usize = 16;

/// 参与 WAL 日志的页头部分（slot/free 字段），即 [0..8)
pub const LOGGED_HEADER_SIZE: usize = 8;

/// page_lsn 在页内的字节偏移
pub const PAGE_LSN_OFFSET: usize = 8;

/// page_lsn 字段宽度
pub const PAGE_LSN_SIZE: usize = 8;

/// Slot 条目大小：offset(u16) + length(u16) = 4 字节
pub const SLOT_ENTRY_SIZE: usize = 4;

/// 数据区起始位置（从页尾部算）
pub const DATA_START: u16 = PAGE_SIZE as u16;

#[derive(Debug, Clone)]
pub struct PageHeader {
    pub slot_count: u16,
    pub free_space_start: u16,
    pub free_space_end: u16,
    pub occupied_count: u16,
    /// 最后一次修改本页的日志 LSN（Redo 幂等判断依据）
    pub page_lsn: Lsn,
}

impl PageHeader {
    pub fn new() -> Self {
        Self {
            slot_count: 0,
            free_space_start: DATA_START,
            free_space_end: PAGE_HEADER_SIZE as u16,
            occupied_count: 0,
            page_lsn: INVALID_LSN,
        }
    }

    pub fn from_bytes(buf: &[u8]) -> Self {
        Self {
            slot_count: u16::from_le_bytes(buf[0..2].try_into().unwrap()),
            free_space_start: u16::from_le_bytes(buf[2..4].try_into().unwrap()),
            free_space_end: u16::from_le_bytes(buf[4..6].try_into().unwrap()),
            occupied_count: u16::from_le_bytes(buf[6..8].try_into().unwrap()),
            page_lsn: u64::from_le_bytes(buf[8..16].try_into().unwrap()),
        }
    }

    pub fn to_bytes(&self) -> [u8; PAGE_HEADER_SIZE] {
        let mut buf = [0u8; PAGE_HEADER_SIZE];
        buf[0..2].copy_from_slice(&self.slot_count.to_le_bytes());
        buf[2..4].copy_from_slice(&self.free_space_start.to_le_bytes());
        buf[4..6].copy_from_slice(&self.free_space_end.to_le_bytes());
        buf[6..8].copy_from_slice(&self.occupied_count.to_le_bytes());
        buf[8..16].copy_from_slice(&self.page_lsn.to_le_bytes());
        buf
    }

    pub fn write_to_page(&self, page: &mut crate::storage::page::Page) {
        let buf = self.to_bytes();
        page.write_data(0, &buf);
    }

    pub fn read_from_page(page: &crate::storage::page::Page) -> Self {
        let buf = page.read_data(0, PAGE_HEADER_SIZE);
        Self::from_bytes(buf)
    }

    /// 读取页的 pageLSN（供 BufferPool 做 WAL 检查，不解析整个页头）
    pub fn read_page_lsn(page: &crate::storage::page::Page) -> Lsn {
        u64::from_le_bytes(
            page.read_data(PAGE_LSN_OFFSET, PAGE_LSN_SIZE).try_into().unwrap(),
        )
    }

    /// 直接写入 pageLSN（【不记日志】：pageLSN 是日志的元数据，自身不参与 WAL）
    pub fn write_page_lsn(page: &mut crate::storage::page::Page, lsn: Lsn) {
        page.write_data(PAGE_LSN_OFFSET, &lsn.to_le_bytes());
    }

    pub fn free_space(&self) -> u16 {
        self.free_space_start - self.free_space_end
    }

    pub fn has_tombstones(&self) -> bool {
        self.occupied_count < self.slot_count
    }

    pub fn increment_occupied(&mut self) {
        self.occupied_count += 1;
    }

    pub fn decrement_occupied(&mut self) {
        if self.occupied_count > 0 {
            self.occupied_count -= 1;
        }
    }
}

impl Default for PageHeader {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::page::Page;

    #[test]
    fn test_serialize_deserialize_roundtrip() {
        let header = PageHeader {
            slot_count: 5,
            free_space_start: 4000,
            free_space_end: 28,
            occupied_count: 3,
            page_lsn: 123456789,
        };
        let buf = header.to_bytes();
        let restored = PageHeader::from_bytes(&buf);
        assert_eq!(restored.slot_count, 5);
        assert_eq!(restored.free_space_start, 4000);
        assert_eq!(restored.free_space_end, 28);
        assert_eq!(restored.occupied_count, 3);
        assert_eq!(restored.page_lsn, 123456789);
    }

    #[test]
    fn test_write_to_and_read_from_page() {
        let mut page = Page::new(1);
        let header = PageHeader::new();
        header.write_to_page(&mut page);
        let restored = PageHeader::read_from_page(&page);
        assert_eq!(restored.slot_count, 0);
        assert_eq!(restored.free_space_start, DATA_START);
        assert_eq!(restored.free_space_end, PAGE_HEADER_SIZE as u16);
        assert_eq!(restored.occupied_count, 0);
        assert_eq!(restored.page_lsn, INVALID_LSN);
    }

    #[test]
    fn test_page_lsn_read_write_helpers() {
        let mut page = Page::new(1);
        PageHeader::new().write_to_page(&mut page);
        assert_eq!(PageHeader::read_page_lsn(&page), INVALID_LSN);
        PageHeader::write_page_lsn(&mut page, 4242);
        assert_eq!(PageHeader::read_page_lsn(&page), 4242);
        // 写 pageLSN 不应破坏逻辑字段
        assert_eq!(PageHeader::read_from_page(&page).free_space_end, PAGE_HEADER_SIZE as u16);
    }

    #[test]
    fn test_free_space() {
        let header = PageHeader::new();
        assert_eq!(header.free_space(), DATA_START - PAGE_HEADER_SIZE as u16);
    }

    #[test]
    fn test_increment_decrement() {
        let mut header = PageHeader::new();
        header.increment_occupied();
        header.increment_occupied();
        assert_eq!(header.occupied_count, 2);
        header.decrement_occupied();
        assert_eq!(header.occupied_count, 1);
    }
}