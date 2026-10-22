// src/sql/executor.rs

use crate::sql::ast::*;
use crate::database::Database;
use crate::table::{Column, ColumnType};
/// 执行结果：SELECT 返回行，其它语句返回受影响行数
use crate::row::RowBuilder;
// 你之前只 import 了 Column，加个 ColumnType
use crate::record::Record;
use crate::row::Row;
use crate::table::Table;
use std::cmp::Ordering;
#[derive(Debug)]
pub enum ResultSet {
    Rows { columns: Vec<String>, rows: Vec<Vec<String>> },
    Affected(usize),
    Ok,
}

/// 每个 TCP 连接一份，跨语句保持事务状态
pub struct Session {
    pub txn_id: Option<u64>,   // 当前活跃事务 ID；None 表示不在事务中
}
fn ast_col_to_column(def: ColumnDef) -> Column {
    let base = match def.data_type {
        DataType::Int => Column::int(&def.name),
        DataType::Varchar(n) => Column::varchar(&def.name, n),
    };
    let mut c = base;
    if def.primary_key { c = c.primary_key(); }
    if def.not_null    { c = c.not_null(); }
    c
}
#[derive(Debug, Clone, PartialEq)]
enum Value {
    Int(i64),
    Str(String),
}

impl Value {
    fn to_string_readable(&self) -> String {
        match self {
            Value::Int(n) => n.to_string(),
            Value::Str(s) => s.clone(),
        }
    }
}

/// 按 schema 把一行字节解析成 Vec<Value>
fn decode_row(table: &Table, record: &Record) -> Result<Vec<Value>, String> {
    let row = Row::from_record(record);
    let mut cursor = row.cursor();
    let mut vals = Vec::new();
    for col in &table.columns {
        let v = match &col.col_type {
            ColumnType::Int => Value::Int(
                cursor.get_int().map_err(|e| format!("decode col '{}': {}", col.name, e))? as i64,
            ),
            ColumnType::Varchar(_) => Value::Str(
                cursor.get_varchar().map_err(|e| format!("decode col '{}': {}", col.name, e))?.to_string(),
            ),
        };
        vals.push(v);
    }
    Ok(vals)
}
fn execute_create_table(name: String, columns: Vec<ColumnDef>, db: &mut Database) -> Result<ResultSet, String> {
    // 1. 把每个 ColumnDef 转成 Column
    let real_cols: Vec<Column> = columns.into_iter().map(ast_col_to_column).collect();
    // 2. 调 db.create_table
    db.create_table(&name, real_cols)
        .map_err(|e| format!("create_table failed: {}", e))?;
    // 3. 返回 OK
    Ok(ResultSet::Ok)
}

impl Session {
    pub fn new() -> Self { Self { txn_id: None } }
}

/// 执行一条 AST，返回结果集
pub fn execute(stmt: Statement, db: &mut Database, session: &mut Session) -> Result<ResultSet, String> {
    match stmt {
        Statement::CreateTable { name, columns } => {
            execute_create_table(name, columns, db)
        }
        Statement::DropTable { name } => execute_drop_table(name, db),
        Statement::Insert { table, columns, values } => execute_insert(table, columns, values, db),
        Statement::Select { columns, table, filter, .. } => execute_select(columns, table, filter, db),
        Statement::Delete { table, filter } => execute_delete(table, filter, db),
        Statement::Begin => {
            // 调 TM.begin，把 txn_id 塞进 session
            todo!()
        }
        Statement::Commit => {
            todo!()
        }
        Statement::Rollback => {
            todo!()
        }
        Statement::Update { .. } => Err("UPDATE not yet implemented".into()),
    }
}



fn execute_drop_table(name: String, db: &mut Database) -> Result<ResultSet, String> {
    db.drop_table(&name).map_err(|e| format!("drop_table failed: {}", e))?;
    Ok(ResultSet::Ok)
}
fn execute_insert(table_name: String, columns: Vec<String>, values: Vec<Expr>, db: &mut Database) -> Result<ResultSet, String> {
    // 1. 打开表（拿到 schema）
    let mut table = db.open_table(&table_name)
        .map_err(|e| format!("open_table failed: {}", e))?;

    // 2. 决定目标列（空 = 全部列）
    let target_count = if columns.is_empty() { table.columns.len() } else { columns.len() };

    // 3. 严格校验：值数量必须等于目标列数量
    if values.len() != target_count {
        return Err(format!("expected {} values, got {}", target_count, values.len()));
    }

    // 4. 逐列构造 Row（Java 风 for 循环，不用 zip）
    let mut builder = RowBuilder::new();
    for (i, expr) in values.into_iter().enumerate() {
        let col = &table.columns[i];
        match (&col.col_type, expr) {
            (ColumnType::Int, Expr::Literal(LitValue::Number(n))) => {
                builder = builder.put_int(n as u32);
            }
            (ColumnType::Varchar(_), Expr::Literal(LitValue::Str(s))) => {
                // 关键：put_varchar 返回 Result<Self>，必须 ? 剥壳
                builder = builder.put_varchar(&s)
                    .map_err(|e| format!("varchar failed: {}", e))?;
            }
            (_, other) => {
                return Err(format!("type mismatch on column '{}': got {:?}", col.name, other));
            }
        }
    }
    let row = builder.build();

    // 5. 调存储层
    db.insert(&mut table, row).map_err(|e| format!("insert failed: {}", e))?;
    Ok(ResultSet::Affected(1))
}

