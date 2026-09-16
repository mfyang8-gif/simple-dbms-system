use std::sync::Arc;
use std::collections::HashMap;

use crate::record::Record;
use crate::row::{Row, RowBuilder, MAX_VARCHAR_LEN};
use crate::storage::bitmap::Bitmap;
use crate::storage::buffer_pool::BufferPool;
use crate::storage::free_page_manager::FreePageManager;
use crate::storage::page_header::PageHeader;
use crate::storage::page_ops;
use crate::types::{PageId, RowId};

// ==================== 列类型与约束定义 ====================

#[derive(Debug, Clone)]
pub enum ColumnType {
    Int,
    Varchar(usize),
}

impl ColumnType {
    pub fn is_int(&self) -> bool {
        matches!(self, ColumnType::Int)
    }
}

#[derive(Debug, Clone)]
pub enum DefaultVal {
    Int(u32),
    Varchar(String),
}

/// 列定义（包含所有常见 SQL 约束的元数据）
#[derive(Debug, Clone)]
pub struct Column {
    pub name: String,
    pub col_type: ColumnType,
    pub nullable: bool,
    pub default_value: Option<DefaultVal>,
    pub auto_increment: bool,
    pub primary_key: bool,
    pub unique: bool,
    pub foreign_key: Option<(String, String)>,
    pub check: Option<String>,
}

impl Column {
    pub fn int(name: &str) -> Self {
        Self {
            name: name.to_string(), col_type: ColumnType::Int,
            nullable: true, default_value: None, auto_increment: false,
            primary_key: false, unique: false, foreign_key: None, check: None,
        }
    }

    pub fn varchar(name: &str, max_len: usize) -> Self {
        Self {
            name: name.to_string(), col_type: ColumnType::Varchar(max_len),
            nullable: true, default_value: None, auto_increment: false,
            primary_key: false, unique: false, foreign_key: None, check: None,
        }
    }

    pub fn not_null(mut self) -> Self { self.nullable = false; self }
    pub fn default_int(mut self, value: u32) -> Self { self.default_value = Some(DefaultVal::Int(value)); self }
    pub fn default_varchar(mut self, value: &str) -> Self { self.default_value = Some(DefaultVal::Varchar(value.to_string())); self }
    pub fn auto_increment(mut self) -> Self { self.auto_increment = true; self }
    pub fn primary_key(mut self) -> Self { self.primary_key = true; self.nullable = false; self.unique = true; self }
    pub fn unique(mut self) -> Self { self.unique = true; self }
    pub fn references(mut self, table: &str, column: &str) -> Self { self.foreign_key = Some((table.to_string(), column.to_string())); self }
    pub fn check(mut self, condition: &str) -> Self { self.check = Some(condition.to_string()); self }

    // ========== 序列化：列定义 → 字节流 ==========
    pub fn serialize(&self) -> Vec<u8> {
        let mut buf = Vec::new();

        // name
        let nb = self.name.as_bytes();
        buf.extend_from_slice(&(nb.len() as u16).to_le_bytes());
        buf.extend_from_slice(nb);

        // col_type
        match &self.col_type {
            ColumnType::Int => buf.push(0),
            ColumnType::Varchar(max_len) => {
                buf.push(1);
                buf.extend_from_slice(&(*max_len as u16).to_le_bytes());
            }
        }

        // flags: bit0=nullable, bit1=auto_increment, bit2=primary_key, bit3=unique
        let flags = (if self.nullable { 1 } else { 0 })
            | (if self.auto_increment { 2 } else { 0 })
            | (if self.primary_key { 4 } else { 0 })
            | (if self.unique { 8 } else { 0 });
        buf.push(flags);

        // default_value
        match &self.default_value {
            None => buf.push(0),
            Some(DefaultVal::Int(v)) => {
                buf.push(1);
                buf.extend_from_slice(&v.to_le_bytes());
            }
            Some(DefaultVal::Varchar(s)) => {
                buf.push(2);
                let sb = s.as_bytes();
                buf.push(sb.len() as u8);
                buf.extend_from_slice(sb);
            }
        }

        // foreign_key
        match &self.foreign_key {
            None => buf.push(0),
            Some((table, column)) => {
                buf.push(1);
                let tb = table.as_bytes();
                buf.extend_from_slice(&(tb.len() as u16).to_le_bytes());
                buf.extend_from_slice(tb);
                let cb = column.as_bytes();
                buf.extend_from_slice(&(cb.len() as u16).to_le_bytes());
                buf.extend_from_slice(cb);
            }
        }

        // check
        match &self.check {
            None => buf.push(0),
            Some(condition) => {
                buf.push(1);
                let cb = condition.as_bytes();
                buf.extend_from_slice(&(cb.len() as u16).to_le_bytes());
                buf.extend_from_slice(cb);
            }
        }

        buf
    }

