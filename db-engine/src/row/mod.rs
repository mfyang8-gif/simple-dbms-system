// src/row/mod.rs
//
// 【模块职责】
// 按列类型自动序列化/反序列化一行数据。
// 类比 Java：相当于一个 TypedRow / TypedTuple，提供 put_int/get_int、put_varchar/get_varchar。
//
// 【行内序列化格式】（自描述，无额外元数据）
//   INT     → u32 小端序，固定 4 字节
//   VARCHAR → 1 字节长度前缀 (u8, max 255) + UTF-8 字节
//
// 【使用方式】
//   写入：RowBuilder::new().put_int(1).put_varchar("Alice").build() → Row
//   存储：row.to_record() → Record（可直接交给 Table::insert）
//   读取：Row::from_record(&record)? → Row
//   取出：let mut c = row.cursor(); c.get_int()?; c.get_varchar()?;

use crate::record::Record;

/// VARCHAR 单值最大字节数（u8 长度前缀限制）
pub const MAX_VARCHAR_LEN: usize = 255;

// ==================== Row ====================

/// 按列类型序列化的一行数据
#[derive(Debug, Clone)]
pub struct Row {
    data: Vec<u8>,
}

impl Row {
    /// 从原始字节创建 Row（通常用于反序列化场景）
    pub fn from_bytes(data: Vec<u8>) -> Self {
        Self { data }
    }

    /// 获取内部原始字节
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// 转为 Record，可直接存入存储引擎
    pub fn to_record(&self) -> std::io::Result<Record> {
        Record::new(self.data.clone())
    }

    /// 从 Record 还原为 Row
    pub fn from_record(record: &Record) -> Self {
        Self {
            data: record.data().to_vec(),
        }
    }

    /// 创建读取游标，按列顺序依次取值
    pub fn cursor(&self) -> RowCursor<'_> {
        RowCursor {
            data: &self.data,
            pos: 0,
        }
    }
}

// ==================== RowBuilder ====================

/// 行数据构建器：按列顺序逐列写入值
///
/// # Example
/// ignore
/// let row = RowBuilder::new()
/// .put_int(42) /// .put_varchar("Alice") /// .build(); /// ```
 pub struct RowBuilder { data: Vec<u8>, }
impl RowBuilder { pub fn new() -> Self { Self { data: Vec::new() } }
    /// 写入一个 INT 列（u32 小端序，4 字节）
    pub fn put_int(mut self, value: u32) -> Self {
        self.data.extend_from_slice(&value.to_le_bytes());
        self
    }

