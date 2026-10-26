// src/row/mod.rs
//
// 【模块职责】
// 按列类型自动序列化/反序列化一行数据。
// 类比 Java：相当于一个 TypedRow / TypedTuple，提供 put_int/get_int、put_varchar/get_varchar。
//
// 【行内序列化格式】
//   [bitmap_len: u8][null bitmap: bitmap_len 字节][列 0 值][列 1 值]...
//
//   null bitmap：第 i 列是否为 NULL 占 1 个 bit（低位优先），bitmap_len = ceil(列数 / 8)。
//   NULL 列在值区**不占任何字节**——这就是 bitmap 比"每个值加一个 flag 字节"省空间的原因。
//   bitmap_len 单独存一个字节放在最前面，让行自己说清头部多长，
//   读取方不必先拿到 schema 就能跳过位图（cursor_with_cols 还会顺带校验列数是否匹配）。
//
//   INT     → u32 小端序，固定 4 字节
//   VARCHAR → 1 字节长度前缀 (u8, max 255) + UTF-8 字节
//
// 【为什么必须留 bitmap】
// VARCHAR 的空串 "" 编码成 [00]，跟"这一列没值"长得一模一样；INT 也没有哪个字节能表示"没值"。
// 靠哨兵值区分是耍流氓（i32::MIN 是合法数字），只能用行外的元数据描述 NULL。
//
// 【使用方式】
//   写入：RowBuilder::new().put_int(1).put_null().put_varchar("Alice").build() → Row
//   存储：row.to_record() → Record（可直接交给 Table::insert）
//   读取：Row::from_record(&record) → Row
//   取出：let mut c = row.cursor_with_cols(3)?; c.get_int_opt()?; c.get_varchar_opt()?;

use crate::record::Record;

/// VARCHAR 单值最大字节数（u8 长度前缀限制）
pub const MAX_VARCHAR_LEN: usize = 255;

/// null bitmap 里一列占 1 bit，8 列占 1 字节
#[inline]
pub fn null_bitmap_len(col_count: usize) -> usize {
    col_count.div_ceil(8)
}

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

    /// 行内 null bitmap 的字节数（不含最前面的长度字节）
    pub fn bitmap_len(&self) -> usize {
        self.data.first().copied().unwrap_or(0) as usize
    }

    /// 第 col 列是否为 NULL。位图里没有这一列（列数超出）时按"非 NULL"处理。
    pub fn is_null(&self, col: usize) -> bool {
        let bitmap_len = self.bitmap_len();
        if bitmap_len == 0 {
            return false;
        }
        let byte_index = 1 + col / 8;
        if byte_index >= 1 + bitmap_len {
            return false;
        }
        (self.data[byte_index] >> (col % 8)) & 1 == 1
    }

    /// 创建读取游标，自动跳过 null bitmap
    pub fn cursor(&self) -> RowCursor<'_> {
        let bitmap_len = self.bitmap_len();
        RowCursor {
            data: &self.data,
            pos: 1 + bitmap_len,
            col: 0,
            bitmap_len,
        }
    }

    /// 带 schema 校验的游标：位图长度必须等于 ceil(列数/8)。
    /// 老的 db.db（无位图格式）第一字节是列值本身，长度几乎不可能对上，
    /// 于是在这里**大声报错**，而不是把值字节当位图静默读出一堆垃圾。
    pub fn cursor_with_cols(&self, col_count: usize) -> std::io::Result<RowCursor<'_>> {
        let expected = null_bitmap_len(col_count);
        let actual = self.bitmap_len();
        if actual != expected {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "row format mismatch: null bitmap is {} bytes, schema expects {} \
                     (数据文件格式已升级，请删除 ./data/db.db 与 ./data/db.log 重建)",
                    actual, expected
                ),
            ));
        }
        Ok(self.cursor())
    }
}

// ==================== RowBuilder ====================

/// 行数据构建器：按列顺序逐列写入值
///
/// 每写一列就往 `nulls` 里记一个 flag，`build()` 时统一编码成 bitmap，
/// 这样 `new()` 不需要提前知道列数（老调用方一行都不用改）。
///
/// # Example
/// ```ignore
/// let row = RowBuilder::new()
///     .put_int(42)
///     .put_null()                  // 第二列是 NULL，不占值区字节
///     .put_varchar("Alice").unwrap()
///     .build();
/// assert!(row.is_null(1));
/// ```
pub struct RowBuilder {
    values: Vec<u8>,
    /// 每列一个 flag：true = 该列为 NULL
    nulls: Vec<bool>,
}

impl RowBuilder {
    pub fn new() -> Self {
        Self { values: Vec::new(), nulls: Vec::new() }
    }

    /// 已写入的列数（含 NULL 列）
    pub fn col_count(&self) -> usize {
        self.nulls.len()
    }