// 评估标量表达式（列名查表 → 下标 → 从当前行取值）
fn eval_scalar(e: &Expr, col_names: &[String], vals: &[Value]) -> Result<Value, String> {
    match e {
        Expr::Column(name) => {
            // 列名查表 → 下标 → 从当前行取值。查不到 = 列不存在（Q3 的第三道防线）
            let idx = col_names.iter().position(|c| c == name)
                .ok_or_else(|| format!("column '{}' not found", name))?;
            Ok(vals[idx].clone())
        }
        Expr::Literal(LitValue::Number(n)) => Ok(Value::Int(*n)),
        Expr::Literal(LitValue::Str(s)) => Ok(Value::Str(s.clone())),
        other => Err(format!("unsupported scalar: {:?}", other)),
    }
}

/// 比较两个值（类型必须一致：Int 和 Int 比、Str 和 Str 比）
fn compare(l: &Value, op: &BinOp, r: &Value) -> Result<bool, String> {
    let ord = match (l, r) {
        (Value::Int(a), Value::Int(b)) => a.cmp(b),
        (Value::Str(a), Value::Str(b)) => a.cmp(b),
        _ => return Err(format!("cannot compare {:?} with {:?}", l, r)),
    };
    Ok(match op {
        BinOp::Eq => ord == Ordering::Equal,
        BinOp::Neq => ord != Ordering::Equal,
        BinOp::Lt => ord == Ordering::Less,
        BinOp::Le => ord != Ordering::Greater,
        BinOp::Gt => ord == Ordering::Greater,
        BinOp::Ge => ord != Ordering::Less,
    })
}

