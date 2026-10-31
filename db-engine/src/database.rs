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
use std::collections::HashMap;
use std::path::PathBuf;

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
    /// 当前实例的 WAL 文件路径（ROLLBACK 的运行时 undo 要读它）
    log_path: String,

    /// 当前实例的 db 文件路径（CREATE DATABASE 按它算出新库的兄弟路径）
    db_path: String,
}

impl Database {
    /// 打开或创建数据库（使用全局默认路径）
    pub fn open() -> std::io::Result<Self> {
        Self::open_at(crate::config::DB_FILE, crate::config::LOG_FILE)
    }

    /// 打开或创建指定路径的数据库。
    ///
    /// 把 db 文件与 WAL 文件的路由从全局配置里抽出来：
    ///   - 测试可以用独立文件，不再删生产库；
    ///   - 以后做多库（CREATE DATABASE / USE）也只需要在这里分派路径。
    pub fn open_at(db_path: &str, log_path: &str) -> std::io::Result<Self> {
        let disk_manager = DiskManager::open_at(db_path)?;
        let is_new = disk_manager.is_empty()?;
        let log_manager = Arc::new(LogManager::from_path(log_path)?);

        if is_new {
            let mut dm = disk_manager;
            dm.preallocate(RESERVED_PAGES)?;
            let bp = Arc::new(BufferPool::new(dm, log_manager.clone()));
            Self::init_new(bp, log_manager, db_path, log_path)
        } else {
            let bp = Arc::new(BufferPool::new(disk_manager, log_manager.clone()));
            Self::load_existing(bp, log_manager, db_path, log_path)
        }
    }

    pub fn insert(&mut self, table: &mut Table, row: crate::row::Row) -> std::io::Result<crate::types::RowId> {
        self.insert_in_txn(table, row, None)
    }

    /// 事务感知插入：txn = Some(id) 表示加入调用方的显式事务
    pub fn insert_in_txn(&mut self, table: &mut Table, row: crate::row::Row, txn: Option<crate::recovery::log_record::TxnId>) -> std::io::Result<crate::types::RowId> {
        table.insert_row(row, &mut self.free_mgr, &mut self.catalog, &self.txn_mgr, txn)
    }
    /// 首次创建：初始化所有系统页
    fn init_new(buffer_pool: Arc<BufferPool>, log_manager: Arc<LogManager>, db_path: &str, log_path: &str) -> std::io::Result<Self> {
        let txn_mgr = Arc::new(Mutex::new(TransactionManager::new(log_manager.clone())));

        // Page 0: FileHeader
        let file_header = FileHeader::new();
        {
            let page0 = buffer_pool.fetch_page(0)?;
            let mut p = page0.write().unwrap();
            file_header.write_to_page(&mut p);
        }


        // Page 1: FreePageManager（位图自己就在 Page 1，必须保留）
        let mut free_mgr = FreePageManager::new(PAGE_SIZE as u64, MAX_PAGES);
        free_mgr.mark_used(0); // FileHeader
        free_mgr.mark_used(1); // FreePageManager 位图页
        free_mgr.mark_used(2); // Catalog 首页
        let catalog = Catalog::new(buffer_pool.clone());

        // ★ 初始化必须立即落盘：
        //   preallocate 已经把文件扩成 3 个零页，而 FileHeader 目前只在 buffer pool 内存里。
        //   若在 shutdown 之前退出，下次 open 会看到“文件非空 + magic=0”，被误判成已有库。
        buffer_pool.flush()?;

        Ok(Self { buffer_pool, catalog, free_mgr, file_header, log_manager, txn_mgr, log_path: log_path.to_string(), db_path: db_path.to_string() })
    }

