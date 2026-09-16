// src/storage/mod.rs
//
// 【模块职责】
// 存储引擎层的入口，声明所有子模块。
// 类比 Java：类似于 storage 包的 package-info.java，只负责组织子包。

pub mod bitmap;
pub mod buffer_pool;
pub mod disk_manager;
pub mod file_header;
pub mod page;
pub mod page_header;
pub mod free_page_manager;
pub(crate) mod page_ops;

