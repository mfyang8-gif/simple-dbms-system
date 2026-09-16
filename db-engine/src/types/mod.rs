// src/types/mod.rs
//
// 【模块职责】
// 定义整个引擎共用的基础类型。
// 类比 Java：相当于 common/model 包下的 BaseEntity、RowId 等公共类。
// 任何模块需要用到这些类型时，通过 `use crate::types::*` 引入。

/// 页号：标识一个页的唯一 ID，本质上是从 0 开始的序号
pub type PageId = u32;

/// 槽号：一个页内可以存多行数据，槽号标识页内的第几行
pub type SlotId = u16;

/// 行 ID：全局定位一行数据 = (在哪个页, 页内第几个槽)
///
/// 类比 Java：类似于一个复合主键 `(page_id, slot_id)`
/// 当上层说"帮我查 RowId(2, 3)"，引擎就知道去第 2 页第 3 个槽位取数据
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RowId {
    pub page_id: PageId,
    pub slot_id: SlotId,
}

impl RowId {
    pub fn new(page_id: PageId, slot_id: SlotId) -> Self {
        Self { page_id, slot_id }
    }
}

impl std::fmt::Display for RowId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RowId(page={}, slot={})", self.page_id, self.slot_id)
    }
}