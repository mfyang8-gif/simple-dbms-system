// src/storage/page_ops.rs
//
// 【模块职责】
// 页级 CRUD 操作：在单个数据页内插入、读取、删除、扫描记录。
// 类比 Java：相当于 DAO 层的单表操作，把 Page + Bitmap + PageHeader + Record 粘合在一起。
//
// 【数据区布局】
// 记录数据从偏移 DATA_OFFSET 开始，每个槽位固定 MAX_RECORD_SIZE 字节：
//   slot_offset(N) = DATA_OFFSET + N × MAX_RECORD_SIZE
// 其中 DATA_OFFSET = PAGE_HEADER_SIZE + BITMAP_BYTES_PER_PAGE = 4 + 1 = 5

use crate::config::{BITMAP_BYTES_PER_PAGE, MAX_RECORD_SIZE, SLOTS_PER_PAGE};
use crate::record::Record;
use crate::storage::bitmap::Bitmap;
use crate::storage::page::Page;
use crate::storage::page_header::{PageHeader, PAGE_HEADER_SIZE};
use crate::types::SlotId;

/// 记录数据区的起始偏移 = 页头(4) + 位图(1) = 5
const DATA_OFFSET: usize = PAGE_HEADER_SIZE + BITMAP_BYTES_PER_PAGE;

/// 计算指定槽位在页内的字节偏移量
fn slot_offset(slot: usize) -> usize {
    DATA_OFFSET + slot * MAX_RECORD_SIZE
}

/// 从页中读取位图
fn read_bitmap(page: &Page) -> Bitmap {
    let buf = page.read_data(PAGE_HEADER_SIZE, BITMAP_BYTES_PER_PAGE);
    Bitmap::from_bytes(buf, SLOTS_PER_PAGE)
}

/// 将位图写回页中
fn write_bitmap(page: &mut Page, bitmap: &Bitmap) {
    page.write_data(PAGE_HEADER_SIZE, bitmap.to_bytes());
}

// ==================== CRUD 操作 ====================

/// 向页中插入一条记录
///
/// 自动寻找第一个空闲槽位，写入序列化后的记录数据，
/// 并同步更新位图和页头。
///
/// # Returns
/// - `Some(slot_id)` — 插入成功，返回槽号
/// - `None` — 页已满，无空闲槽位
pub fn insert_record(page: &mut Page, record: &Record) -> Option<SlotId> {
    let mut bitmap = read_bitmap(page);
    let slot = bitmap.first_free()?;

    let offset = slot_offset(slot);
    let serialized = record.serialize();
    page.write_data(offset, &serialized);

    bitmap.set(slot);
    write_bitmap(page, &bitmap);

    let mut header = PageHeader::read_from_page(page);
    header.increment_occupied();
    header.write_to_page(page);

    Some(slot as SlotId)
}

/// 从页中读取指定槽位的记录
///
/// # Returns
/// - `Ok(record)` — 读取成功
/// - `Err` — 槽位为空或槽号越界
pub fn read_record(page: &Page, slot: SlotId) -> std::io::Result<Record> {
    let slot = slot as usize;
    if slot >= SLOTS_PER_PAGE {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("Slot {} out of range [0..{})", slot, SLOTS_PER_PAGE),
        ));
    }

    let bitmap = read_bitmap(page);
    if !bitmap.is_set(slot) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("Slot {} is empty", slot),
        ));
    }

    let offset = slot_offset(slot);
    let buf = page.read_data(offset, MAX_RECORD_SIZE);
    Record::deserialize(buf)
}

/// 删除页中指定槽位的记录
///
/// 清除位图标记并更新页头。槽位中的原始字节不做清除（下次插入时会覆盖）。
///
/// # Returns
/// - `Ok(())` — 删除成功
/// - `Err` — 槽位为空或槽号越界
pub fn delete_record(page: &mut Page, slot: SlotId) -> std::io::Result<()> {
    let slot = slot as usize;
    if slot >= SLOTS_PER_PAGE {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("Slot {} out of range [0..{})", slot, SLOTS_PER_PAGE),
        ));
    }

    let mut bitmap = read_bitmap(page);
    if !bitmap.is_set(slot) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("Slot {} is already empty", slot),
        ));
    }

    bitmap.clear(slot);
    write_bitmap(page, &bitmap);

    let mut header = PageHeader::read_from_page(page);
    header.decrement_occupied();
    header.write_to_page(page);

    Ok(())
}