    // ========== 反序列化：字节流 → 列定义 ==========
    pub fn deserialize(data: &[u8], offset: &mut usize) -> std::io::Result<Self> {
        // name
        if *offset + 2 > data.len() { return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "column name length")); }
        let name_len = u16::from_le_bytes(data[*offset..*offset + 2].try_into().unwrap()) as usize;
        *offset += 2;
        if *offset + name_len > data.len() { return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "column name")); }
        let name = std::str::from_utf8(&data[*offset..*offset + name_len])
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, format!("column name: {}", e)))?.to_string();
        *offset += name_len;

        // col_type
        if *offset >= data.len() { return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "col_type")); }
        let type_tag = data[*offset]; *offset += 1;
        let col_type = match type_tag {
            0 => ColumnType::Int,
            1 => {
                if *offset + 2 > data.len() { return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "varchar max_len")); }
                let max_len = u16::from_le_bytes(data[*offset..*offset + 2].try_into().unwrap()) as usize;
                *offset += 2;
                ColumnType::Varchar(max_len)
            }
            _ => return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, format!("unknown type tag {}", type_tag))),
        };

        // flags
        if *offset >= data.len() { return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "flags")); }
        let flags = data[*offset]; *offset += 1;
        let nullable = (flags & 1) != 0;
        let auto_increment = (flags & 2) != 0;
        let primary_key = (flags & 4) != 0;
        let unique = (flags & 8) != 0;

        // default_value
        if *offset >= data.len() { return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "default flag")); }
        let has_default = data[*offset]; *offset += 1;
        let default_value = match has_default {
            0 => None,
            1 => {
                if *offset + 4 > data.len() { return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "int default")); }
                let v = u32::from_le_bytes(data[*offset..*offset + 4].try_into().unwrap());
                *offset += 4;
                Some(DefaultVal::Int(v))
            }
            2 => {
                if *offset >= data.len() { return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "varchar default len")); }
                let len = data[*offset] as usize; *offset += 1;
                if *offset + len > data.len() { return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "varchar default")); }
                let s = std::str::from_utf8(&data[*offset..*offset + len])
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, format!("default: {}", e)))?.to_string();
                *offset += len;
                Some(DefaultVal::Varchar(s))
            }
            _ => return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, format!("unknown default tag {}", has_default))),
        };

        // foreign_key
        if *offset >= data.len() { return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "fk flag")); }
        let has_fk = data[*offset]; *offset += 1;
        let foreign_key = if has_fk == 0 {
            None
        } else {
            if *offset + 2 > data.len() { return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "fk table len")); }
            let tbl_len = u16::from_le_bytes(data[*offset..*offset + 2].try_into().unwrap()) as usize; *offset += 2;
            if *offset + tbl_len > data.len() { return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "fk table")); }
            let table = std::str::from_utf8(&data[*offset..*offset + tbl_len])
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, format!("fk table: {}", e)))?.to_string();
            *offset += tbl_len;

            if *offset + 2 > data.len() { return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "fk col len")); }
            let col_len = u16::from_le_bytes(data[*offset..*offset + 2].try_into().unwrap()) as usize; *offset += 2;
            if *offset + col_len > data.len() { return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "fk col")); }
            let column = std::str::from_utf8(&data[*offset..*offset + col_len])
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, format!("fk col: {}", e)))?.to_string();
            *offset += col_len;
            Some((table, column))
        };

        // check
        if *offset >= data.len() { return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "check flag")); }
        let has_check = data[*offset]; *offset += 1;
        let check = if has_check == 0 {
            None
        } else {
            if *offset + 2 > data.len() { return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "check len")); }
            let len = u16::from_le_bytes(data[*offset..*offset + 2].try_into().unwrap()) as usize; *offset += 2;
            if *offset + len > data.len() { return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "check")); }
            let s = std::str::from_utf8(&data[*offset..*offset + len])
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, format!("check: {}", e)))?.to_string();
            *offset += len;
            Some(s)
        };

        Ok(Self { name, col_type, nullable, default_value, auto_increment, primary_key, unique, foreign_key, check })
    }
}