/// LIKE 匹配（支持 % 通配符，双指针回溯法）
fn like_match(text: &str, pattern: &str) -> bool {
    let t: Vec<char> = text.chars().collect();
    let p: Vec<char> = pattern.chars().collect();
    let (mut ti, mut pi) = (0usize, 0usize);
    let (mut last_star_p, mut last_star_t) = (usize::MAX, 0usize);

    while ti < t.len() {
        if pi < p.len() && p[pi] == t[ti] {
            ti += 1; pi += 1;                      // 字符相等，双双前进
        } else if pi < p.len() && p[pi] == '%' {
            last_star_p = pi; last_star_t = ti;    // 记住 % 的位置
            pi += 1;                               // 先假设 % 匹配 0 个字符
        } else if last_star_p != usize::MAX {
            pi = last_star_p + 1;                  // 失败 → 回溯到 %
            last_star_t += 1;                      // 让 % 多吃 1 个字符
            ti = last_star_t;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '%' { pi += 1; } // 末尾的 % 允许空匹配
    pi == p.len()
}

/// 评估整个过滤表达式 → true/false（递归下降——和 Parser 的树一一对应）
fn eval_expr(expr: &Expr, col_names: &[String], vals: &[Value]) -> Result<bool, String> {
    match expr {
        Expr::BinOp { left, op, right } => {
            let l = eval_scalar(left, col_names, vals)?;
            let r = eval_scalar(right, col_names, vals)?;
            compare(&l, op, &r)
        }
        Expr::And(a, b) => Ok(eval_expr(a, col_names, vals)? && eval_expr(b, col_names, vals)?),
        Expr::Or(a, b) => Ok(eval_expr(a, col_names, vals)? || eval_expr(b, col_names, vals)?),
        Expr::Like { expr, pattern } => {
            let v = eval_scalar(expr, col_names, vals)?;
            Ok(like_match(&v.to_string_readable(), pattern))
        }
        other => Err(format!("unsupported filter expression: {:?}", other)),
    }
}
fn execute_select(columns: Vec<SelectItem>, table_name: String, filter: Option<Expr>, db: &mut Database) -> Result<ResultSet, String> {
    let table = db.open_table(&table_name)
        .map_err(|e| format!("open_table failed: {}", e))?;

    // 全列名（schema 顺序）
    let all_col_names: Vec<String> = table.columns.iter().map(|c| c.name.clone()).collect();

    // ★ 5.4-c：决定投影下标（"要输出哪几列"）
    let proj: Vec<usize> = match &columns[..] {
        [SelectItem::Star] => (0..all_col_names.len()).collect(),   // * → 全列
        items => {
            let mut idxs = Vec::new();
            for item in items {
                match item {
                    SelectItem::Column(name) => {
                        let i = all_col_names.iter().position(|c| c == name)
                            .ok_or_else(|| format!("column '{}' not found", name))?;
                        idxs.push(i);
                    }
                    other => return Err(format!("unsupported select item: {:?}", other)),
                }
            }
            idxs
        }
    };

    // 全表扫描
    let out_rows = table.scan_all().map_err(|e| format!("scan_all failed: {}", e))?;

    // 逐行：解码 → 过滤（用全列）→ 投影（只取 proj 下标）
    let mut rows = Vec::new();
    for (_row_id, record) in &out_rows {
        let vals = decode_row(&table, record)?;
        if let Some(f) = &filter {
            if !eval_expr(f, &all_col_names, &vals)? {
                continue;
            }
        }
        let projected: Vec<String> = proj.iter()
            .map(|&i| vals[i].to_string_readable())
            .collect();
        rows.push(projected);
    }

    // 输出的列名也只保留投影的
    let out_col_names: Vec<String> = proj.iter()
        .map(|&i| all_col_names[i].clone())
        .collect();

    Ok(ResultSet::Rows { columns: out_col_names, rows })
}
fn execute_delete(_t: String, _f: Option<Expr>, _db: &mut Database) -> Result<ResultSet, String> { todo!() }


#[cfg(test)]
mod tests {
    use super::*;
    use crate::sql::lexer::Lexer;
    use crate::sql::parser::Parser;

    fn exec_sql(db: &mut Database, session: &mut Session, sql: &str) -> Result<ResultSet, String> {
        let tokens = Lexer::new(sql).tokenize()?;
        let stmt = Parser::new(tokens).parse()?;
        execute(stmt, db, session)
    }

    #[test]
    fn test_e2e_create_and_insert() {
        let mut db = Database::open().unwrap();
        let mut session = Session::new();

        // 1. 建表
        let r = exec_sql(&mut db, &mut session,
                         "CREATE TABLE e2e_users (id INT PRIMARY KEY, name VARCHAR(50))").unwrap();
        assert!(matches!(r, ResultSet::Ok));

        // 2. 插入一行
        let r = exec_sql(&mut db, &mut session,
                         "INSERT INTO e2e_users VALUES (1, 'Alice')").unwrap();
        assert!(matches!(r, ResultSet::Affected(1)));

        // 3. 类型错误（INT 列传字符串）→ executor 层拒
        let err = exec_sql(&mut db, &mut session,
                           "INSERT INTO e2e_users VALUES ('oops', 'Bob')").unwrap_err();
        assert!(err.contains("type mismatch"), "expected type mismatch, got: {}", err);

        // 4. 数量错误（只给一个值）→ executor 层拒
        let err = exec_sql(&mut db, &mut session,
                           "INSERT INTO e2e_users VALUES (99)").unwrap_err();
        assert!(err.contains("expected 2 values"), "got: {}", err);

        // 清理
        let _ = exec_sql(&mut db, &mut session, "DROP TABLE e2e_users");
        let _ = db.shutdown();
    }

    #[test]
    fn test_e2e_select_star() {
        let _ = std::fs::remove_file(crate::config::DB_FILE);
        let _ = std::fs::remove_file(crate::config::LOG_FILE);

        let mut db = Database::open().unwrap();
        let mut session = Session::new();

        exec_sql(&mut db, &mut session, "CREATE TABLE q_users (id INT PRIMARY KEY, name VARCHAR(50))").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO q_users VALUES (1, 'Alice')").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO q_users VALUES (2, 'Bob')").unwrap();

        let r = exec_sql(&mut db, &mut session, "SELECT * FROM q_users").unwrap();
        match r {
            ResultSet::Rows { columns, rows } => {
                assert_eq!(columns, vec!["id".to_string(), "name".to_string()]);
                assert_eq!(rows.len(), 2);
                assert_eq!(rows[0], vec!["1".to_string(), "Alice".to_string()]);
                assert_eq!(rows[1], vec!["2".to_string(), "Bob".to_string()]);
            }
            other => panic!("expected Rows, got {:?}", other),
        }

        let _ = db.shutdown();
    }
    #[test]
    fn test_e2e_select_where_gt() {
        let _ = std::fs::remove_file(crate::config::DB_FILE);
        let _ = std::fs::remove_file(crate::config::LOG_FILE);

        let mut db = Database::open().unwrap();
        let mut session = Session::new();

        exec_sql(&mut db, &mut session, "CREATE TABLE w_emp (id INT PRIMARY KEY, age INT, name VARCHAR(50))").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO w_emp VALUES (1, 20, 'Alice')").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO w_emp VALUES (2, 17, 'Bob')").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO w_emp VALUES (3, 25, 'Carter')").unwrap();

        let r = exec_sql(&mut db, &mut session, "SELECT * FROM w_emp WHERE age > 18").unwrap();
        match r {
            ResultSet::Rows { rows, .. } => {
                assert_eq!(rows.len(), 2);                     // Alice 和 Carter
                assert_eq!(rows[0][2], "Alice".to_string());
                assert_eq!(rows[1][2], "Carter".to_string());
            }
            other => panic!("expected Rows, got {:?}", other),
        }
        let _ = db.shutdown();
    }

    #[test]
    fn test_e2e_select_where_and_like() {
        let _ = std::fs::remove_file(crate::config::DB_FILE);
        let _ = std::fs::remove_file(crate::config::LOG_FILE);

        let mut db = Database::open().unwrap();
        let mut session = Session::new();

        exec_sql(&mut db, &mut session, "CREATE TABLE w_emp2 (id INT PRIMARY KEY, age INT, name VARCHAR(50))").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO w_emp2 VALUES (1, 20, 'Alice')").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO w_emp2 VALUES (2, 20, 'Alan')").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO w_emp2 VALUES (3, 30, 'Alan')").unwrap();

        // WHERE age = 20 AND name LIKE 'Al%' → 前两条
        let r = exec_sql(&mut db, &mut session,
                         "SELECT * FROM w_emp2 WHERE age = 20 AND name LIKE 'Al%'").unwrap();
        match r {
            ResultSet::Rows { rows, .. } => {
                assert_eq!(rows.len(), 2);
                assert_eq!(rows[0][2], "Alice".to_string());
                assert_eq!(rows[1][2], "Alan".to_string());
            }
            other => panic!("expected Rows, got {:?}", other),
        }
        let _ = db.shutdown();
    }

    #[test]
    fn test_select_unknown_column_errors() {
        let _ = std::fs::remove_file(crate::config::DB_FILE);
        let _ = std::fs::remove_file(crate::config::LOG_FILE);

        let mut db = Database::open().unwrap();
        let mut session = Session::new();

        exec_sql(&mut db, &mut session, "CREATE TABLE w_bad (id INT PRIMARY KEY)").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO w_bad VALUES (1)").unwrap();

        // 引用不存在的列 → 语义错误，第三道防线
        let err = exec_sql(&mut db, &mut session, "SELECT * FROM w_bad WHERE foo = 1").unwrap_err();
        assert!(err.contains("not found"), "got: {}", err);

        let _ = db.shutdown();
    }
    #[test]
    fn test_e2e_select_projection() {
        let _ = std::fs::remove_file(crate::config::DB_FILE);
        let _ = std::fs::remove_file(crate::config::LOG_FILE);

        let mut db = Database::open().unwrap();
        let mut session = Session::new();

        exec_sql(&mut db, &mut session, "CREATE TABLE p_emp (id INT PRIMARY KEY, age INT, name VARCHAR(50))").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO p_emp VALUES (1, 20, 'Alice')").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO p_emp VALUES (2, 17, 'Bob')").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO p_emp VALUES (3, 25, 'Carter')").unwrap();

        // 只投影 name，但 WHERE 用 age —— "先筛行后删列"的活教材
        let r = exec_sql(&mut db, &mut session,
                         "SELECT name FROM p_emp WHERE age > 18").unwrap();
        match r {
            ResultSet::Rows { columns, rows } => {
                assert_eq!(columns, vec!["name".to_string()]);
                assert_eq!(rows, vec![
                    vec!["Alice".to_string()],
                    vec!["Carter".to_string()],
                ]);
            }
            other => panic!("expected Rows, got {:?}", other),
        }

        // 列不存在 → 报错
        let err = exec_sql(&mut db, &mut session, "SELECT ghost FROM p_emp").unwrap_err();
        assert!(err.contains("not found"), "got: {}", err);

        let _ = db.shutdown();
    }
}