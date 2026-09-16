use std::sync::Arc;

use db_engine::config::*;
use db_engine::storage::bitmap::Bitmap;
use db_engine::storage::buffer_pool::BufferPool;
use db_engine::storage::disk_manager::DiskManager;
use db_engine::storage::file_header::{FileHeader, FILE_HEADER_VERSION};
use db_engine::storage::free_page_manager::FreePageManager;
use db_engine::storage::page_header::PageHeader;

fn main() {
    println!("=== Mini DBMS Engine v{} ===", FILE_HEADER_VERSION);

    // 1. 初始化磁盘管理器
    let mut disk_manager = DiskManager::new().expect("[FATAL] Failed to initialize DiskManager");
    println!("[OK] DiskManager initialized, db file: {}", DB_FILE);

    // 首次运行检测
    if disk_manager.is_empty().expect("[FATAL] Failed to check file status") {
        disk_manager.preallocate(2).expect("[FATAL] Failed to preallocate");
        println!("[OK] New database file created, pre-allocated 2 pages (8192 bytes)");
    } else {
        println!("[OK] Existing database file found");
    }

    // 2. 初始化缓冲池（用 Arc 包装，支持多线程共享）
    let buffer_pool = Arc::new(BufferPool::new(disk_manager));
    println!("[OK] BufferPool initialized, max pages: {}", BUFFER_POOL_SIZE);

    // 3. 创建文件头并写入第 0 页
    let header = FileHeader::new();
    println!("[OK] {}", header);

    let page0_ref = buffer_pool
        .fetch_page(0)
        .expect("[FATAL] Failed to fetch page 0");
    {
        let mut page0 = page0_ref.write().unwrap();
        header.write_to_page(&mut page0);
    }

    // 4. 初始化第 1 页：写页头 + 位图
    let page1_ref = buffer_pool
        .fetch_page(1)
        .expect("[FATAL] Failed to fetch page 1");

    {
        let mut page1 = page1_ref.write().unwrap();

        // 4a. 写页头
        let page_header = PageHeader::new();
        page_header.write_to_page(&mut page1);
        println!("[OK] PageHeader written: slots={}, occupied={}",
                 page_header.slot_count, page_header.occupied_count);

        // 4b. 写位图
        let bitmap = Bitmap::new();
        page1.write_data(4, bitmap.to_bytes());
        println!("[OK] {} (initial state)", bitmap);
    }

    // 4c. 测试：标记几个槽位为已占用
    let mut bm_test = Bitmap::new();
    bm_test.set(0);
    bm_test.set(2);
    bm_test.set(5);
    println!("[OK] {} (after setting slots 0,2,5)", bm_test);

    let free_slot = bm_test.first_free().expect("Should have free slot");
    println!("[OK] First free slot: {}", free_slot);

    // 5. 测试 FreePageManager（通过 BufferPool 读写，不再直接操作磁盘）
    let mut free_mgr = FreePageManager::new(PAGE_SIZE as u64, 1024);
    free_mgr.load(&buffer_pool).expect("[FATAL] Failed to load free page bitmap");
    free_mgr.mark_used(0);
    free_mgr.mark_used(1);
    println!("[OK] FreePageManager: free_pages={}, first_free={:?}",
             free_mgr.count_free_pages(), free_mgr.find_free_page());
    free_mgr.flush(&buffer_pool).expect("[FATAL] Failed to flush free page bitmap");

    // 6. 刷盘
    buffer_pool.flush().expect("[FATAL] Failed to flush");
    println!("[OK] All dirty pages flushed to disk");

    println!("\n=== Engine verification complete ===");
}