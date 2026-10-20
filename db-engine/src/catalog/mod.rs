// src/catalog/mod.rs
//
// 【模块职责】
// 目录管理器：管理所有表的元数据（DDL），并提供持久化能力。
// 类比 Java：相当于数据库的 System Catalog / Data Dictionary。
//
// 【磁盘布局 — 多页链表结构】
// Page 2: Catalog Head Page
//   [PageHeader: 16B] [u32 magic=0x4341544C] [u32 version=1]
//   [u32 table_count_on_page] [u32 next_catalog_page_id]
//   [TableEntry1][TableEntry2]...
//
// Page N: Catalog Extension Page（可选，链表后续节点）
//   [PageHeader: 16B] [u32 magic=0x4341544C] [u32 version=1]
//   [u32 table_count_on_page] [u32 next_catalog_page_id]
//   [TableEntryN][TableEntryN+1]...
//
// 每个 TableEntry:
//   [u16 name_len][name_bytes]
//   [u32 first_data_page][u32 page_count][u32 record_count]
//   [u16 data_pages_len][page_id_1][page_id_2]...
//   [u16 column_count]
//   [Column1.serialize()][Column2.serialize()]...

use std::collections::HashMap;
use std::sync::Arc;

use crate::config::PAGE_SIZE;
use crate::storage::buffer_pool::BufferPool;
use crate::storage::free_page_manager::FreePageManager;
use crate::storage::page_header::{PageHeader, PAGE_HEADER_SIZE};
use crate::table::{Column, Table};
use crate::types::PageId;

/// 第一个 Catalog 页固定在 Page 2
const FIRST_CATALOG_PAGE_ID: PageId = 2;

/// Catalog 魔数
const CATALOG_MAGIC: u32 = 0x4341544C; // "CATL"

/// Catalog 版本号
const CATALOG_VERSION: u32 = 1;

/// Catalog 页内头部（不含 PageHeader）
/// magic(4) + version(4) + table_count(4) + next_page_id(4) = 16 bytes
const CATALOG_PAGE_HEADER_SIZE: usize = 16;

/// 单张表的元数据
#[derive(Debug, Clone)]
pub struct TableMeta {
    pub name: String,
    pub columns: Vec<Column>,
    pub first_data_page: PageId,
    pub page_count: u32,
    pub record_count: u32,
    /// 该表所有数据页的页号列表（可能不连续）
    pub data_pages: Vec<PageId>,
}

impl TableMeta {
    /// 序列化表元数据
    pub fn serialize(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        let name_bytes = self.name.as_bytes();
        buf.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        buf.extend_from_slice(name_bytes);

        buf.extend_from_slice(&self.first_data_page.to_le_bytes());
        buf.extend_from_slice(&self.page_count.to_le_bytes());
        buf.extend_from_slice(&self.record_count.to_le_bytes());

        // 序列化 data_pages 列表
        buf.extend_from_slice(&(self.data_pages.len() as u16).to_le_bytes());
        for &page_id in &self.data_pages {
            buf.extend_from_slice(&page_id.to_le_bytes());
        }

        buf.extend_from_slice(&(self.columns.len() as u16).to_le_bytes());
        for col in &self.columns {
            buf.extend_from_slice(&col.serialize());
        }

        buf
    }

