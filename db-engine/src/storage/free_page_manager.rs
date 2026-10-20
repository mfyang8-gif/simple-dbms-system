// src/storage/free_page_manager.rs
//
// 【模块职责】
// 管理哪些页是空闲的，用于快速分配新页。
// 类比 Java：类似于一个全局的空闲资源池。
//
// 【架构原则】
// BufferPool 是所有磁盘 I/O 的唯一入口。
// FreePageManager 不直接持有 DiskManager，
// 而是通过 BufferPool 读写位图所在的页，保证缓存一致性。

use crate::config::PAGE_SIZE;
use crate::storage::buffer_pool::BufferPool;

/// 空闲页管理器：用位图管理哪些页是空的
pub struct FreePageManager {
    /// 位图数据：1 bit = 1 页，0=空闲，1=已使用
    bitmap: Vec<u8>,
    /// 最大管理的页数
    max_pages: u32,
    /// 位图在文件中的起始偏移量（位于 Page 0 内 FileHeader 之后）
    bitmap_offset: u64,
    
    
}

impl FreePageManager {
    /// 创建新的空闲页管理器
    pub fn new(bitmap_offset: u64, max_pages: u32) -> Self {
        let bitmap_size = ((max_pages + 7) / 8) as usize;
        Self {
            bitmap: vec![0u8; bitmap_size],
            max_pages,
            bitmap_offset,
        }
    }


    pub fn total_pages(&self) -> u32 {
        self.max_pages
    }
    /// 从 BufferPool 加载位图到内存
    ///
    /// 通过 BufferPool 获取位图所在的页，从中拷贝位图数据。
    /// 当前配置下 max_pages=1024，位图仅 128 字节，不会跨页。
    pub fn load(&mut self, buffer_pool: &BufferPool) -> std::io::Result<()> {
        let bitmap_size = self.bitmap.len();
        let mut offset_in_file = self.bitmap_offset;
        let mut offset_in_bitmap = 0;

        while offset_in_bitmap < bitmap_size {
            let remaining = bitmap_size - offset_in_bitmap;
            let read_len = remaining.min(PAGE_SIZE);

            let page_id = (offset_in_file / PAGE_SIZE as u64) as u32;
            let page_offset = (offset_in_file % PAGE_SIZE as u64) as usize;

            let page_ref = buffer_pool.fetch_page(page_id)?;
            let page = page_ref.read().unwrap();
            let page_data = page.read_data(page_offset, read_len);

            self.bitmap[offset_in_bitmap..offset_in_bitmap + read_len]
                .copy_from_slice(page_data);

            offset_in_file += read_len as u64;
            offset_in_bitmap += read_len;
        }

        Ok(())
    }

    /// 将内存中的位图写回 BufferPool 中对应的页
    ///
    /// 不直接写磁盘，而是修改 BufferPool 中的页并标记脏页，
    /// 由 BufferPool.flush() 统一刷盘。
    pub fn flush(&mut self, buffer_pool: &BufferPool) -> std::io::Result<()> {
        let bitmap_size = self.bitmap.len();
        let mut offset_in_file = self.bitmap_offset;
        let mut offset_in_bitmap = 0;

        while offset_in_bitmap < bitmap_size {
            let remaining = bitmap_size - offset_in_bitmap;
            let write_len = remaining.min(PAGE_SIZE);

            let page_id = (offset_in_file / PAGE_SIZE as u64) as u32;
            let page_offset = (offset_in_file % PAGE_SIZE as u64) as usize;

            let page_ref = buffer_pool.fetch_page(page_id)?;
            let mut page = page_ref.write().unwrap();
            page.write_data(
                page_offset,
                &self.bitmap[offset_in_bitmap..offset_in_bitmap + write_len],
            );

            offset_in_file += write_len as u64;
            offset_in_bitmap += write_len;
        }

        Ok(())
    }

    /// 标记指定页为已使用
    pub fn mark_used(&mut self, page_id: u32) {
        assert!(page_id < self.max_pages, "page_id {} out of range", page_id);
        let byte_index = (page_id / 8) as usize;
        let bit_index = page_id % 8;
        self.bitmap[byte_index] |= 1 << bit_index;
    }

    /// 标记指定页为空闲
    pub fn mark_free(&mut self, page_id: u32) {
        assert!(page_id < self.max_pages, "page_id {} out of range", page_id);
        let byte_index = (page_id / 8) as usize;
        let bit_index = page_id % 8;
        self.bitmap[byte_index] &= !(1 << bit_index);
    }

    /// 检查指定页是否空闲
    pub fn is_free(&self, page_id: u32) -> bool {
        assert!(page_id < self.max_pages, "page_id {} out of range", page_id);
        let byte_index = (page_id / 8) as usize;
        let bit_index = page_id % 8;
        (self.bitmap[byte_index] >> bit_index) & 1 == 0
    }

    /// 查找第一个空闲页
    pub fn find_free_page(&self) -> Option<u32> {
        for page_id in 0..self.max_pages {
            if self.is_free(page_id) {
                return Some(page_id);
            }
        }
        None
    }

    /// 统计空闲页数
    pub fn count_free_pages(&self) -> u32 {
        (0..self.max_pages).filter(|&i| self.is_free(i)).count() as u32
    }
}