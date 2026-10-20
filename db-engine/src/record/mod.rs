// src/record/mod.rs
//
// 【模块职责】
// 定义数据库中的一行记录。
// 类比 Java：相当于一个通用的 Row / Entity 基类。
//
// 【说明】
// Slotted Page 改造后，记录的存取由 page_ops 直接操作原始字节。
// Record 本身仅作为字节容器的封装，不再参与固定长度的序列化。
// 列的解析（哪些字节对应哪个列）交给表管理器处理。

use crate::config::MAX_RECORD_SIZE;

/// 单条记录最大数据字节数
pub const MAX_DATA_SIZE: usize = MAX_RECORD_SIZE - 2;

/// 一条数据库记录：持有原始字节数据
#[derive(Debug, Clone)]
pub struct Record {
    data: Vec<u8>,
}

impl Record {
    /// 从原始字节创建一条记录
    ///
    /// # Errors
    /// 当数据长度超过 MAX_DATA_SIZE 时返回错误
    pub fn new(data: Vec<u8>) -> std::io::Result<Self> {
        if data.len() > MAX_DATA_SIZE {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("Record too large: {} bytes, max is {}", data.len(), MAX_DATA_SIZE),
            ));
        }
        Ok(Self { data })
    }

    /// 获取记录的原始数据
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// 获取记录的实际数据长度
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// 是否为空记录
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

impl std::fmt::Display for Record {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Record({} bytes)", self.data.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_record() {
        let data = b"Hello, DBMS!".to_vec();
        let record = Record::new(data.clone()).unwrap();
        assert_eq!(record.len(), 12);
        assert_eq!(record.data(), &data[..]);
    }

    #[test]
    fn test_empty_record() {
        let record = Record::new(vec![]).unwrap();
        assert!(record.is_empty());
        assert_eq!(record.len(), 0);
    }

    #[test]
    fn test_max_size_record() {
        let data = vec![0xAB; MAX_DATA_SIZE];
        let record = Record::new(data.clone()).unwrap();
        assert_eq!(record.len(), MAX_DATA_SIZE);
    }

    #[test]
    fn test_oversized_record_rejected() {
        let data = vec![0xFF; MAX_DATA_SIZE + 1];
        assert!(Record::new(data).is_err());
    }

    #[test]
    fn test_display() {
        let record = Record::new(b"test".to_vec()).unwrap();
        assert_eq!(format!("{}", record), "Record(4 bytes)");
    }
}