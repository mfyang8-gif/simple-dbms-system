// src/storage/page_header.rs
//
// 【模块职责】
// 数据页的页头，记录本页的槽位信息。
// 类比 Java：类似于一个页级别的元数据结构。

use crate::config::SLOTS_PER_PAGE;

/// 页头固定大小（字节数）
pub const PAGE_HEADER_SIZE: usize = 4;

/// 页头：管理单个数据页的槽位信息
#[derive(Debug, Clone)]
pub struct PageHeader {
    /// 本页总槽位数
    pub slot_count: u16,
    /// 本页已占用的槽位数
    pub occupied_count: u16,
}

impl PageHeader {
    /// 创建一个全新的页头（所有槽位空闲）
    pub fn new() -> Self {
        Self {
            slot_count: SLOTS_PER_PAGE as u16,
            occupied_count: 0,
        }
    }

    /// 从字节数组反序列化页头
    pub fn from_bytes(buf: &[u8]) -> Self {
        let slot_count = u16::from_le_bytes(buf[0..2].try_into().unwrap());
        let occupied_count = u16::from_le_bytes(buf[2..4].try_into().unwrap());
        Self {
            slot_count,
            occupied_count,
        }
    }

    /// 序列化为字节数组
    pub fn to_bytes(&self) -> [u8; PAGE_HEADER_SIZE] {
        let mut buf = [0u8; PAGE_HEADER_SIZE];
        buf[0..2].copy_from_slice(&self.slot_count.to_le_bytes());
        buf[2..4].copy_from_slice(&self.occupied_count.to_le_bytes());
        buf
    }

    /// 将页头写入指定页的开头
    pub fn write_to_page(&self, page: &mut crate::storage::page::Page) {
        let buf = self.to_bytes();
        page.write_data(0, &buf);
    }

    /// 从指定页的开头读取页头
    pub fn read_from_page(page: &crate::storage::page::Page) -> Self {
        let buf = page.read_data(0, PAGE_HEADER_SIZE);
        Self::from_bytes(buf)
    }

    /// 增加占用计数（插入记录时调用）
    pub fn increment_occupied(&mut self) {
        self.occupied_count += 1;
    }

    /// 减少占用计数（删除记录时调用）
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
            slot_count: 7,
            occupied_count: 3,
        };

        let buf = header.to_bytes();
        let restored = PageHeader::from_bytes(&buf);

        assert_eq!(restored.slot_count, header.slot_count);
        assert_eq!(restored.occupied_count, header.occupied_count);
    }

    #[test]
    fn test_write_to_and_read_from_page() {
        let mut page = Page::new(1);
        let header = PageHeader::new();
        header.write_to_page(&mut page);

        let restored = PageHeader::read_from_page(&page);
        assert_eq!(restored.slot_count, SLOTS_PER_PAGE as u16);
        assert_eq!(restored.occupied_count, 0);
    }

    #[test]
    fn test_increment_decrement() {
        let mut header = PageHeader::new();
        assert_eq!(header.occupied_count, 0);

        header.increment_occupied();
        header.increment_occupied();
        assert_eq!(header.occupied_count, 2);

        header.decrement_occupied();
        assert_eq!(header.occupied_count, 1);
    }
}