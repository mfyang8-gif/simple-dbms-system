// src/storage/buffer_pool.rs
//
// 【模块职责】
// 在内存中缓存磁盘页，减少物理 IO 次数。
//
// 【并发设计】
// 每个 Page 用 Arc<RwLock<Page>> 包装，实现页级读写锁。
// BufferPool 自身所有方法均为 &self，可被多线程共享（需 Arc<BufferPool>）。
//
// 【WAL 集成】
// BufferPool 是内存页落盘的唯一咽喉（flush / evict 两条路径）。
// 因此"日志先行"铁律在此强制：脏页写盘前，必须先 flush_to(pageLSN)。
//
// 【淘汰策略 — Clock 算法】
// 缓冲池固定大小 BUFFER_POOL_SIZE，满时用二次机会 Clock 淘汰。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use crate::config::BUFFER_POOL_SIZE;
use crate::recovery::log_manager::LogManager;
use crate::storage::disk_manager::DiskManager;
use crate::storage::page::Page;
use crate::storage::page_header::PageHeader;

/// 被 Arc<RwLock> 包装的页引用类型
pub type SharedPage = Arc<RwLock<Page>>;

/// Clock 算法的环形数组条目
struct ClockEntry {
    page_id: u32,
    page: SharedPage,
}

/// 缓冲池：管理内存中的页缓存（线程安全，固定大小，Clock 淘汰）
pub struct BufferPool {
    pages: Mutex<HashMap<u32, SharedPage>>,
    clock: Mutex<Vec<ClockEntry>>,
    clock_hand: Mutex<usize>,
    disk_manager: Mutex<DiskManager>,
    /// WAL 日志管理器：脏页落盘前强制"日志先行"
    log_manager: Arc<LogManager>,
}

impl BufferPool {
    pub fn new(disk_manager: DiskManager, log_manager: Arc<LogManager>) -> Self {
        Self {
            pages: Mutex::new(HashMap::new()),
            clock: Mutex::new(Vec::with_capacity(BUFFER_POOL_SIZE)),
            clock_hand: Mutex::new(0),
            disk_manager: Mutex::new(disk_manager),
            log_manager,
        }
    }

    /// 暴露日志管理器，供上层（Table/page_ops）构造 WalContext 写日志
    pub fn log_manager(&self) -> &Arc<LogManager> {
        &self.log_manager
    }

    /// 获取指定页的共享引用（线程安全）
    pub fn fetch_page(&self, page_id: u32) -> std::io::Result<SharedPage> {
        {
            let pages = self.pages.lock().unwrap();
            if let Some(page) = pages.get(&page_id) {
                let shared = Arc::clone(page);
                page.write().unwrap().reference_bit = true;
                return Ok(shared);
            }
        }
        self.load_page_from_disk(page_id)?;
        let pages = self.pages.lock().unwrap();
        let page = pages.get(&page_id).unwrap();
        let shared = Arc::clone(page);
        page.write().unwrap().reference_bit = true;
        Ok(shared)
    }

    /// 将所有脏页刷回磁盘（落盘前强制日志先行）
    pub fn flush(&self) -> std::io::Result<()> {
        let pages = self.pages.lock().unwrap();
        let mut dm = self.disk_manager.lock().unwrap();
        for page_ref in pages.values() {
            let mut page = page_ref.write().unwrap();
            if page.is_dirty {
                self.wal_flush_before_write(&page)?;
                dm.write_page(page.id, &page.data)?;
                page.is_dirty = false;
            }
        }
        Ok(())
    }

    /// 从磁盘加载指定页到缓存
    fn load_page_from_disk(&self, page_id: u32) -> std::io::Result<()> {
        let mut pages = self.pages.lock().unwrap();
        let mut clock = self.clock.lock().unwrap();

        if pages.contains_key(&page_id) {
            return Ok(());
        }

        if pages.len() >= BUFFER_POOL_SIZE {
            self.evict_one(&mut pages, &mut clock)?;
        }

        let mut page = Page::new(page_id);
        {
            let mut dm = self.disk_manager.lock().unwrap();
            dm.read_page(page_id, &mut page.data)?;
        }
        page.reference_bit = true;

        let shared_page = Arc::new(RwLock::new(page));

        let hand = *self.clock_hand.lock().unwrap();
        if clock.len() < BUFFER_POOL_SIZE {
            clock.push(ClockEntry { page_id, page: shared_page.clone() });
        } else {
            clock[hand] = ClockEntry { page_id, page: shared_page.clone() };
            *self.clock_hand.lock().unwrap() = (hand + 1) % BUFFER_POOL_SIZE;
        }

        pages.insert(page_id, shared_page);
        Ok(())
    }

    /// Clock 淘汰算法：扫描环形数组找牺牲页
    fn evict_one(
        &self,
        pages: &mut HashMap<u32, SharedPage>,
        clock: &mut Vec<ClockEntry>,
    ) -> std::io::Result<()> {
        let mut hand = *self.clock_hand.lock().unwrap();
        let len = clock.len();

        for _ in 0..len * 2 {
            let entry = &clock[hand];

            //"baseline=2(pages+clock 各一份)
            if Arc::strong_count(&entry.page) > 2 {
                hand = (hand + 1) % len;
                continue;
            }

            let mut page = entry.page.write().unwrap();

            if page.reference_bit {
                page.reference_bit = false;
                drop(page);
                hand = (hand + 1) % len;
                continue;
            }

            // 找到了牺牲页！如果是脏页，先强制日志落盘，再写页
            if page.is_dirty {
                self.wal_flush_before_write(&page)?;
                let mut dm = self.disk_manager.lock().unwrap();
                dm.write_page(page.id, &page.data)?;
                page.is_dirty = false;
            }
            let victim_id = page.id;
            drop(page);

            pages.remove(&victim_id);
            *self.clock_hand.lock().unwrap() = hand;
            return Ok(());
        }

        Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            "Clock eviction failed: all pages are pinned",
        ))
    }

    /// WAL 检查：若该页 pageLSN 是日志分配过的（<= curr_lsn），
    /// 则强制把日志 fsync 到 pageLSN，满足"日志先行"。
    /// 对未记日志的页（pageLSN=0）或非 Slotted 页（Page0/Page1 的 [8..16) 垃圾值 > curr_lsn）安全跳过。
    fn wal_flush_before_write(&self, page: &Page) -> std::io::Result<()> {
        let page_lsn = PageHeader::read_page_lsn(page);
        if page_lsn <= self.log_manager.curr_lsn() {
            self.log_manager.flush_to(page_lsn)?;
        }
        Ok(())
    }
}