// src/storage/file_header.rs

use crate::config::{FILE_HEADER_SIZE, PAGE_SIZE};

pub const FILE_HEADER_MAGIC: u32 = 0x4442_4D53;
pub const FILE_HEADER_VERSION: u32 = 1;

#[derive(Debug, Clone)]
pub struct FileHeader {
    pub magic: u32,
    pub version: u32,
    pub page_count: u32,
    pub record_count: u32,
    pub free_page_count: u32,
    pub first_data_page: u32,
    pub free_page_bitmap_offset: u64,
    pub free_page_bitmap_size: u32,
    pub max_pages: u32,
}

impl FileHeader {
    pub fn new() -> Self {
        let max_pages = 1024;
        let bitmap_size = (max_pages + 7) / 8;

        Self {
            magic: FILE_HEADER_MAGIC,
            version: FILE_HEADER_VERSION,
            page_count: 1,
            record_count: 0,
            free_page_count: 0,
            first_data_page: 3,
            free_page_bitmap_offset: PAGE_SIZE as u64,
            free_page_bitmap_size: bitmap_size as u32,
            max_pages,
        }
    }

    pub fn serialize(&self) -> [u8; FILE_HEADER_SIZE] {
        let mut buf = [0u8; FILE_HEADER_SIZE];
        buf[0..4].copy_from_slice(&self.magic.to_le_bytes());
        buf[4..8].copy_from_slice(&self.version.to_le_bytes());
        buf[8..12].copy_from_slice(&self.page_count.to_le_bytes());
        buf[12..16].copy_from_slice(&self.record_count.to_le_bytes());
        buf[16..20].copy_from_slice(&self.free_page_count.to_le_bytes());
        buf[20..24].copy_from_slice(&self.first_data_page.to_le_bytes());
        buf[24..32].copy_from_slice(&self.free_page_bitmap_offset.to_le_bytes());
        buf[32..36].copy_from_slice(&self.free_page_bitmap_size.to_le_bytes());
        buf[36..40].copy_from_slice(&self.max_pages.to_le_bytes());
        buf
    }

    pub fn deserialize(buf: &[u8]) -> std::io::Result<Self> {
        if buf.len() < FILE_HEADER_SIZE {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Buffer too small: {} < {}", buf.len(), FILE_HEADER_SIZE),
            ));
        }

        let magic = u32::from_le_bytes(buf[0..4].try_into().unwrap());
        if magic != FILE_HEADER_MAGIC {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "Invalid magic: expected 0x{:08X}, got 0x{:08X}",
                    FILE_HEADER_MAGIC, magic
                ),
            ));
        }

        Ok(Self {
            magic,
            version: u32::from_le_bytes(buf[4..8].try_into().unwrap()),
            page_count: u32::from_le_bytes(buf[8..12].try_into().unwrap()),
            record_count: u32::from_le_bytes(buf[12..16].try_into().unwrap()),
            free_page_count: u32::from_le_bytes(buf[16..20].try_into().unwrap()),
            first_data_page: u32::from_le_bytes(buf[20..24].try_into().unwrap()),
            free_page_bitmap_offset: u64::from_le_bytes(buf[24..32].try_into().unwrap()),
            free_page_bitmap_size: u32::from_le_bytes(buf[32..36].try_into().unwrap()),
            max_pages: u32::from_le_bytes(buf[36..40].try_into().unwrap()),
        })
    }

    pub fn write_to_page(&self, page: &mut crate::storage::page::Page) {
        let buf = self.serialize();
        page.write_data(0, &buf);
    }

    pub fn read_from_page(page: &crate::storage::page::Page) -> std::io::Result<Self> {
        let buf = page.read_data(0, FILE_HEADER_SIZE);
        Self::deserialize(buf)
    }
}

impl Default for FileHeader {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for FileHeader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "FileHeader(v{}, pages={}, records={}, free_pages={}, first_data_page={})",
            self.version, self.page_count, self.record_count, self.free_page_count, self.first_data_page
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::page::Page;

    #[test]
    fn test_serialize_deserialize_roundtrip() {
        let header = FileHeader::new();
        let buf = header.serialize();
        let restored = FileHeader::deserialize(&buf).unwrap();
        assert_eq!(restored.magic, header.magic);
        assert_eq!(restored.page_count, header.page_count);
    }

    #[test]
    fn test_write_to_and_read_from_page() {
        let mut page = Page::new(0);
        let header = FileHeader::new();
        header.write_to_page(&mut page);
        let restored = FileHeader::read_from_page(&page).unwrap();
        assert_eq!(restored.magic, FILE_HEADER_MAGIC);
    }
}