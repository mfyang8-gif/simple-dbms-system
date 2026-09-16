// src/storage/buffer_pool.rs
//
// 【模块职责】
// 在内存中缓存磁盘页，减少物理 IO 次数。
// 类比 Java：类似于 MyBatis 的一级缓存 / 数据库连接池的思路。
// 把频繁使用的页缓存在内存里，避免每次都访问磁盘。
//
// 【并发设计】
// 每个 Page 用 Arc<RwLock<Page>> 包装，实现页级读写锁：
// - 线程 A 修改 Page 1 时，线程 B 仍可读取 Page 2
// - 同一页支持多个读者并发，写者独占
// BufferPool 自身所有方法均为 &self，可被多线程共享（需 Arc<BufferPool>）
//
// 【淘汰策略 — Clock 算法】
// 缓冲池固定大小为 BUFFER_POOL_SIZE（64 页）。
// 当缓存满时，用 Clock（时钟/二次机会）算法淘汰最久未使用的页：
// - 维护一个环形数组 + 一个"时钟指针"
// - 每次访问页 → 设置 reference_bit = true
// - 需要淘汰时 → 指针扫一圈：
//     遇到 true → 清为 false，跳过（给它第二次机会）
//     遇到 false → 就是它了，刷盘后淘汰

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use crate::config::BUFFER_POOL_SIZE;
use crate::storage::disk_manager::DiskManager;
use crate::storage::page::Page;

/// 被 Arc<RwLock> 包装的页引用类型
pub type SharedPage = Arc<RwLock<Page>>;

/// Clock 算法的环形数组条目
struct ClockEntry {
    page_id: u32,
    page: SharedPage,
}

/// 缓冲池：管理内存中的页缓存（线程安全，固定大小，Clock 淘汰）
pub struct BufferPool {
    /// 快速查找表：page_id → SharedPage
    pages: Mutex<HashMap<u32, SharedPage>>,

    /// Clock 算法的环形数组（固定大小 = BUFFER_POOL_SIZE）
    clock: Mutex<Vec<ClockEntry>>,

    /// Clock 算法的指针（当前扫描位置）
    clock_hand: Mutex<usize>,

    /// 磁盘管理器：缓存未命中时，需要通过它去磁盘加载
    disk_manager: Mutex<DiskManager>,
}

impl BufferPool {
    pub fn new(disk_manager: DiskManager) -> Self {
        Self {
            pages: Mutex::new(HashMap::new()),
            clock: Mutex::new(Vec::with_capacity(BUFFER_POOL_SIZE)),
            clock_hand: Mutex::new(0),
            disk_manager: Mutex::new(disk_manager),
        }
    }

    /// 获取指定页的共享引用（线程安全）
    ///
    /// 返回 Arc<RwLock<Page>> 的克隆，上层通过 .read() / .write() 获取读写锁。
    /// 如果页不在缓存中，会先从磁盘加载（缓存满时自动淘汰一个旧页）。
    /// 每次访问都会设置 reference_bit = true（给这个页"续命"）。
    pub fn fetch_page(&self, page_id: u32) -> std::io::Result<SharedPage> {
        {
            let pages = self.pages.lock().unwrap();
            if let Some(page) = pages.get(&page_id) {
                let shared = Arc::clone(page);
                // 命中缓存 → 设置引用位（"我来过了，别淘汰我"）
                page.write().unwrap().reference_bit = true;
                return Ok(shared);
            }
        }

        // 缓存未命中 → 加载（可能触发淘汰）
        self.load_page_from_disk(page_id)?;

        let pages = self.pages.lock().unwrap();
        let page = pages.get(&page_id).unwrap();
        let shared = Arc::clone(page);
        page.write().unwrap().reference_bit = true;
        Ok(shared)
    }

    /// 将所有脏页刷回磁盘，确保数据持久化
    pub fn flush(&self) -> std::io::Result<()> {
        let pages = self.pages.lock().unwrap();
        let mut dm = self.disk_manager.lock().unwrap();
        for page_ref in pages.values() {
            let mut page = page_ref.write().unwrap();
            if page.is_dirty {
                dm.write_page(page.id, &page.data)?;
                page.is_dirty = false;
            }
        }
        Ok(())
    }