// ==================== CHECK 约束解析与求值 ====================

#[derive(Debug, Clone)]
struct CheckCondition {
    column_name: String,
    op: CheckOp,
    value: String,
}

#[derive(Debug, Clone)]
enum CheckOp {
    Eq, Neq, Lt, Le, Gt, Ge, In(Vec<String>),
}

impl CheckCondition {
    fn parse(condition: &str) -> std::io::Result<Self> {
        let cond = condition.trim();
        if let Some(pos) = cond.find(" IN ") {
            let col_name = cond[..pos].trim().to_string();
            let values_str = cond[pos + 4..].trim();
            let inner = &values_str[1..values_str.len() - 1];
            let values: Vec<String> = inner.split(',').map(|v| v.trim().trim_matches('\'').to_string()).collect();
            Ok(Self { column_name: col_name, op: CheckOp::In(values), value: String::new() })
        } else if let Some(pos) = cond.find("!=") {
            Ok(Self { column_name: cond[..pos].trim().to_string(), op: CheckOp::Neq, value: cond[pos + 2..].trim().trim_matches('\'').to_string() })
        } else if let Some(pos) = cond.find("<=") {
            Ok(Self { column_name: cond[..pos].trim().to_string(), op: CheckOp::Le, value: cond[pos + 2..].trim().trim_matches('\'').to_string() })
        } else if let Some(pos) = cond.find(">=") {
            Ok(Self { column_name: cond[..pos].trim().to_string(), op: CheckOp::Ge, value: cond[pos + 2..].trim().trim_matches('\'').to_string() })
        } else if let Some(pos) = cond.find('<') {
            Ok(Self { column_name: cond[..pos].trim().to_string(), op: CheckOp::Lt, value: cond[pos + 1..].trim().trim_matches('\'').to_string() })
        } else if let Some(pos) = cond.find('>') {
            Ok(Self { column_name: cond[..pos].trim().to_string(), op: CheckOp::Gt, value: cond[pos + 1..].trim().trim_matches('\'').to_string() })
        } else if let Some(pos) = cond.find('=') {
            Ok(Self { column_name: cond[..pos].trim().to_string(), op: CheckOp::Eq, value: cond[pos + 1..].trim().trim_matches('\'').to_string() })
        } else {
            Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("Cannot parse CHECK: {}", cond)))
        }
    }

    fn evaluate(&self, row: &Row, columns: &[Column]) -> std::io::Result<bool> {
        let col_idx = match columns.iter().position(|c| c.name == self.column_name) {
            Some(i) => i,
            None => return Ok(true),
        };
        let actual_value = extract_single_column_value(row, columns, col_idx)?;
        let is_int = columns[col_idx].col_type.is_int();

        match &self.op {
            CheckOp::Eq => Ok(actual_value == self.value),
            CheckOp::Neq => Ok(actual_value != self.value),
            CheckOp::Lt => compare_values(&actual_value, &self.value, is_int, |a, b| a < b),
            CheckOp::Le => compare_values(&actual_value, &self.value, is_int, |a, b| a <= b),
            CheckOp::Gt => compare_values(&actual_value, &self.value, is_int, |a, b| a > b),
            CheckOp::Ge => compare_values(&actual_value, &self.value, is_int, |a, b| a >= b),
            CheckOp::In(values) => Ok(values.contains(&actual_value)),
        }
    }
}