    /// 写入一个 INT 列（u32 小端序，4 字节）
    pub fn put_int(mut self, value: u32) -> Self {
        self.values.extend_from_slice(&value.to_le_bytes());
        self.nulls.push(false);
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
        self.values.push(bytes.len() as u8);
        self.values.extend_from_slice(bytes);
        self.nulls.push(false);
        Ok(self)
    }

    /// 写入一个 NULL 列：只在位图里点一个 bit，值区一个字节都不写
    pub fn put_null(mut self) -> Self {
        self.nulls.push(true);
        self
    }

    /// 完成构建，返回 Row（头部拼上 [bitmap_len][bitmap]）
    pub fn build(self) -> Row {
        let bitmap_len = null_bitmap_len(self.nulls.len());
        let mut data = Vec::with_capacity(1 + bitmap_len + self.values.len());

        data.push(bitmap_len as u8);
        data.resize(1 + bitmap_len, 0); // 位图初始化为全 0（全都非 NULL）

        for (i, is_null) in self.nulls.iter().enumerate() {
            if *is_null {
                data[1 + i / 8] |= 1 << (i % 8);
            }
        }

        data.extend_from_slice(&self.values);
        Row { data }
    }
}

// ==================== RowCursor ====================

/// 行数据读取游标：按列顺序逐列读取值
///
/// 必须按照写入时的列顺序和类型依次读取，否则解析结果错误。
/// 每个 get_* 都会把列下标前进 1；NULL 列不消耗值区字节。
#[derive(Debug)]
pub struct RowCursor<'a> {
    data: &'a [u8],
    pos: usize,
    /// 当前正在读第几列（用于查位图）
    col: usize,
    bitmap_len: usize,
}

impl<'a> RowCursor<'a> {
    /// 当前列是否为 NULL（不移动游标）
    pub fn is_null(&self) -> bool {
        if self.bitmap_len == 0 {
            return false;
        }
        let byte_index = 1 + self.col / 8;
        if byte_index >= 1 + self.bitmap_len {
            return false;
        }
        (self.data[byte_index] >> (self.col % 8)) & 1 == 1
    }

    /// 当前列下标
    pub fn col_index(&self) -> usize {
        self.col
    }

