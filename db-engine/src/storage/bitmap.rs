// src/storage/bitmap.rs
//
// 【模块职责】
// 管理数据页内记录槽位的占用状态，像停车场的"空位指示灯"。
// 类比 Java：类似于一个 BitSet，但需要手动做字节级的序列化。
//
// 【核心概念】
// 每个数据页有 SLOTS_PER_PAGE 个槽位（当前配置为 7 个）。
// 位图用 1 个比特表示一个槽位的状态：
//   0 = 空闲（可插入新记录）
//   1 = 已占用（该槽位有数据，或被删除但未回收）
//
// 【为什么用位图而不是数组？】
// 7 个槽位如果用 u32 数组存状态：7 × 4 = 28 字节
// 用位图：只需 1 字节 (7 bit → 向上取整 = 1 byte)
// 位运算速度极快，CPU 一条指令就能完成 AND/OR/NOT 操作。
//
// 【在页中的存放位置】
// ┌─ Page 数据区 (4096 bytes) ──────────────────────────┐
// │ [0..4)   页头：slot_count(u16) + occupied_count(u16)│
// │ [4..5)   位图数据（1 字节管理 7 个槽位）              │
// │ [5..5+512)  槽位 0 的记录数据                        │
// │ [517..1029) 槽位 1 的记录数据                        │
// │ ...                                                 │
// └─────────────────────────────────────────────────────┘

use crate::config::{BITMAP_BYTES_PER_PAGE, SLOTS_PER_PAGE};

/// 页内位图：用比特位管理每个槽位的占用状态
#[derive(Debug, Clone)]
pub struct Bitmap {
    /// 位图的底层字节存储
    bits: Vec<u8>,
    /// 该位图管理的槽位总数
    capacity: usize,
}

impl Bitmap {
    /// 创建一个全新的空位图（所有槽位均为空闲状态）
    pub fn new() -> Self {
        Self {
            bits: vec![0u8; BITMAP_BYTES_PER_PAGE],
            capacity: SLOTS_PER_PAGE,
        }
    }

    /// 从原始字节恢复位图（从磁盘页中读取后使用）
    pub fn from_bytes(data: &[u8], capacity: usize) -> Self {
        Self {
            bits: data.to_vec(),
            capacity,
        }
    }

    /// 将槽位标记为已占用
    ///
    /// # Panics
    /// 当 `slot >= capacity` 时 panic
    pub fn set(&mut self, slot: usize) {
        assert!(slot < self.capacity, "slot {} out of range [0..{})", slot, self.capacity);
        self.bits[slot / 8] |= 1 << (slot % 8);
    }

    /// 将槽位标记为空闲
    pub fn clear(&mut self, slot: usize) {
        assert!(slot < self.capacity, "slot {} out of range [0..{})", slot, self.capacity);
        self.bits[slot / 8] &= !(1 << (slot % 8));
    }

    /// 检查指定槽位是否被占用
    pub fn is_set(&self, slot: usize) -> bool {
        assert!(slot < self.capacity, "slot {} out of range [0..{})", slot, self.capacity);
        (self.bits[slot / 8] >> (slot % 8)) & 1 == 1
    }

    /// 查找第一个空闲槽位，返回其索引
    ///
    /// 如果没有空闲槽位，返回 None。
    /// 插入新记录时调用此方法来决定写到哪里。
    pub fn first_free(&self) -> Option<usize> {
        (0..self.capacity).find(|&i| !self.is_set(i))
    }

    /// 当前已占用的槽位数
    pub fn occupied_count(&self) -> usize {
        (0..self.capacity).filter(|&i| self.is_set(i)).count()
    }

    /// 当前空闲的槽位数
    pub fn free_count(&self) -> usize {
        self.capacity - self.occupied_count()
    }

    /// 是否已满（所有槽位都被占用）
    pub fn is_full(&self) -> bool {
        self.free_count() == 0
    }

    /// 序列化为字节数组（写回磁盘用）
    pub fn to_bytes(&self) -> &[u8] {
        &self.bits
    }

    /// 位图管理的槽位总数
    pub fn capacity(&self) -> usize {
        self.capacity
    }
}

impl Default for Bitmap {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for Bitmap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let slots: String = (0..self.capacity)
            .map(|i| if self.is_set(i) { '1' } else { '0' })
            .collect();
        write!(
            f,
            "Bitmap[{}/{} free]: {}",
            self.free_count(),
            self.capacity,
            slots
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_bitmap_all_free() {
        let bm = Bitmap::new();
        assert_eq!(bm.free_count(), SLOTS_PER_PAGE);
        assert_eq!(bm.occupied_count(), 0);
        assert!(!bm.is_full());
    }

    #[test]
    fn test_set_and_clear() {
        let mut bm = Bitmap::new();

        bm.set(0);
        bm.set(3);
        assert!(bm.is_set(0));
        assert!(!bm.is_set(1));
        assert!(bm.is_set(3));
        assert_eq!(bm.occupied_count(), 2);

        bm.clear(0);
        assert!(!bm.is_set(0));
        assert_eq!(bm.occupied_count(), 1);
    }

    #[test]
    fn test_first_free() {
        let mut bm = Bitmap::new();
        assert_eq!(bm.first_free(), Some(0));

        bm.set(0);
        bm.set(1);
        assert_eq!(bm.first_free(), Some(2));

        for i in 0..SLOTS_PER_PAGE {
            bm.set(i);
        }
        assert_eq!(bm.first_free(), None);
        assert!(bm.is_full());
    }

    #[test]
    fn test_serialize_deserialize_roundtrip() {
        let mut bm = Bitmap::new();
        bm.set(1);
        bm.set(4);
        bm.set(6);

        let bytes = bm.to_bytes().to_vec();
        let restored = Bitmap::from_bytes(&bytes, SLOTS_PER_PAGE);

        assert!(!restored.is_set(0));
        assert!(restored.is_set(1));
        assert!(!restored.is_set(2));
        assert!(!restored.is_set(3));
        assert!(restored.is_set(4));
        assert!(!restored.is_set(5));
        assert!(restored.is_set(6));
    }

    #[test]
    #[should_panic(expected = "out of range")]
    fn test_out_of_range_panics() {
        let bm = Bitmap::new();
        bm.is_set(SLOTS_PER_PAGE + 1);
    }
}