fn extract_single_column_value(row: &Row, columns: &[Column], col_idx: usize) -> std::io::Result<String> {
    let mut cursor = row.cursor();
    for i in 0..columns.len() {
        match &columns[i].col_type {
            ColumnType::Int => {
                let v = cursor.get_int()?;
                if i == col_idx { return Ok(v.to_string()); }
            }
            ColumnType::Varchar(_) => {
                let s = cursor.get_varchar()?;
                if i == col_idx { return Ok(s.to_string()); }
            }
        }
    }
    Err(std::io::Error::new(std::io::ErrorKind::NotFound, "Column index out of range"))
}

fn compare_values<F>(actual: &str, expected: &str, is_int: bool, cmp: F) -> std::io::Result<bool>
where F: FnOnce(i64, i64) -> bool,
{
    if is_int {
        let a: i64 = actual.parse().unwrap_or(0);
        let b: i64 = expected.parse().unwrap_or(0);
        Ok(cmp(a, b))
    } else {
        Ok(cmp_str(actual, expected, &cmp))
    }
}
fn cmp_str<F>(a: &str, b: &str, _: &F) -> bool where F: FnOnce(i64, i64) -> bool { a == b }

// ==================== Schema 绑定的 RowBuilder ====================

pub struct TypedRowBuilder<'a> {
    builder: RowBuilder,
    columns: &'a [Column],
    current_col: usize,
}

impl<'a> TypedRowBuilder<'a> {
    pub fn new(columns: &'a [Column]) -> Self { Self { builder: RowBuilder::new(), columns, current_col: 0 } }

    pub fn put_int(mut self, value: u32) -> std::io::Result<Self> {
        let col = &self.columns[self.current_col];
        if col.col_type.is_int() {
            self.builder = self.builder.put_int(value);
            self.current_col += 1;
            Ok(self)
        } else {
            Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "Type mismatch"))
        }
    }

    pub fn put_varchar(mut self, value: &str) -> std::io::Result<Self> {
        let col = &self.columns[self.current_col];
        if let ColumnType::Varchar(max_len) = col.col_type {
            if value.len() > max_len { return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "VARCHAR too long")); }
            self.builder = self.builder.put_varchar(value)?;
            self.current_col += 1;
            Ok(self)
        } else {
            Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "Type mismatch"))
        }
    }

    pub fn build(self) -> std::io::Result<Row> {
        if self.current_col != self.columns.len() {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "Column count mismatch"));
        }
        Ok(self.builder.build())
    }
}

// ==================== 表管理器 ====================

pub struct Table {
    pub name: String,
    pub columns: Vec<Column>,
    first_data_page: PageId,
    page_count: u32,
    record_count: u32,
    buffer_pool: Arc<BufferPool>,
    data_pages: Vec<PageId>,
    /// AUTO_INCREMENT 的下一个分配值 (列名 -> next_id)
    auto_increment_next: HashMap<String, u32>,
}

impl Table {
    pub fn new(name: String, columns: Vec<Column>, first_data_page: PageId, buffer_pool: Arc<BufferPool>) -> std::io::Result<Self> {
        let page_ref = buffer_pool.fetch_page(first_data_page)?;
        {
            let mut page = page_ref.write().unwrap();
            PageHeader::new().write_to_page(&mut page);
            page.write_data(crate::config::PAGE_HEADER_SIZE, Bitmap::new().to_bytes());
        }

        let mut auto_increment_next = HashMap::new();
        for col in &columns {
            if col.auto_increment {
                auto_increment_next.insert(col.name.clone(), 1);
            }
        }

        Ok(Self {
            name, columns, first_data_page, page_count: 1, record_count: 0,
            buffer_pool, data_pages: vec![first_data_page],
            auto_increment_next,
        })
    }