    /// 读取一个 INT 列。若该列是 NULL 则报错——想安全读请用 `get_int_opt`。
    pub fn get_int(&mut self) -> std::io::Result<u32> {
        self.get_int_opt()?.ok_or_else(|| std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("column {} is NULL, cannot read as INT", self.col),
        ))
    }

    /// 读取一个 INT 列，NULL 返回 None
    pub fn get_int_opt(&mut self) -> std::io::Result<Option<u32>> {
        let col = self.col;
        self.col += 1;
        if self.is_null_before() {
            return Ok(None);
        }
        if self.pos + 4 > self.data.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!(
                    "Not enough bytes for INT at column {}: need 4, have {}",
                    col,
                    self.data.len() - self.pos
                ),
            ));
        }
        let value = u32::from_le_bytes(self.data[self.pos..self.pos + 4].try_into().unwrap());
        self.pos += 4;
        Ok(Some(value))
    }

    /// 读取一个 VARCHAR 列。若该列是 NULL 则报错——想安全读请用 `get_varchar_opt`。
    pub fn get_varchar(&mut self) -> std::io::Result<&'a str> {
        self.get_varchar_opt()?.ok_or_else(|| std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("column {} is NULL, cannot read as VARCHAR", self.col),
        ))
    }

    /// 读取一个 VARCHAR 列，NULL 返回 None
    pub fn get_varchar_opt(&mut self) -> std::io::Result<Option<&'a str>> {
        let col = self.col;
        self.col += 1;
        if self.is_null_before() {
            return Ok(None);
        }
        if self.pos >= self.data.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!("Not enough bytes for VARCHAR length at column {}", col),
            ));
        }
        let len = self.data[self.pos] as usize;
        self.pos += 1;

        if self.pos + len > self.data.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!(
                    "Not enough bytes for VARCHAR data at column {}: need {}, have {}",
                    col,
                    len,
                    self.data.len() - self.pos
                ),
            ));
        }
        let s = std::str::from_utf8(&self.data[self.pos..self.pos + len]).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, format!("Invalid UTF-8: {}", e))
        })?;
        self.pos += len;
        Ok(Some(s))
    }

    /// 判断"刚取走的那一列"是否为 NULL。
    /// get_* 已经先把 col 自增过，所以这里查的是 col - 1。
    fn is_null_before(&self) -> bool {
        let target = self.col - 1;
        if self.bitmap_len == 0 {
            return false;
        }
        let byte_index = 1 + target / 8;
        if byte_index >= 1 + self.bitmap_len {
            return false;
        }
        (self.data[byte_index] >> (target % 8)) & 1 == 1
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

impl std::fmt::Display for Row {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Row({} bytes)", self.data.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        // 值区只有 3 字节（1B 长度 + 2B "hi"），不够读 INT（需要 4B）
        assert!(cursor.get_int().is_err());
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
        // 验证字节级格式：2 列 → 位图 1 字节，两列都非 NULL
        let row = RowBuilder::new()
            .put_int(1)
            .put_varchar("AB").unwrap()
            .build();
        let data = row.data();

        // [bitmap_len=1][bitmap=0x00]
        assert_eq!(data[0], 1);
        assert_eq!(data[1], 0x00);
        // INT(1) = [01, 00, 00, 00]（小端序）
        assert_eq!(&data[2..6], &[0x01, 0x00, 0x00, 0x00]);
        // VARCHAR("AB") = [02, 0x41, 0x42]（长度 2 + 'A' + 'B'）
        assert_eq!(data[6], 2);
        assert_eq!(&data[7..9], b"AB");
        assert_eq!(data.len(), 9);
    }

    #[test]
    fn test_null_occupies_only_one_bit() {
        // NULL 不写值区：三列 (NULL, 42, NULL) 的总长度 = 头部 2 + INT 4 = 6
        let row = RowBuilder::new()
            .put_null()
            .put_int(42)
            .put_null()
            .build();

        assert_eq!(row.data().len(), 6);
        assert!(row.is_null(0));
        assert!(!row.is_null(1));
        assert!(row.is_null(2));
        // bitmap = 0b101 = 0x05
        assert_eq!(row.data()[1], 0b101);
    }

    #[test]
    fn test_null_roundtrip_mixed() {
        let row = RowBuilder::new()
            .put_int(7)
            .put_null()
            .put_varchar("Bob").unwrap()
            .build();

        let mut cursor = row.cursor_with_cols(3).unwrap();
        assert_eq!(cursor.get_int_opt().unwrap(), Some(7));
        assert_eq!(cursor.get_varchar_opt().unwrap(), None);   // 第 2 列是 NULL
        assert_eq!(cursor.get_varchar_opt().unwrap(), Some("Bob"));
        assert!(cursor.is_eof());
    }

    #[test]
    fn test_get_int_on_null_column_errors() {
        // 老式 get_int 遇到 NULL 必须报错，绝不能把后面的字节当成值读走
        let row = RowBuilder::new().put_null().put_int(42).build();
        let mut cursor = row.cursor();
        let err = cursor.get_int().unwrap_err();
        assert!(err.to_string().contains("is NULL"), "got: {}", err);
    }

    #[test]
    fn test_null_across_byte_boundary() {
        // 第 9 列（下标 8）落在位图第 2 个字节，验证跨字节定位
        let mut builder = RowBuilder::new();
        for i in 0..10 {
            builder = if i == 8 { builder.put_null() } else { builder.put_int(i) };
        }
        let row = builder.build();

        assert_eq!(row.bitmap_len(), 2);
        assert!(row.is_null(8));
        assert!(!row.is_null(7));
        assert!(!row.is_null(9));

        let mut cursor = row.cursor_with_cols(10).unwrap();
        for i in 0..10 {
            let got = cursor.get_int_opt().unwrap();
            assert_eq!(got, if i == 8 { None } else { Some(i) }, "column {}", i);
        }
    }

    #[test]
    fn test_cursor_with_cols_rejects_old_format() {
        // 手工构造一行“老格式”字节（没有 bitmap 头）：INT(3) + VARCHAR("AB")。
        // 首字节 0x03 本来是值的一部分，会被当成“位图有 3 字节”，而 2 列只需要 1 字节 → 报错
        let old = Row::from_bytes(vec![0x03, 0x00, 0x00, 0x00, 0x02, b'A', b'B']);
        let err = old.cursor_with_cols(2).unwrap_err();
        assert!(
            err.to_string().contains("row format mismatch"),
            "expected format mismatch, got: {}",
            err
        );

        // 诚实说一句：如果老行的首字节恰好等于 ceil(列数/8)（INT(1) 配 3 列就是这样），
        // 光看行自身是发现不了的——真正兜底的是 FileHeader 的魔数 + 版本升级（见 file_header.rs）。
        let sneaky = Row::from_bytes(vec![0x01, 0x00, 0x00, 0x00, 0x02, b'A', b'B']);
        assert!(sneaky.cursor_with_cols(3).is_ok(), "位图长度恰好对得上时只能放行");
    }

    #[test]
    fn test_all_null_row() {
        let row = RowBuilder::new().put_null().put_null().build();
        let mut cursor = row.cursor_with_cols(2).unwrap();
        assert_eq!(cursor.get_int_opt().unwrap(), None);
        assert_eq!(cursor.get_varchar_opt().unwrap(), None);
        assert!(cursor.is_eof(), "NULL 列不该消耗值区字节");
    }
}