    /// 从已有数据库文件恢复
    fn load_existing(buffer_pool: Arc<BufferPool>, log_manager: Arc<LogManager>, db_path: &str, log_path: &str) -> std::io::Result<Self> {
        let txn_mgr = Arc::new(Mutex::new(TransactionManager::new(log_manager.clone())));
        // Page 0: 读取 FileHeader
        let page0 = buffer_pool.fetch_page(0)?;
        let file_header = FileHeader::read_from_page(&page0.read().unwrap())?;

        // Page 1: FreePageManager 先占位，等 Catalog 恢复完再整体重建（位图是派生状态）
        let mut free_mgr = FreePageManager::new(PAGE_SIZE as u64, MAX_PAGES);

        // 先 redo/undo：把所有已提交的页修改重放进 buffer pool
        crate::recovery::restore::restore(log_path, &buffer_pool, &log_manager)?;

        // Page 2+: 从（可能已被 redo 修正过的）页里恢复 Catalog
        let catalog = Catalog::load(buffer_pool.clone())?;

        // 位图不信任磁盘旧值：以 Catalog 为权威重建，顺手回收崩溃留下的孤儿页
        catalog.rebuild_free_bitmap(&mut free_mgr);
        free_mgr.flush(&buffer_pool)?;

        Ok(Self { buffer_pool, catalog, free_mgr, file_header, log_manager, txn_mgr, log_path: log_path.to_string(), db_path: db_path.to_string() })
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
    /// 当前实例的 WAL 路径（测试隔离后不能再用全局 config::LOG_FILE）
    pub fn log_path(&self) -> &str { &self.log_path }
    /// 当前实例的 db 文件路径（多库时 CREATE DATABASE 用它定位同目录）
    pub fn db_path(&self) -> &str { &self.db_path }
    pub fn catalog_mut(&mut self) -> &mut Catalog { &mut self.catalog }
    pub fn free_mgr_mut(&mut self) -> &mut FreePageManager { &mut self.free_mgr }
    pub fn txn_mgr(&self) -> &Arc<Mutex<TransactionManager>> { &self.txn_mgr }

}
// ==================== 多库注册表 ====================

/// 进程内的库集合：默认库常驻，其它库按需打开并缓存。
/// 默认库同时充当「实例级账号库」（__minidb_users 只有一份，在这里）。
pub struct DbRegistry {
    root: PathBuf,                  // 库文件所在目录（从默认库的路径推出）
    default_name: String,           // 默认库名（文件名去掉扩展名）
    default_db: Database,
    others: HashMap<String, Database>,
}

impl DbRegistry {
    pub fn new(default_db: Database) -> Self {
        let p = PathBuf::from(default_db.db_path());
        let root = p.parent()
            .map(|x| x.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
        let default_name = p.file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "main".to_string());
        Self { root, default_name, default_db, others: HashMap::new() }
    }

    /// 按会话的 current_db 取「当前库」和「账号库」。
    ///   - current = 默认库 → accounts = None（同一个对象，不需要第二个写入口）
    ///   - current = 其它库 → accounts = Some(默认库)
    pub fn resolve(
        &mut self,
        name: Option<&str>,
    ) -> Result<(&mut Database, Option<&mut Database>), String> {
        let name = match name {
            None => return Ok((&mut self.default_db, None)),
            Some(n) => n,
        };
        if name == self.default_name {
            return Ok((&mut self.default_db, None));
        }

        self.ensure_open(name)?;

        let cur = self.others.get_mut(name)
            .ok_or_else(|| format!("database '{}' does not exist", name))?;
        // 两个不相交的字段，可以同时可变借用
        Ok((cur, Some(&mut self.default_db)))
    }

    /// 懒加载：第一次访问某个库时才 open（会跑一遍崩溃恢复）
    fn ensure_open(&mut self, name: &str) -> Result<(), String> {
        if self.others.contains_key(name) {
            return Ok(());
        }
        let db_file = self.root.join(format!("{}.db", name));
        let log_file = self.root.join(format!("{}.log", name));

        // 存在性判断用 exists()，绝不用 open_at 试探（它会创建）
        if !db_file.exists() {
            return Err(format!("database '{}' does not exist", name));
        }

        let db_s = db_file.to_string_lossy().to_string();
        let log_s = log_file.to_string_lossy().to_string();
        let db = Database::open_at(&db_s, &log_s)
            .map_err(|e| format!("open database '{}': {}", name, e))?;
        self.others.insert(name.to_string(), db);
        Ok(())
    }

    /// 退出时统一刷盘
    pub fn shutdown(&mut self) {
        for db in self.others.values_mut() {
            let _ = db.shutdown();
        }
        let _ = self.default_db.shutdown();
    }
}