    /// 反序列化表元数据
    pub fn deserialize(data: &[u8], offset: &mut usize) -> std::io::Result<Self> {
        if *offset + 2 > data.len() {
            return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "Not enough data for table name length"));
        }
        let name_len = u16::from_le_bytes(data[*offset..*offset + 2].try_into().unwrap()) as usize;
        *offset += 2;

        if *offset + name_len > data.len() {
            return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "Not enough data for table name"));
        }
        let name = std::str::from_utf8(&data[*offset..*offset + name_len])
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, format!("Invalid UTF-8 in table name: {}", e)))?
            .to_string();
        *offset += name_len;

        if *offset + 12 > data.len() {
            return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "Not enough data for table metadata"));
        }
        let first_data_page = u32::from_le_bytes(data[*offset..*offset + 4].try_into().unwrap());
        *offset += 4;
        let page_count = u32::from_le_bytes(data[*offset..*offset + 4].try_into().unwrap());
        *offset += 4;
        let record_count = u32::from_le_bytes(data[*offset..*offset + 4].try_into().unwrap());
        *offset += 4;

        // 反序列化 data_pages 列表
        if *offset + 2 > data.len() {
            return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "Not enough data for data_pages length"));
        }
        let data_pages_len = u16::from_le_bytes(data[*offset..*offset + 2].try_into().unwrap()) as usize;
        *offset += 2;

        let mut data_pages = Vec::with_capacity(data_pages_len);
        for _ in 0..data_pages_len {
            if *offset + 4 > data.len() {
                return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "Not enough data for data_page id"));
            }
            let page_id = u32::from_le_bytes(data[*offset..*offset + 4].try_into().unwrap());
            *offset += 4;
            data_pages.push(page_id);
        }

        if *offset + 2 > data.len() {
            return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "Not enough data for column count"));
        }
        let column_count = u16::from_le_bytes(data[*offset..*offset + 2].try_into().unwrap()) as usize;
        *offset += 2;

        let mut columns = Vec::with_capacity(column_count);
        for _ in 0..column_count {
            let col = Column::deserialize(data, offset)?;
            columns.push(col);
        }

        Ok(Self {
            name,
            columns,
            first_data_page,
            page_count,
            record_count,
            data_pages,
        })
    }
}

/// 目录管理器：管理所有表的元数据（支持多页链表存储）
pub struct Catalog {
    buffer_pool: Arc<BufferPool>,
    tables: HashMap<String, TableMeta>,
    /// 当前 Catalog 链使用的页号列表（第一项恒为 FIRST_CATALOG_PAGE_ID）
    catalog_page_ids: Vec<PageId>,
    dirty: bool,
}

impl Catalog {
    /// 创建新的 Catalog（空）
    pub fn new(buffer_pool: Arc<BufferPool>) -> Self {
        Self {
            buffer_pool,
            tables: HashMap::new(),
            catalog_page_ids: vec![FIRST_CATALOG_PAGE_ID],
            dirty: false,
        }
    }

    /// 每个 Catalog 页可用于 TableEntry 的最大字节数
    fn max_entries_bytes_per_page() -> usize {
        PAGE_SIZE - PAGE_HEADER_SIZE - CATALOG_PAGE_HEADER_SIZE
    }

