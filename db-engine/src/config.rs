// src/config.rs
//
// 【模块职责】
// 全局常量配置，所有魔法数字的唯一来源。
// 类比 Java：相当于 application.yml 或 Constants.java。
// 修改任何配置值只需要改这一个文件。

// ========== 存储引擎配置 ==========

/// 页大小：数据库读写磁盘的最小单位 (4KB)
pub const PAGE_SIZE: usize = 4096;

/// 缓冲池最大页数
pub const BUFFER_POOL_SIZE: usize = 64;

// ========== 文件路径配置 ==========

/// 数据库文件存放目录
pub const DB_DIR: &str = "./data";

/// 主数据文件
pub const DB_FILE: &str = "./data/db.db";

// ========== 网络配置 ==========

/// 服务监听地址
pub const LISTEN_ADDR: &str = "127.0.0.1:5432";

// ========== 记录格式配置 ==========

/// 单条记录最大字节数
/// personnel 表最宽：INT(4)+VARCHAR(50)+VARCHAR(10)+VARCHAR(20)+VARCHAR(20)
///                   +VARCHAR(50)+VARCHAR(100)+VARCHAR(20)+VARCHAR(64)+INT(4) ≈ 342
/// 向上取整到 512 字节留余量
pub const MAX_RECORD_SIZE: usize = 512;

/// 每页数据页的页头大小：4 字节（slot_count: u16 + occupied_count: u16）
pub const PAGE_HEADER_SIZE: usize = 4;

/// 每页可容纳的记录槽位数：(4096 - 4) / 512 = 7
pub const SLOTS_PER_PAGE: usize = (PAGE_SIZE - PAGE_HEADER_SIZE) / MAX_RECORD_SIZE;

/// 位图所需字节数：7 个槽位需要 7 bit → 向上取整 = 1 字节
pub const BITMAP_BYTES_PER_PAGE: usize = (SLOTS_PER_PAGE + 7) / 8;

/// 文件头固定大小（字节数）- 40 字节
pub const FILE_HEADER_SIZE: usize = 40;