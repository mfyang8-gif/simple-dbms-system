// src/database.rs
//
// 【模块职责】
// 数据库引擎的统一入口，负责协调所有底层组件的生命周期。
// 类比 Java：相当于 DataSource / ConnectionFactory，应用层只需要拿到一个 Database 对象。

use std::sync::{Arc, Mutex};

use crate::catalog::{Catalog, TableMeta};
use crate::config::PAGE_SIZE;
use crate::recovery::log_manager::LogManager;
use crate::recovery::transaction::TransactionManager;
use crate::storage::buffer_pool::BufferPool;
use crate::storage::disk_manager::DiskManager;
use crate::storage::file_header::{FileHeader};
use crate::storage::free_page_manager::FreePageManager;
use crate::table::{Column, Table};
use crate::types::PageId;

/// FreePageManager 位图管理的最大页数
const MAX_PAGES: u32 = 1024;

/// 系统保留页数量：Page 0 (FileHeader) + Page 1 (FreePageManager 位图) + Page 2 (Catalog 首页)
const RESERVED_PAGES: u32 = 3;

/// 数据库引擎：统一管理所有底层组件
pub struct Database {
    buffer_pool: Arc<BufferPool>,
    catalog: Catalog,
    free_mgr: FreePageManager,
    file_header: FileHeader,
    /// WAL 日志管理器（Slice B 中 page_ops 写日志要用）
    log_manager: Arc<LogManager>,
    txn_mgr: Arc<Mutex<TransactionManager>>,
}

impl Database {
    /// 打开或创建数据库（自动判断是新建还是打开已有）
    pub fn open() -> std::io::Result<Self> {
        let disk_manager = DiskManager::new()?;
        let is_new = disk_manager.is_empty()?;
        let log_manager = Arc::new(LogManager::new()?);

        if is_new {
            let mut dm = disk_manager;
            dm.preallocate(RESERVED_PAGES)?;
            let bp = Arc::new(BufferPool::new(dm, log_manager.clone()));
            Self::init_new(bp, log_manager)
        } else {
            let bp = Arc::new(BufferPool::new(disk_manager, log_manager.clone()));
            Self::load_existing(bp, log_manager)
        }
    }

    pub fn insert(&mut self, table: &mut Table, row: crate::row::Row) -> std::io::Result<crate::types::RowId> {
        table.insert_row(row, &mut self.free_mgr, &mut self.catalog, &self.txn_mgr)
    }

    /// 首次创建：初始化所有系统页
    fn init_new(buffer_pool: Arc<BufferPool>, log_manager: Arc<LogManager>) -> std::io::Result<Self> {
        let txn_mgr = Arc::new(Mutex::new(TransactionManager::new(log_manager.clone())));

        // Page 0: FileHeader
        let file_header = FileHeader::new();
        {
            let page0 = buffer_pool.fetch_page(0)?;
            let mut p = page0.write().unwrap();
            file_header.write_to_page(&mut p);
        }


        // Page 1: FreePageManager
        let mut free_mgr = FreePageManager::new(PAGE_SIZE as u64, MAX_PAGES);
        free_mgr.mark_used(0); // FileHeader
        free_mgr.flush(&buffer_pool)?;

        // Page 2: Catalog（空）
        let catalog = Catalog::new(buffer_pool.clone());

        Ok(Self { buffer_pool, catalog, free_mgr, file_header, log_manager, txn_mgr })
    }

    /// 从已有数据库文件恢复
    fn load_existing(buffer_pool: Arc<BufferPool>, log_manager: Arc<LogManager>) -> std::io::Result<Self> {
        let txn_mgr = Arc::new(Mutex::new(TransactionManager::new(log_manager.clone())));
        // Page 0: 读取 FileHeader
        let page0 = buffer_pool.fetch_page(0)?;
        let file_header = FileHeader::read_from_page(&page0.read().unwrap())?;

        // Page 1: 恢复 FreePageManager
        let mut free_mgr = FreePageManager::new(PAGE_SIZE as u64, MAX_PAGES);
        free_mgr.load(&buffer_pool)?;

        // Page 2+: 恢复 Catalog
        let catalog = Catalog::load(buffer_pool.clone())?;

        Ok(Self { buffer_pool, catalog, free_mgr, file_header, log_manager,txn_mgr })
    }

    // ==================== DDL ====================

    /// 创建新表
    pub fn create_table(&mut self, name: &str, columns: Vec<Column>) -> std::io::Result<Table> {
        let table = self.catalog.create_table(
            name.to_string(),
            columns,
            &mut self.free_mgr,
        )?;
        self.sync_file_header()?;
        Ok(table)
    }

    /// 删除表
    pub fn drop_table(&mut self, name: &str) -> std::io::Result<()> {
        self.catalog.drop_table(name, &mut self.free_mgr)?;
        self.sync_file_header()?;
        Ok(())
    }

    /// 获取一张可操作的表
    pub fn open_table(&self, name: &str) -> std::io::Result<Table> {
        self.catalog.open_table(name)
    }

    /// 列出所有表名
    pub fn list_tables(&self) -> Vec<&str> {
        self.catalog.list_tables()
    }

    /// 获取表的元数据（只读）
    pub fn get_table_meta(&self, name: &str) -> Option<&TableMeta> {
        self.catalog.get_table_meta(name)
    }

    // ==================== 内部同步 ====================

    /// 将全局统计信息同步到 FileHeader 并刷盘
    fn sync_file_header(&mut self) -> std::io::Result<()> {
        self.file_header.page_count = self.free_mgr.total_pages() as u32;
        self.file_header.free_page_count = self.free_mgr.count_free_pages();

        let page0 = self.buffer_pool.fetch_page(0)?;
        let mut p = page0.write().unwrap();
        self.file_header.write_to_page(&mut p);
        Ok(())
    }

    // ==================== 生命周期 ====================

    /// 关闭数据库：刷盘所有脏页与日志
    pub fn shutdown(&mut self) -> std::io::Result<()> {
        self.catalog.flush(&mut self.free_mgr)?;
        self.free_mgr.flush(&self.buffer_pool)?;
        self.sync_file_header()?;
        self.buffer_pool.flush()?;
        self.log_manager.flush()?;
        Ok(())
    }

    /// 获取内部组件引用（Table 的 insert/delete 需要用到）
    pub fn buffer_pool(&self) -> &Arc<BufferPool> { &self.buffer_pool }
    pub fn log_manager(&self) -> &Arc<LogManager> { &self.log_manager }
    pub fn catalog_mut(&mut self) -> &mut Catalog { &mut self.catalog }
    pub fn free_mgr_mut(&mut self) -> &mut FreePageManager { &mut self.free_mgr }
    pub fn txn_mgr(&self) -> &Arc<Mutex<TransactionManager>> { &self.txn_mgr }

}