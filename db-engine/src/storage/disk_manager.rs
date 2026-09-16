// src/storage/disk_manager.rs
//
// 【模块职责】
// 最底层的文件 IO 操作封装，是整个 DBMS 的"地基"。
// 类比 Java：类似于 JDBC 最底层直接操作 FileOutputStream 的那层代码。
// 所有数据的持久化最终都要经过它：seek 到文件指定位置，然后 read 或 write。

use std::fs::File;
use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};

use crate::config::{DB_FILE, PAGE_SIZE};

/// 磁盘管理器：封装数据库文件的随机读写操作
///
/// 每次读写都以"页"为单位，通过页号计算文件偏移量：
/// `offset = page_id × PAGE_SIZE`
pub struct DiskManager {
    file: File,
}

impl DiskManager {
    /// 打开或创建数据库文件并返回磁盘管理器
    ///
    /// 如果文件不存在会自动创建，同时确保父目录存在。
    pub fn new() -> std::io::Result<Self> {
        if let Some(parent) = std::path::Path::new(DB_FILE).parent() {
            std::fs::create_dir_all(parent)?;
        }

        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(DB_FILE)?;

        Ok(Self { file })
    }

    pub fn from_file(file: File) -> Self {
        Self { file }
    }

    /// 从磁盘读取指定页号的数据到缓冲区
    ///
    /// 如果请求的页号超出文件当前大小，会自动用 0 填充扩展文件。
    /// - `page_id = 0` → 读文件开头的前 4096 字节
    /// - `page_id = 1` → 跳过前 4096 字节，读下一个 4096 字节
    pub fn read_page(&mut self, page_id: u32, buffer: &mut [u8]) -> std::io::Result<()> {
        self.ensure_page_exists(page_id)?;
        let offset = page_id as u64 * PAGE_SIZE as u64;
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.read_exact(buffer)?;
        Ok(())
    }

    // ... existing code ...

    /// 预分配指定页数的磁盘空间（用 0 填充）
    ///
    /// 例如 `preallocate(2)` 会在文件中写入 2 × 4096 = 8192 字节的 0，
    /// 确保后续 `read_page` 不会因文件太短而失败。
    pub fn preallocate(&mut self, num_pages: u32) -> std::io::Result<()> {
        let zeros = [0u8; PAGE_SIZE];
        for page_id in 0..num_pages {
            self.write_page(page_id, &zeros)?;
        }
        Ok(())
    }

    /// 确保指定页号在文件中存在，不存在则自动扩展（用 0 填充）
    ///
    /// 这是自动扩写的核心逻辑：
    /// 如果文件当前只有 2 页，但请求了第 5 页，
    /// 就会自动写入第 2、3、4 页（全 0），让文件足够大。
    fn ensure_page_exists(&mut self, page_id: u32) -> std::io::Result<()> {
        let file_len = self.file.metadata()?.len();
        let needed_size = (page_id as u64 + 1) * PAGE_SIZE as u64;

        if file_len < needed_size {
            let zeros = [0u8; PAGE_SIZE];
            let current_pages = (file_len / PAGE_SIZE as u64) as u32;
            for pid in current_pages..=page_id {
                self.write_page(pid, &zeros)?;
            }
        }

        Ok(())
    }

    /// 将缓冲区的数据写入磁盘指定页号的位置
    pub fn write_page(&mut self, page_id: u32, buffer: &[u8]) -> std::io::Result<()> {
        let offset = page_id as u64 * PAGE_SIZE as u64;
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.write_all(buffer)?;
        self.file.flush()?;
        Ok(())
    }

    /// 获取数据库文件的当前大小（字节数）
    pub fn file_size(&self) -> std::io::Result<u64> {
        Ok(self.file.metadata()?.len())
    }
    pub fn is_empty(&self) -> std::io::Result<bool> {
        Ok(self.file.metadata()?.len() == 0)
    }



}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_disk_manager_write_and_read() {
        let test_file = "./data/test_dm.db";
        let _ = fs::remove_file(test_file);
        fs::create_dir_all("./data").unwrap();

        let mut dm = {
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .open(test_file)
                .unwrap();
            DiskManager { file }
        };

        let write_data = [0xAA; PAGE_SIZE];
        dm.write_page(0, &write_data).unwrap();

        let mut read_buf = [0u8; PAGE_SIZE];
        dm.read_page(0, &mut read_buf).unwrap();
        assert_eq!(read_buf, write_data);

        let _ = fs::remove_file(test_file);
    }
}