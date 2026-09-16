// src/storage/page.rs
//
// 【模块职责】
// 定义"页"——数据库与磁盘交换数据的最小单位。
// 类比 Java：类似于 MyBatis 中一个 ResultSet 对应的内存缓冲块。
// 操作系统一次 IO 至少读一个块，数据库也一样，这个"块"就是 Page。

use crate::config::PAGE_SIZE;

/// 数据库页：固定 4096 字节的内存缓冲区
///
/// 每个页对应磁盘文件中的一段连续空间。
/// 读取时：磁盘 → Page.data
/// 写入时：Page.data → 磁盘
/// 数据库页：固定 4096 字节的内存缓冲区
///
/// 每个页对应磁盘文件中的一段连续空间。
/// 读取时：磁盘 → Page.data
/// 写入时：Page.data → 磁盘
#[derive(Debug)]
pub struct Page {
    /// 页号：这个页在文件中的序号（第 0 页、第 1 页...）
    pub id: u32,

    /// 页数据：固定大小的字节数组，直接对应磁盘上的一段连续空间
    pub data: [u8; PAGE_SIZE],

    /// 脏标记：内存中修改了但还没写回磁盘时为 true
    pub is_dirty: bool,

    /// 引用位（Clock 算法用）：每次被访问时置 true，
    /// 时钟指针扫过时会清除，连续两轮没被访问的页会被淘汰
    pub reference_bit: bool,
}

impl Page {
    /// 创建一个指定页号的空白页（所有字节为 0）
    pub fn new(id: u32) -> Self {
        Self {
            id,
            data: [0u8; PAGE_SIZE],
            is_dirty: false,
            reference_bit: false,
        }
    }
    /// 读取页数据中指定范围的字节切片
    ///
    /// # Panics
    /// 当 `offset + len > PAGE_SIZE` 时会 panic
    pub fn read_data(&self, offset: usize, len: usize) -> &[u8] {
        &self.data[offset..offset + len]
    }

    /// 向页数据中写入字节，并标记为脏页
    ///
    /// # Panics
    /// 当 `offset + buf.len() > PAGE_SIZE` 时会 panic
    pub fn write_data(&mut self, offset: usize, buf: &[u8]) {
        self.data[offset..offset + buf.len()].copy_from_slice(buf);
        self.is_dirty = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_page_write_and_read() {
        let mut page = Page::new(0);
        assert!(!page.is_dirty);

        page.write_data(0, b"Hello DBMS");
        assert!(page.is_dirty);
        assert_eq!(page.read_data(0, 10), b"Hello DBMS");
    }

    #[test]
    fn test_page_write_at_offset() {
        let mut page = Page::new(1);
        page.write_data(100, &[0xFF, 0xAA, 0xBB]);
        assert_eq!(page.read_data(100, 3), &[0xFF, 0xAA, 0xBB]);
        assert_eq!(page.data[99], 0);
        assert_eq!(page.data[103], 0);
    }
}