    /// 从磁盘加载指定页到缓存
    ///
    /// 如果缓存已满（达到 BUFFER_POOL_SIZE），先用 Clock 算法淘汰一个页，
    /// 再加载新页。
    fn load_page_from_disk(&self, page_id: u32) -> std::io::Result<()> {
        let mut pages = self.pages.lock().unwrap();
        let mut clock = self.clock.lock().unwrap();

        // 双重检查：可能在等锁期间被其他线程加载进来了
        if pages.contains_key(&page_id) {
            return Ok(());
        }

        // 缓存已满 → 淘汰一个页腾出空间
        if pages.len() >= BUFFER_POOL_SIZE {
            self.evict_one(&mut pages, &mut clock)?;
        }

        // 从磁盘读取新页
        let mut page = Page::new(page_id);
        {
            let mut dm = self.disk_manager.lock().unwrap();
            dm.read_page(page_id, &mut page.data)?;
        }
        page.reference_bit = true;

        let shared_page = Arc::new(RwLock::new(page));

        // 插入 Clock 环形数组
        let hand = *self.clock_hand.lock().unwrap();
        if clock.len() < BUFFER_POOL_SIZE {
            // 还没填满 → 直接追加
            clock.push(ClockEntry { page_id, page: shared_page.clone() });
        } else {
            // 已填满 → 替换被淘汰的位置（evict_one 已经清理了 HashMap，
            // 但 Clock 槽位还留着旧条目，这里覆盖它）
            clock[hand] = ClockEntry { page_id, page: shared_page.clone() };
            // 指针后移（环形）
            *self.clock_hand.lock().unwrap() = (hand + 1) % BUFFER_POOL_SIZE;
        }

        pages.insert(page_id, shared_page);
        Ok(())
    }

    /// Clock 淘汰算法核心：扫描环形数组，找到一个牺牲页
    ///
    /// 算法流程（像时钟指针一样扫）：
    ///   1. 看当前指针位置的页
    ///   2. reference_bit = true → 清为 false，指针后移（给它第二次机会）
    ///   3. reference_bit = false → 就是它！淘汰
    ///   4. 额外安全检查：如果 Arc 引用计数 > 1（有人正在用这页），跳过
    fn evict_one(
        &self,
        pages: &mut HashMap<u32, SharedPage>,
        clock: &mut Vec<ClockEntry>,
    ) -> std::io::Result<()> {
        let mut hand = *self.clock_hand.lock().unwrap();
        let len = clock.len();

        // 最多扫两圈（2 × len），第一圈清引用位，第二圈必定找到
        for _ in 0..len * 2 {
            let entry = &clock[hand];

            // 安全检查：Arc 引用计数 > 1 说明有人在用这个页，不能淘汰
            // （HashMap 持有 1 + 外部 Arc 克隆 ≥ 2）
            if Arc::strong_count(&entry.page) > 1 {
                hand = (hand + 1) % len;
                continue;
            }

            let mut page = entry.page.write().unwrap();

            if page.reference_bit {
                // 最近被访问过 → 给第二次机会
                page.reference_bit = false;
                drop(page); // 释放页写锁
                hand = (hand + 1) % len;
                continue;
            }

            // 找到了牺牲页！如果是脏页，先刷盘
            if page.is_dirty {
                let mut dm = self.disk_manager.lock().unwrap();
                dm.write_page(page.id, &page.data)?;
                page.is_dirty = false;
            }
            let victim_id = page.id;
            drop(page); // 释放页写锁

            // 从 HashMap 移除（clock 槽位留给 load_page_from_disk 覆盖）
            pages.remove(&victim_id);

            // 更新指针位置
            *self.clock_hand.lock().unwrap() = hand;
            return Ok(());
        }

        // 理论上不会到这里（最多扫两圈必能找到）
        Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            "Clock eviction failed: all pages are pinned",
        ))
    }
}