    /// 写入一个 VARCHAR 列（1 字节长度 + UTF-8 字节）
    ///
    /// # Errors
    /// 当字符串字节长度超过 255 时返回错误
    pub fn put_varchar(mut self, value: &str) -> std::io::Result<Self> {
        let bytes = value.as_bytes();
        if bytes.len() > MAX_VARCHAR_LEN {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "VARCHAR too long: {} bytes, max is {}",
                    bytes.len(),
                    MAX_VARCHAR_LEN
                ),
            ));
        }
        self.data.push(bytes.len() as u8);
        self.data.extend_from_slice(bytes);
        Ok(self)
    }

    /// 完成构建，返回 Row
    pub fn build(self) -> Row {
        Row { data: self.data }
    }
}
// ==================== RowCursor ====================
/// 行数据读取游标：按列顺序逐列读取值 /// /// 必须按照写入时的列顺序和类型依次读取，否则解析结果错误。
pub struct RowCursor<'a> { data: &'a [u8], pos: usize, }
impl<'a> RowCursor<'a> { /// 读取一个 INT 列（u32 小端序，4 字节）
pub fn get_int(&mut self) -> std::io::Result<u32> { if self.pos + 4 > self.data.len() { return Err(std::io::Error::new( std::io::ErrorKind::UnexpectedEof, format!( "Not enough bytes for INT at offset {}: need 4, have {}", self.pos, self.data.len() - self.pos ), )); } let value = u32::from_le_bytes( self.data[self.pos..self.pos + 4] .try_into() .unwrap(), ); self.pos += 4; Ok(value) }
/// 读取一个 VARCHAR 列（1 字节长度 + UTF-8 字节）
pub fn get_varchar(&mut self) -> std::io::Result<&'a str> {
    if self.pos >= self.data.len() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            format!("Not enough bytes for VARCHAR length at offset {}", self.pos),
        ));
    }
    let len = self.data[self.pos] as usize;
    self.pos += 1;

    if self.pos + len > self.data.len() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            format!(
                "Not enough bytes for VARCHAR data at offset {}: need {}, have {}",
                self.pos,
                len,
                self.data.len() - self.pos
            ),
        ));
    }
    let s = std::str::from_utf8(&self.data[self.pos..self.pos + len]).map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, format!("Invalid UTF-8: {}", e))
    })?;
    self.pos += len;
    Ok(s)
}

    /// 是否已读完所有数据
    pub fn is_eof(&self) -> bool {
        self.pos >= self.data.len()
    }

    /// 当前读取位置
    pub fn position(&self) -> usize {
        self.pos
    }
}
impl std::fmt::Display for Row { fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "Row({} bytes)", self.data.len()) } }
#[cfg(test)] mod tests { use super::*;
    #[test]
    fn test_int_roundtrip() {
        let row = RowBuilder::new().put_int(42).build();
        let mut cursor = row.cursor();
        assert_eq!(cursor.get_int().unwrap(), 42);
        assert!(cursor.is_eof());
    }

    #[test]
    fn test_varchar_roundtrip() {
        let row = RowBuilder::new().put_varchar("Hello").unwrap().build();
        let mut cursor = row.cursor();
        assert_eq!(cursor.get_varchar().unwrap(), "Hello");
        assert!(cursor.is_eof());
    }

    #[test]
    fn test_empty_varchar() {
        let row = RowBuilder::new().put_varchar("").unwrap().build();
        let mut cursor = row.cursor();
        assert_eq!(cursor.get_varchar().unwrap(), "");
        assert!(cursor.is_eof());
    }

    #[test]
    fn test_multi_column_roundtrip() {
        let row = RowBuilder::new()
            .put_int(1)
            .put_varchar("Alice").unwrap()
            .put_int(25)
            .put_varchar("Engineering").unwrap()
            .build();

        let mut cursor = row.cursor();
        assert_eq!(cursor.get_int().unwrap(), 1);
        assert_eq!(cursor.get_varchar().unwrap(), "Alice");
        assert_eq!(cursor.get_int().unwrap(), 25);
        assert_eq!(cursor.get_varchar().unwrap(), "Engineering");
        assert!(cursor.is_eof());
    }

    #[test]
    fn test_record_roundtrip() {
        let row = RowBuilder::new()
            .put_int(100)
            .put_varchar("test").unwrap()
            .build();

        let record = row.to_record().unwrap();
        let restored = Row::from_record(&record);

        let mut cursor = restored.cursor();
        assert_eq!(cursor.get_int().unwrap(), 100);
        assert_eq!(cursor.get_varchar().unwrap(), "test");
    }

    #[test]
    fn test_varchar_too_long() {
        let long_str = "a".repeat(MAX_VARCHAR_LEN + 1);
        let result = RowBuilder::new().put_varchar(&long_str);
        assert!(result.is_err());
    }

    #[test]
    fn test_max_varchar() {
        let max_str = "x".repeat(MAX_VARCHAR_LEN);
        let row = RowBuilder::new().put_varchar(&max_str).unwrap().build();
        let mut cursor = row.cursor();
        assert_eq!(cursor.get_varchar().unwrap(), max_str.as_str());
    }

    #[test]
    fn test_cursor_underflow_int() {
        let row = RowBuilder::new().put_varchar("hi").unwrap().build();
        let mut cursor = row.cursor();
        // 只有 3 字节（1B 长度 + 2B "hi"），不够读 INT（需要 4B）
        assert!(cursor.get_int().is_err());
    }

    #[test]
    fn test_cursor_underflow_varchar() {
        let row = RowBuilder::new().put_int(1).build();
        let mut cursor = row.cursor();
        // INT 4 字节，cursor 把它当 VARCHAR 读：第一字节是 0x01（长度=1）
        // 但后面只剩 3 字节，其中取 1 字节做 UTF-8 是合法的
        let s = cursor.get_varchar().unwrap();
        assert_eq!(s, "\x01"); // 0x01 作为 u8 长度 = 1，然后读取 1 字节 0x00
    }

    #[test]
    fn test_chinese_varchar() {
        let row = RowBuilder::new()
            .put_int(1)
            .put_varchar("张三").unwrap()
            .put_varchar("工程部").unwrap()
            .build();

        let mut cursor = row.cursor();
        assert_eq!(cursor.get_int().unwrap(), 1);
        assert_eq!(cursor.get_varchar().unwrap(), "张三");
        assert_eq!(cursor.get_varchar().unwrap(), "工程部");
        assert!(cursor.is_eof());
    }

    #[test]
    fn test_multiple_ints() {
        let row = RowBuilder::new()
            .put_int(0)
            .put_int(u32::MAX)
            .put_int(12345)
            .build();

        let mut cursor = row.cursor();
        assert_eq!(cursor.get_int().unwrap(), 0);
        assert_eq!(cursor.get_int().unwrap(), u32::MAX);
        assert_eq!(cursor.get_int().unwrap(), 12345);
        assert!(cursor.is_eof());
    }

    #[test]
    fn test_serialization_format() {
        // 验证字节级格式：INT(1) + VARCHAR("AB")
        let row = RowBuilder::new()
            .put_int(1)
            .put_varchar("AB").unwrap()
            .build();
        let data = row.data();

        // INT(1) = [01, 00, 00, 00]（小端序）
        assert_eq!(&data[0..4], &[0x01, 0x00, 0x00, 0x00]);
        // VARCHAR("AB") = [02, 0x41, 0x42]（长度 2 + 'A' + 'B'）
        assert_eq!(data[4], 2);
        assert_eq!(&data[5..7], b"AB");
        assert_eq!(data.len(), 7);
    }
}