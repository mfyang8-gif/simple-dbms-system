// src/record/mod.rs
//
// 【模块职责】
// 定义数据库中的一行记录，以及它的序列化/反序列化格式。
// 类比 Java：相当于一个通用的 Row / Entity 基类。
//
// 【记录格式（固定 512 字节槽位）】
// ┌──────────────────────────────────────────────┐
// │ [0..2)  数据长度 (u16, 小端序)                │
// │ [2..2+len)  实际记录数据                      │
// │ [2+len..512)  填充 0                         │
// └──────────────────────────────────────────────┘
// 上层不关心内部字节含义，只存取原始字节。
// 列的解析（哪些字节对应哪个列）交给表管理器处理。

use crate::config::MAX_RECORD_SIZE;

/// 长度前缀占用的字节数
const LEN_PREFIX_SIZE: usize = 2;

/// 单条记录可用的最大数据字节数（512 - 2 = 510）
pub const MAX_DATA_SIZE: usize = MAX_RECORD_SIZE - LEN_PREFIX_SIZE;

/// 一条数据库记录：持有原始字节数据
#[derive(Debug, Clone)]
pub struct Record {
    /// 记录的实际数据（不含长度前缀）
    data: Vec<u8>,
}

impl Record {
    /// 从原始字节创建一条记录
    ///
    /// # Errors
    /// 当数据长度超过 MAX_DATA_SIZE (510) 时返回错误
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

    /// 序列化为固定 512 字节的槽位格式
    ///
    /// 格式：[长度 u16 小端] + [数据] + [0 填充到 512]
    pub fn serialize(&self) -> [u8; MAX_RECORD_SIZE] {
        let mut buf = [0u8; MAX_RECORD_SIZE];
        let len = self.data.len() as u16;
        buf[0..2].copy_from_slice(&len.to_le_bytes());
        buf[2..2 + self.data.len()].copy_from_slice(&self.data);
        buf
    }

    /// 从 512 字节的槽位数据反序列化出记录
    ///
    /// 读取前 2 字节得到长度，再截取对应长度的数据。
    pub fn deserialize(buf: &[u8]) -> std::io::Result<Self> {
        if buf.len() < MAX_RECORD_SIZE {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Buffer too small: {} < {}", buf.len(), MAX_RECORD_SIZE),
            ));
        }
        let len = u16::from_le_bytes(buf[0..2].try_into().unwrap()) as usize;
        if len > MAX_DATA_SIZE {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Invalid record length: {}, max is {}", len, MAX_DATA_SIZE),
            ));
        }
        Ok(Self {
            data: buf[2..2 + len].to_vec(),
        })
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
    fn test_serialize_deserialize_roundtrip() {
        let data = b"Hello, DBMS!".to_vec();
        let record = Record::new(data.clone()).unwrap();
        assert_eq!(record.len(), 12);

        let buf = record.serialize();
        assert_eq!(buf.len(), MAX_RECORD_SIZE);

        let restored = Record::deserialize(&buf).unwrap();
        assert_eq!(restored.data(), &data[..]);
    }

    #[test]
    fn test_empty_record() {
        let record = Record::new(vec![]).unwrap();
        assert!(record.is_empty());

        let buf = record.serialize();
        let restored = Record::deserialize(&buf).unwrap();
        assert!(restored.is_empty());
    }

    #[test]
    fn test_max_size_record() {
        let data = vec![0xAB; MAX_DATA_SIZE];
        let record = Record::new(data.clone()).unwrap();

        let buf = record.serialize();
        let restored = Record::deserialize(&buf).unwrap();
        assert_eq!(restored.data(), &data[..]);
    }

    #[test]
    fn test_oversized_record_rejected() {
        let data = vec![0xFF; MAX_DATA_SIZE + 1];
        assert!(Record::new(data).is_err());
    }

    #[test]
    fn test_length_prefix_format() {
        let data = vec![0x01, 0x02, 0x03];
        let record = Record::new(data).unwrap();
        let buf = record.serialize();

        // 前 2 字节是长度 = 3（小端序）
        assert_eq!(buf[0], 3);
        assert_eq!(buf[1], 0);
        // 紧接着是数据
        assert_eq!(buf[2], 0x01);
        assert_eq!(buf[3], 0x02);
        assert_eq!(buf[4], 0x03);
        // 后面全是 0
        assert_eq!(buf[5], 0x00);
    }
}