    /// 从元数据构建 Table 对象（重启恢复时使用）
    pub fn open_table(&self, name: &str) -> std::io::Result<Table> {
        let meta = self.tables.get(name).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, format!("Table '{}' not found", name))
        })?;
        Table::from_meta(meta, self.buffer_pool.clone())
    }

    /// 将一个 Catalog 页的内容写入 BufferPool
    fn write_catalog_page_to_buffer(page: &mut crate::storage::page::Page, table_entries: &[Vec<u8>], next_page_id: PageId) {
        PageHeader::new().write_to_page(page);

        let ph = PAGE_HEADER_SIZE;
        page.write_data(ph, &CATALOG_MAGIC.to_le_bytes());
        page.write_data(ph + 4, &CATALOG_VERSION.to_le_bytes());
        page.write_data(ph + 8, &(table_entries.len() as u32).to_le_bytes());
        page.write_data(ph + 12, &next_page_id.to_le_bytes());

        let mut offset = ph + CATALOG_PAGE_HEADER_SIZE;
        for entry in table_entries {
            page.write_data(offset, entry);
            offset += entry.len();
        }
    }

    /// 从磁盘加载单个 Catalog 页，返回本页信息（表数量、下一页 ID）
    fn load_one_catalog_page(page_id: PageId, buffer_pool: &Arc<BufferPool>, tables: &mut HashMap<String, TableMeta>) -> std::io::Result<(u32, PageId)> {
        let page_ref = buffer_pool.fetch_page(page_id)?;
        let page = page_ref.read().unwrap();

        let ph = PAGE_HEADER_SIZE;
        let data = page.read_data(ph, PAGE_SIZE - ph);

        if data.len() < 4 {
            return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "Catalog page too small"));
        }

        let magic = u32::from_le_bytes(data[0..4].try_into().unwrap());
        if magic != CATALOG_MAGIC {
            return Ok((0, 0));
        }

        if data.len() < CATALOG_PAGE_HEADER_SIZE {
            return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "Not enough data for catalog page header"));
        }

        let _version = u32::from_le_bytes(data[4..8].try_into().unwrap());
        let table_count = u32::from_le_bytes(data[8..12].try_into().unwrap());
        let next_page_id = u32::from_le_bytes(data[12..16].try_into().unwrap());

        let mut offset = CATALOG_PAGE_HEADER_SIZE;
        for _ in 0..table_count {
            let meta = TableMeta::deserialize(data, &mut offset)?;
            tables.insert(meta.name.clone(), meta);
        }

        Ok((table_count, next_page_id))
    }

    /// 从磁盘加载 Catalog（遍历链表）
    pub fn load(buffer_pool: Arc<BufferPool>) -> std::io::Result<Self> {
        let mut tables = HashMap::new();
        let mut catalog_page_ids = vec![FIRST_CATALOG_PAGE_ID];

        let (_, next_page_id) = Self::load_one_catalog_page(FIRST_CATALOG_PAGE_ID, &buffer_pool, &mut tables)?;

        let mut current_page_id = next_page_id;
        while current_page_id != 0 {
            catalog_page_ids.push(current_page_id);
            let (_, next) = Self::load_one_catalog_page(current_page_id, &buffer_pool, &mut tables)?;
            current_page_id = next;
        }

        Ok(Self {
            buffer_pool,
            tables,
            catalog_page_ids,
            dirty: false,
        })
    }

    /// 持久化 Catalog 到磁盘（支持多页链表，需要 FreePageManager 分配扩展页）
    pub fn flush(&mut self, free_mgr: &mut FreePageManager) -> std::io::Result<()> {
        let all_entries: Vec<Vec<u8>> = self.tables.values()
            .map(|meta| meta.serialize())
            .collect();

        let max_bytes = Self::max_entries_bytes_per_page();

        // 将表条目按页容量分组
        let mut pages_data: Vec<Vec<Vec<u8>>> = Vec::new();
        let mut current_page_entries: Vec<Vec<u8>> = Vec::new();
        let mut current_page_used: usize = 0;

        for entry in &all_entries {
            let entry_len = entry.len();
            if current_page_used + entry_len > max_bytes && !current_page_entries.is_empty() {
                pages_data.push(std::mem::take(&mut current_page_entries));
                current_page_used = 0;
            }
            current_page_entries.push(entry.clone());
            current_page_used += entry_len;
        }
        if !current_page_entries.is_empty() {
            pages_data.push(current_page_entries);
        }

        // 空 Catalog 也要保证至少首页
        if pages_data.is_empty() {
            pages_data.push(Vec::new());
        }

        // 确保首页在列表中
        if self.catalog_page_ids.is_empty() {
            self.catalog_page_ids.push(FIRST_CATALOG_PAGE_ID);
        }

        // 为超出的页分配新的扩展页
        while self.catalog_page_ids.len() < pages_data.len() {
            let new_page_id = free_mgr.find_free_page()
                .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::Other, "No free pages for catalog extension"))?;
            free_mgr.mark_used(new_page_id);
            self.catalog_page_ids.push(new_page_id);
        }

        // 释放多余的旧扩展页
        while self.catalog_page_ids.len() > pages_data.len() {
            let removed_page_id = self.catalog_page_ids.pop().unwrap();
            if removed_page_id != FIRST_CATALOG_PAGE_ID {
                free_mgr.mark_free(removed_page_id);
            }
        }

        // 确保首页被标记为已使用
        free_mgr.mark_used(FIRST_CATALOG_PAGE_ID);

        // 写入所有 Catalog 页（设置链表指针）
        for (i, entries) in pages_data.iter().enumerate() {
            let page_id = self.catalog_page_ids[i];
            let next_page_id = if i + 1 < pages_data.len() {
                self.catalog_page_ids[i + 1]
            } else {
                0
            };

            let page_ref = self.buffer_pool.fetch_page(page_id)?;
            {
                let mut page = page_ref.write().unwrap();
                Self::write_catalog_page_to_buffer(&mut page, entries, next_page_id);
            }
        }

        // 同步 FreePageManager 位图到 BufferPool
        free_mgr.flush(&self.buffer_pool)?;

        self.dirty = false;
        Ok(())
    }

    /// 创建一张新表
    pub fn create_table(
        &mut self,
        name: String,
        columns: Vec<Column>,
        free_mgr: &mut FreePageManager,
    ) -> std::io::Result<Table> {
        if self.tables.contains_key(&name) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("Table '{}' already exists", name),
            ));
        }

        // 分配第一个数据页
        let first_data_page = free_mgr.find_free_page().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::Other, "No free pages available")
        })?;
        free_mgr.mark_used(first_data_page);

        // 创建 Table（自动初始化 Slotted Page 页头）
        let table = Table::new(name.clone(), columns.clone(), first_data_page, self.buffer_pool.clone())?;

        // 记录元数据
        let meta = TableMeta {
            name: name.clone(),
            columns,
            first_data_page,
            page_count: 1,
            record_count: 0,
            data_pages: vec![first_data_page],
        };
        self.tables.insert(name, meta);
        self.dirty = true;

        // 自动持久化
        self.flush(free_mgr)?;

        Ok(table)
    }

    /// 删除一张表
    pub fn drop_table(&mut self, name: &str, free_mgr: &mut FreePageManager) -> std::io::Result<()> {
        let meta = self.tables.remove(name).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, format!("Table '{}' not found", name))
        })?;

        // 释放所有数据页
        for &page_id in &meta.data_pages {
            free_mgr.mark_free(page_id);
        }

        self.dirty = true;
        self.flush(free_mgr)?;

        Ok(())
    }

    /// 获取表元数据
    pub fn get_table_meta(&self, name: &str) -> Option<&TableMeta> {
        self.tables.get(name)
    }

    /// 列出所有表名
    pub fn list_tables(&self) -> Vec<&str> {
        self.tables.keys().map(|s| s.as_str()).collect()
    }

    /// 更新表的元数据（插入/删除后调用）
    pub fn update_table_meta(&mut self, name: &str, page_count: u32, record_count: u32, data_pages: Vec<PageId>, free_mgr: &mut FreePageManager) -> std::io::Result<()> {
        if let Some(meta) = self.tables.get_mut(name) {
            meta.page_count = page_count;
            meta.record_count = record_count;
            meta.data_pages = data_pages;
            self.dirty = true;
            self.flush(free_mgr)?;
        }
        Ok(())
    }

    /// 同步表的列定义到 Catalog 并持久化（ALTER 操作后调用）
    pub fn update_columns(&mut self, table_name: &str, columns: &[Column], free_mgr: &mut FreePageManager) -> std::io::Result<()> {
        let meta = self.tables.get_mut(table_name).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, format!("Table '{}' not found", table_name))
        })?;
        meta.columns = columns.to_vec();
        self.dirty = true;
        self.flush(free_mgr)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::disk_manager::DiskManager;
    use crate::recovery::log_manager::LogManager;

    fn setup() -> (Arc<BufferPool>, FreePageManager) {
        let test_file = "./data/test_catalog.db";
        let _ = std::fs::remove_file(test_file);
        std::fs::create_dir_all("./data").unwrap();

        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(test_file)
            .unwrap();

        let dm = DiskManager::from_file(file);
        let test_log = "./data/test_catalog.log";
        let _ = std::fs::remove_file(test_log);
        let lm = Arc::new(LogManager::from_path(test_log).unwrap());
        let bp = Arc::new(BufferPool::new(dm, lm));

        // 预分配足够的页（支持多页 Catalog 扩展）
        for pid in 0..100u32 {
            let _pref = bp.fetch_page(pid).unwrap();
        }

        let mut fpm = FreePageManager::new(crate::config::PAGE_SIZE as u64, 1024);
        fpm.mark_used(0); // FileHeader

        (bp, fpm)
    }

    #[test]
    fn test_create_and_list_tables() {
        let (bp, mut fpm) = setup();
        let mut catalog = Catalog::new(bp);

        let cols = vec![
            Column::int("id").not_null().auto_increment(),
            Column::varchar("name", 50).not_null().default_varchar("unknown"),
        ];

        let _table = catalog.create_table("users".into(), cols.clone(), &mut fpm).unwrap();

        let tables = catalog.list_tables();
        assert_eq!(tables, vec!["users"]);

        let meta = catalog.get_table_meta("users").unwrap();
        assert_eq!(meta.columns.len(), 2);
        assert_eq!(meta.first_data_page, 1);
        assert_eq!(meta.data_pages, vec![1]);
    }

    #[test]
    fn test_duplicate_table_rejected() {
        let (bp, mut fpm) = setup();
        let mut catalog = Catalog::new(bp);

        let cols = vec![Column::int("id")];
        let _t1 = catalog.create_table("test".into(), cols.clone(), &mut fpm).unwrap();
        let result = catalog.create_table("test".into(), cols, &mut fpm);
        assert!(result.is_err());
    }

    #[test]
    fn test_drop_table() {
        let (bp, mut fpm) = setup();
        let mut catalog = Catalog::new(bp);

        let cols = vec![Column::int("id")];
        let _t = catalog.create_table("temp".into(), cols, &mut fpm).unwrap();
        assert_eq!(catalog.list_tables().len(), 1);

        catalog.drop_table("temp", &mut fpm).unwrap();
        assert!(catalog.list_tables().is_empty());
    }

    #[test]
    fn test_persistence_roundtrip() {
        let (bp, mut fpm) = setup();
        let mut catalog = Catalog::new(bp.clone());

        let cols = vec![
            Column::int("id").not_null().auto_increment(),
            Column::varchar("name", 100).default_varchar("N/A"),
        ];
        let _t = catalog.create_table("persist_test".into(), cols.clone(), &mut fpm).unwrap();

        // 模拟重启：重新加载 Catalog
        let catalog2 = Catalog::load(bp).unwrap();
        let meta = catalog2.get_table_meta("persist_test").unwrap();
        assert_eq!(meta.columns.len(), 2);
        assert_eq!(meta.columns[0].name, "id");
        assert_eq!(meta.columns[1].name, "name");
        assert!(!meta.columns[0].nullable);
        assert!(meta.columns[0].auto_increment);
        assert_eq!(meta.data_pages, vec![1]);
    }

    #[test]
    fn test_multi_page_catalog() {
        let (bp, mut fpm) = setup();
        let mut catalog = Catalog::new(bp.clone());

        // 创建大量表以触发多页扩展
        let num_tables = 50;
        for i in 0..num_tables {
            let cols = vec![
                Column::int("id"),
                Column::varchar(&format!("col_{}", i), 20),
            ];
            let table_name = format!("table_{:03}", i);
            let _t = catalog.create_table(table_name, cols, &mut fpm).unwrap();
        }

        assert_eq!(catalog.list_tables().len(), num_tables);

        // 验证持久化后能正确加载（遍历链表）
        let catalog2 = Catalog::load(bp).unwrap();
        assert_eq!(catalog2.list_tables().len(), num_tables);

        // 随机验证几个表
        for i in [0, 10, 25, 49] {
            let table_name = format!("table_{:03}", i);
            let meta = catalog2.get_table_meta(&table_name).unwrap();
            assert_eq!(meta.name, table_name);
            assert_eq!(meta.columns.len(), 2);
            assert_eq!(meta.data_pages.len(), 1);
        }
    }
}