    pub fn new_row(&self) -> TypedRowBuilder<'_> { TypedRowBuilder::new(&self.columns) }

    /// 高层插入接口：带有完整的约束校验与自增填充
    pub fn insert_row(&mut self, row: Row, free_mgr: &mut FreePageManager, catalog: &mut crate::catalog::Catalog) -> std::io::Result<RowId> {
        // 1. 拦截并处理 AUTO_INCREMENT
        let filled_row = self.fill_auto_increment(row)?;

        // 2. 校验 UNIQUE / PRIMARY KEY
        self.check_unique_constraints(&filled_row)?;

        // 3. 校验 CHECK 约束
        self.evaluate_check_constraints(&filled_row)?;

        // 4. 真正落盘写入数据
        let record = filled_row.to_record()?;
        self.insert_raw(&record, free_mgr, catalog)
    }

    /// 填充 AUTO_INCREMENT 的逻辑
    /// 采用“解析旧 Row -> 替换特定字段 -> 组装新 Row”的策略
    fn fill_auto_increment(&mut self, row: Row) -> std::io::Result<Row> {
        // 找出哪些列是自增列
        let auto_cols: Vec<usize> = self.columns.iter().enumerate()
            .filter(|(_, c)| c.auto_increment).map(|(i, _)| i).collect();

        if auto_cols.is_empty() { return Ok(row); }

        let mut cursor = row.cursor();
        let mut builder = RowBuilder::new();

        for (i, col) in self.columns.iter().enumerate() {
            if auto_cols.contains(&i) {
                // 读取用户填的 dummy 占位值（如 0）并抛弃
                match &col.col_type {
                    ColumnType::Int => { cursor.get_int()?; }
                    ColumnType::Varchar(_) => { cursor.get_varchar()?; }
                }

                // 从内存状态中取出真正的 next_id 并写入，然后自增 1
                let next_id = self.auto_increment_next.entry(col.name.clone()).or_insert(1);
                builder = builder.put_int(*next_id);
                *next_id += 1;
            } else {
                // 如果不是自增列，把老 row 里的数据原封不动读出来，塞进新 row
                match &col.col_type {
                    ColumnType::Int => { builder = builder.put_int(cursor.get_int()?); }
                    ColumnType::Varchar(_) => { builder = builder.put_varchar(cursor.get_varchar()?)?; }
                }
            }
        }

        // 返回重组后的 Row
        Ok(builder.build())
    }