/// 扫描页中所有已占用的记录
///
/// 返回 `(slot_id, record)` 的列表，按槽号升序。
pub fn scan_page(page: &Page) -> Vec<(SlotId, Record)> {
    let bitmap = read_bitmap(page);
    let mut records = Vec::new();

    for slot in 0..SLOTS_PER_PAGE {
        if bitmap.is_set(slot) {
            let offset = slot_offset(slot);
            let buf = page.read_data(offset, MAX_RECORD_SIZE);
            if let Ok(record) = Record::deserialize(buf) {
                records.push((slot as SlotId, record));
            }
        }
    }

    records
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_page() -> Page {
        let mut page = Page::new(1);
        let header = PageHeader::new();
        header.write_to_page(&mut page);
        let bitmap = Bitmap::new();
        write_bitmap(&mut page, &bitmap);
        page.is_dirty = false;
        page
    }

    #[test]
    fn test_insert_and_read() {
        let mut page = make_test_page();
        let record = Record::new(b"Hello, World!".to_vec()).unwrap();

        let slot = insert_record(&mut page, &record).unwrap();
        assert_eq!(slot, 0);

        let restored = read_record(&page, slot).unwrap();
        assert_eq!(restored.data(), record.data());
    }

    #[test]
    fn test_insert_multiple() {
        let mut page = make_test_page();

        for i in 0..SLOTS_PER_PAGE {
            let data = format!("record-{}", i).into_bytes();
            let record = Record::new(data).unwrap();
            let slot = insert_record(&mut page, &record).unwrap();
            assert_eq!(slot, i as SlotId);
        }

        let header = PageHeader::read_from_page(&page);
        assert_eq!(header.occupied_count, SLOTS_PER_PAGE as u16);

        // 页已满，再插入应该返回 None
        let extra = Record::new(b"overflow".to_vec()).unwrap();
        assert!(insert_record(&mut page, &extra).is_none());
    }

    #[test]
    fn test_delete_and_reinsert() {
        let mut page = make_test_page();

        let r1 = Record::new(b"first".to_vec()).unwrap();
        let r2 = Record::new(b"second".to_vec()).unwrap();
        let s1 = insert_record(&mut page, &r1).unwrap();
        let _s2 = insert_record(&mut page, &r2).unwrap();

        // 删除 slot 0
        delete_record(&mut page, s1).unwrap();
        assert!(read_record(&page, s1).is_err());

        let header = PageHeader::read_from_page(&page);
        assert_eq!(header.occupied_count, 1);

        // 重新插入，应该复用 slot 0
        let r3 = Record::new(b"third".to_vec()).unwrap();
        let s3 = insert_record(&mut page, &r3).unwrap();
        assert_eq!(s3, 0);

        let restored = read_record(&page, s3).unwrap();
        assert_eq!(restored.data(), r3.data());
    }

    #[test]
    fn test_scan_page() {
        let mut page = make_test_page();

        let r1 = Record::new(b"aaa".to_vec()).unwrap();
        let r2 = Record::new(b"bbb".to_vec()).unwrap();
        let r3 = Record::new(b"ccc".to_vec()).unwrap();
        insert_record(&mut page, &r1).unwrap();
        insert_record(&mut page, &r2).unwrap();
        insert_record(&mut page, &r3).unwrap();

        let results = scan_page(&page);
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].0, 0);
        assert_eq!(results[0].1.data(), b"aaa");
        assert_eq!(results[1].0, 1);
        assert_eq!(results[1].1.data(), b"bbb");
        assert_eq!(results[2].0, 2);
        assert_eq!(results[2].1.data(), b"ccc");
    }

    #[test]
    fn test_delete_nonexistent_slot() {
        let mut page = make_test_page();
        assert!(delete_record(&mut page, 3).is_err());
    }

    #[test]
    fn test_read_empty_slot() {
        let page = make_test_page();
        assert!(read_record(&page, 0).is_err());
    }
}