    fn evaluate_check_constraints(&self, row: &Row) -> std::io::Result<()> {
        for col in &self.columns {
            if let Some(ref condition) = col.check {
                let parsed = CheckCondition::parse(condition)?;
                if !parsed.evaluate(row, &self.columns)? {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        format!("CHECK constraint violated for column '{}': {}", col.name, condition),
                    ));
                }
            }
        }
        Ok(())
    }

    fn check_unique_constraints(&self, new_row: &Row) -> std::io::Result<()> {
        let unique_cols: Vec<usize> = self.columns.iter().enumerate()
            .filter(|(_, col)| col.primary_key || col.unique).map(|(i, _)| i).collect();

        if unique_cols.is_empty() { return Ok(()); }
        let new_values = self.extract_column_values(new_row, &unique_cols)?;

        for (_, existing_record) in self.scan_all()? {
            let existing_row = Row::from_record(&existing_record);
            let existing_values = self.extract_column_values(&existing_row, &unique_cols)?;

            if new_values.iter().zip(existing_values.iter()).all(|(a, b)| a == b) {
                let col = &self.columns[unique_cols[0]];
                let kind = if col.primary_key { "PRIMARY KEY" } else { "UNIQUE" };
                return Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, format!("Duplicate value for {} column '{}'", kind, col.name)));
            }
        }
        Ok(())
    }

    fn extract_column_values(&self, row: &Row, col_indices: &[usize]) -> std::io::Result<Vec<Vec<u8>>> {
        let mut values = Vec::new();
        let mut cursor = row.cursor();
        let mut current_col = 0;

        for &target_col in col_indices {
            while current_col < target_col {
                match &self.columns[current_col].col_type {
                    ColumnType::Int => { cursor.get_int()?; }
                    ColumnType::Varchar(_) => { cursor.get_varchar()?; }
                }
                current_col += 1;
            }
            let value = match &self.columns[target_col].col_type {
                ColumnType::Int => cursor.get_int()?.to_le_bytes().to_vec(),
                ColumnType::Varchar(_) => cursor.get_varchar()?.as_bytes().to_vec(),
            };
            values.push(value);
            current_col += 1;
        }
        Ok(values)
    }

    /// 纯底层插入，绕过所有校验逻辑（内部使用）
    /// 纯底层插入，绕过所有校验逻辑（内部使用）
    pub fn insert_raw(&mut self, record: &Record, free_mgr: &mut FreePageManager, catalog: &mut crate::catalog::Catalog) -> std::io::Result<RowId> {
        let last_page = *self.data_pages.last().unwrap();
        let page_ref = self.buffer_pool.fetch_page(last_page)?;
        if let Some(slot) = page_ops::insert_record(&mut page_ref.write().unwrap(), record) {
            self.record_count += 1;
            return Ok(RowId::new(last_page, slot));
        }

        // 当前页已满，分配新页
        let new_page_id = free_mgr.find_free_page().unwrap();
        free_mgr.mark_used(new_page_id);

        let new_page_ref = self.buffer_pool.fetch_page(new_page_id)?;
        {
            let mut page = new_page_ref.write().unwrap();
            PageHeader::new().write_to_page(&mut page);
            page.write_data(crate::config::PAGE_HEADER_SIZE, Bitmap::new().to_bytes());
            if let Some(slot) = page_ops::insert_record(&mut page, record) {
                self.record_count += 1;
                self.page_count += 1;
                self.data_pages.push(new_page_id);

                // 同步更新 Catalog 中的 TableMeta
                catalog.update_table_meta(
                    &self.name,
                    self.page_count,
                    self.record_count,
                    self.data_pages.clone(),
                    free_mgr,
                )?;

                return Ok(RowId::new(new_page_id, slot));
            }
        }
        Err(std::io::Error::new(std::io::ErrorKind::Other, "Insert failed"))
    }

    pub fn read(&self, row_id: RowId) -> std::io::Result<Record> {
        page_ops::read_record(&self.buffer_pool.fetch_page(row_id.page_id)?.read().unwrap(), row_id.slot_id)
    }

    pub fn delete(&mut self, row_id: RowId) -> std::io::Result<()> {
        page_ops::delete_record(&mut self.buffer_pool.fetch_page(row_id.page_id)?.write().unwrap(), row_id.slot_id)?;
        self.record_count -= 1;
        Ok(())
    }

    pub fn scan_all(&self) -> std::io::Result<Vec<(RowId, Record)>> {
        let mut results = Vec::new();
        for &page_id in &self.data_pages {
            for (slot_id, record) in page_ops::scan_page(&self.buffer_pool.fetch_page(page_id)?.read().unwrap()) {
                results.push((RowId::new(page_id, slot_id), record));
            }
        }
        Ok(results)
    }

    pub fn record_count(&self) -> u32 { self.record_count }
    pub fn page_count(&self) -> u32 { self.page_count }
    pub fn data_pages(&self) -> &[PageId] { &self.data_pages }
}

impl std::fmt::Display for Table {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let col_names: Vec<&str> = self.columns.iter().map(|c| c.name.as_str()).collect();
        write!(f, "Table '{}' [{}] pages={}, records={}", self.name, col_names.join(", "), self.page_count, self.record_count)
    }
}