// src/sql/executor.rs

use crate::sql::ast::*;
use crate::database::Database;
use crate::table::{Column, ColumnType};
use crate::types::RowId;
/// 执行结果：SELECT 返回行，其它语句返回受影响行数
use crate::row::RowBuilder;
// 你之前只 import 了 Column，加个 ColumnType
use crate::record::Record;
use crate::row::Row;
use crate::table::Table;
use std::cmp::Ordering;
use std::collections::HashSet;
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
        Statement::Insert { table, columns, values } => execute_insert(table, columns, values, db, session),
        Statement::Select { columns, table, filter, order_by, .. } => execute_select(columns, table, filter, order_by, db),
        Statement::Delete { table, filter } => execute_delete(table, filter, db, session),
        Statement::Begin => {
            if session.txn_id.is_some() {
                return Err("transaction already in progress".into());
            }
            let (txn_id, _begin_lsn) = db.txn_mgr().lock().unwrap().begin();
            session.txn_id = Some(txn_id);
            Ok(ResultSet::Ok)
        }
        Statement::Commit => {
            let txn_id = session.txn_id.ok_or("no transaction in progress")?;
            db.txn_mgr().lock().unwrap().commit(txn_id);
            session.txn_id = None;
            Ok(ResultSet::Ok)
        }
        Statement::Rollback => {
            let txn_id = session.txn_id.ok_or("no transaction in progress")?;
            let last_lsn = db.txn_mgr().lock().unwrap().last_lsn(txn_id);

            // 运行时 Undo：沿 prev_lsn 链撤回所有 Update（写 CLR + 恢复 before image）
            crate::recovery::restore::undo_txn(
                txn_id,
                last_lsn,
                db.buffer_pool(),
                db.log_manager(),
                crate::config::LOG_FILE,
            ).map_err(|e| format!("rollback failed: {}", e))?;

            // 写 Abort 日志 + 从活跃表摘除
            db.txn_mgr().lock().unwrap().abort(txn_id);
            session.txn_id = None;
            Ok(ResultSet::Ok)
        }
        Statement::Update { table, assignments, filter } => execute_update(table, assignments, filter, db, session),
    }
}


fn execute_drop_table(name: String, db: &mut Database) -> Result<ResultSet, String> {
    db.drop_table(&name).map_err(|e| format!("drop_table failed: {}", e))?;
    Ok(ResultSet::Ok)
}
fn execute_insert(table_name: String, columns: Vec<String>, values: Vec<Expr>, db: &mut Database, session: &mut Session) -> Result<ResultSet, String> {
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
                // put_varchar 返回 Result<Self>，必须 ? 剥壳
                builder = builder.put_varchar(&s)
                    .map_err(|e| format!("varchar failed: {}", e))?;
            }
            (_, other) => {
                return Err(format!("type mismatch on column '{}': got {:?}", col.name, other));
            }
        }
    }
    let row = builder.build();

    // 5. 调存储层（session 有事务 → 加入该事务；否则自动提交）
    db.insert_in_txn(&mut table, row, session.txn_id)
        .map_err(|e| format!("insert failed: {}", e))?;
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
fn execute_select(columns: Vec<SelectItem>, table_name: String, filter: Option<Expr>, order_by: Vec<OrderByItem>, db: &mut Database) -> Result<ResultSet, String> {
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

    // 先把 ORDER BY 的列名解析成下标（提前报错，不拖到扫完）
    let mut sort_keys: Vec<(usize, bool)> = Vec::new();
    for item in &order_by {
        let i = all_col_names.iter().position(|c| c == &item.column)
            .ok_or_else(|| format!("column '{}' not found in ORDER BY", item.column))?;
        sort_keys.push((i, item.asc));
    }

    // 全表扫描
    let out_rows = table.scan_all().map_err(|e| format!("scan_all failed: {}", e))?;

    // 逐行：解码 → 过滤（用全列），先不投影（ORDER BY 可能引用不在 SELECT 里的列）
    let mut decoded: Vec<Vec<Value>> = Vec::new();
    for (_row_id, record) in &out_rows {
        let vals = decode_row(&table, record)?;
        if let Some(f) = &filter {
            if !eval_expr(f, &all_col_names, &vals)? {
                continue;
            }
        }
        decoded.push(vals);
    }

    // 排序（在投影之前，直接用源表列的值比较）
    if !sort_keys.is_empty() {
        decoded.sort_by(|a, b| {
            for &(idx, asc) in &sort_keys {
                let ord = match (&a[idx], &b[idx]) {
                    (Value::Int(x), Value::Int(y)) => x.cmp(y),
                    (Value::Str(x), Value::Str(y)) => x.cmp(y),
                    _ => std::cmp::Ordering::Equal,
                };
                if ord != std::cmp::Ordering::Equal {
                    return if asc { ord } else { ord.reverse() };
                }
            }
            std::cmp::Ordering::Equal
        });
    }

    // 投影（只取 proj 下标）
    let rows: Vec<Vec<String>> = decoded.iter()
        .map(|vals| proj.iter().map(|&i| vals[i].to_string_readable()).collect())
        .collect();

    // 输出的列名也只保留投影的
    let out_col_names: Vec<String> = proj.iter()
        .map(|&i| all_col_names[i].clone())
        .collect();

    Ok(ResultSet::Rows { columns: out_col_names, rows })
}
// 删除表中所有行
fn execute_delete(t: String, f: Option<Expr>, db: &mut Database, session: &mut Session) -> Result<ResultSet, String> {
    let mut table = db.open_table(&t)
        .map_err(|e| format!("open_table failed: {}", e))?;

    // 全列名（schema 顺序）
    let all_col_names: Vec<String> = table.columns.iter().map(|c| c.name.clone()).collect();
    let out_rows = table.scan_all().map_err(|e| format!("scan_all failed: {}", e))?;

    // 先收集命中的 RowId（决定与执行分离）
    let mut row_ids = Vec::new();
    for (row_id, record) in &out_rows {
        let vals = decode_row(&table, record)?;
        if let Some(f) = &f {
            if !eval_expr(f, &all_col_names, &vals)? {
                continue;
            }
        }
        row_ids.push(*row_id);
    }

    let count = row_ids.len();
    for row_id in row_ids {
        table.delete(row_id, db.txn_mgr(), session.txn_id)
            .map_err(|e| format!("delete failed: {}", e))?;
    }
    Ok(ResultSet::Affected(count))
}

/// 唯一性预检：把"改完之后"的键集合算出来，检查新行之间、以及新行与"没被改的旧行"是否撞车。
/// 必须在动任何数据之前跑完，否则删了旧行才发现冲突，表就残了。
fn check_update_unique_keys(table: &Table, plan: &[(RowId, Vec<Value>)]) -> Result<(), String> {
    let unique_cols: Vec<usize> = table.columns.iter().enumerate()
        .filter(|(_, c)| c.primary_key || c.unique)
        .map(|(i, _)| i)
        .collect();
    if unique_cols.is_empty() {
        return Ok(());
    }

    let key_of = |vals: &[Value]| -> Vec<String> {
        unique_cols.iter().map(|&i| vals[i].to_string_readable()).collect()
    };

    // ① 没被 UPDATE 碰到的行 → 它们的键继续被占用
    let changing: HashSet<RowId> = plan.iter().map(|(id, _)| *id).collect();
    let mut taken: HashSet<Vec<String>> = HashSet::new();
    for (row_id, record) in table.scan_all().map_err(|e| format!("scan_all failed: {}", e))? {
        if changing.contains(&row_id) {
            continue;
        }
        taken.insert(key_of(&decode_row(table, &record)?));
    }

    // ② 新行逐个占坑，insert 失败 = 自己跟自己或跟旧行重复
    for (_, new_vals) in plan {
        if !taken.insert(key_of(new_vals)) {
            let col = &table.columns[unique_cols[0]];
            let kind = if col.primary_key { "PRIMARY KEY" } else { "UNIQUE" };
            return Err(format!("Duplicate value for {} column '{}'", kind, col.name));
        }
    }
    Ok(())
}

/// UPDATE 的落地策略：**删旧行 + 插新行**。
/// 为什么不原地改字节：页层只有 insert/delete 两套 WAL 记录，没有 update_record；
/// 而 delete + insert 的 before/after 镜像已经完整，ROLLBACK 时倒着撤就能还原。
fn execute_update(
    table_name: String,
    assignments: Vec<(String, Expr)>,
    filter: Option<Expr>,
    db: &mut Database,
    session: &mut Session,
) -> Result<ResultSet, String> {
    if assignments.is_empty() {
        return Err("UPDATE requires at least one assignment".to_string());
    }

    let mut table = db.open_table(&table_name)
        .map_err(|e| format!("open_table failed: {}", e))?;
    let all_col_names: Vec<String> = table.columns.iter().map(|c| c.name.clone()).collect();

    // ① SET 的列名先解析成下标：列名写错要在动数据之前报出来
    let mut set_items: Vec<(usize, Expr)> = Vec::new();
    for (name, expr) in assignments {
        let idx = all_col_names.iter().position(|c| c == &name)
            .ok_or_else(|| format!("column '{}' not found in UPDATE", name))?;
        set_items.push((idx, expr));
    }

    // ② 扫描 + 过滤 + 在内存里算出每行的新值（此时一个字节都还没改）
    let scanned = table.scan_all().map_err(|e| format!("scan_all failed: {}", e))?;
    let mut plan: Vec<(RowId, Vec<Value>)> = Vec::new();
    for (row_id, record) in &scanned {
        let old_vals = decode_row(&table, record)?;
        if let Some(f) = &filter {
            if !eval_expr(f, &all_col_names, &old_vals)? {
                continue;
            }
        }
        let mut new_vals = old_vals.clone();
        for (idx, expr) in &set_items {
            // 右边一律用"改之前的旧行"求值（标准语义：SET a = b, b = a 是交换，不是级联）
            new_vals[*idx] = eval_scalar(expr, &all_col_names, &old_vals)?;
        }
        plan.push((*row_id, new_vals));
    }
    if plan.is_empty() {
        return Ok(ResultSet::Affected(0));
    }

    // ③ 先按 schema 编码成 Row（类型/长度错误在这一步暴露），再预检唯一键
    let mut rows: Vec<(RowId, Row)> = Vec::with_capacity(plan.len());
    for (row_id, new_vals) in &plan {
        let mut builder = table.new_row();
        for (i, v) in new_vals.iter().enumerate() {
            builder = match (&table.columns[i].col_type, v) {
                (ColumnType::Int, Value::Int(n)) => builder
                    .put_int(*n as u32)
                    .map_err(|e| format!("column '{}': {}", all_col_names[i], e))?,
                (ColumnType::Varchar(_), Value::Str(s)) => builder
                    .put_varchar(s)
                    .map_err(|e| format!("column '{}': {}", all_col_names[i], e))?,
                (_, other) => {
                    return Err(format!("type mismatch on column '{}': got {:?}", all_col_names[i], other));
                }
            };
        }
        let row = builder.build().map_err(|e| format!("build row failed: {}", e))?;
        rows.push((*row_id, row));
    }
    check_update_unique_keys(&table, &plan)?;

    // ④ 真正落盘：删旧 + 插新。两步都挂在同一条事务链上，ROLLBACK 能整体还原
    let count = rows.len();
    for (row_id, row) in rows {
        table.delete(row_id, db.txn_mgr(), session.txn_id)
            .map_err(|e| format!("delete old row failed: {}", e))?;
        db.insert_in_txn(&mut table, row, session.txn_id)
            .map_err(|e| format!("insert new row failed: {}", e))?;
    }

    Ok(ResultSet::Affected(count))
}


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

    /// 所有 e2e 测试共用 ./data/db.db，并行跑会互相删文件（Invalid magic: got 0x00000000）。
    /// 拿一把全局锁把"开库→建表→断言→关库"整段串起来，比给每个测试造一套路径简单得多。
    /// 用 `let _guard = db_lock();` 绑定到变量，守卫才会活到测试结束（`let _ =` 会立刻释放）。
    fn db_lock() -> std::sync::MutexGuard<'static, ()> {
        use std::sync::{Mutex, OnceLock};
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())  // 前一个测试 panic 不该连坐
    }

    #[test]
    fn test_e2e_create_and_insert() {
        let _guard = db_lock();
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
        let _guard = db_lock();
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
        let _guard = db_lock();
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
        let _guard = db_lock();
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
        let _guard = db_lock();
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
        let _guard = db_lock();
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
    #[test]
    fn test_e2e_delete() {
        let _guard = db_lock();
        let _ = std::fs::remove_file(crate::config::DB_FILE);
        let _ = std::fs::remove_file(crate::config::LOG_FILE);

        let mut db = Database::open().unwrap();
        let mut session = Session::new();

        exec_sql(&mut db, &mut session, "CREATE TABLE d_emp (id INT PRIMARY KEY, age INT, name VARCHAR(50))").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO d_emp VALUES (1, 20, 'Alice')").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO d_emp VALUES (2, 17, 'Bob')").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO d_emp VALUES (3, 25, 'Carter')").unwrap();

        // 条件删除：age < 18 → 只删 Bob
        let r = exec_sql(&mut db, &mut session, "DELETE FROM d_emp WHERE age < 18").unwrap();
        assert!(matches!(r, ResultSet::Affected(1)), "got {:?}", r);

        // 验证：剩 2 行且没有 Bob
        let r = exec_sql(&mut db, &mut session, "SELECT name FROM d_emp").unwrap();
        match r {
            ResultSet::Rows { rows, .. } => {
                assert_eq!(rows.len(), 2);
                assert!(!rows.iter().any(|row| row[0] == "Bob"));
            }
            other => panic!("expected Rows, got {:?}", other),
        }
        let _ = db.shutdown();
    }
    #[test]
    fn test_e2e_begin_commit() {
        let _guard = db_lock();
        let _ = std::fs::remove_file(crate::config::DB_FILE);
        let _ = std::fs::remove_file(crate::config::LOG_FILE);
        let mut db = Database::open().unwrap();
        let mut session = Session::new();

        exec_sql(&mut db, &mut session, "CREATE TABLE t_emp (id INT PRIMARY KEY, name VARCHAR(50))").unwrap();
        exec_sql(&mut db, &mut session, "BEGIN").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO t_emp VALUES (1, 'Alice')").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO t_emp VALUES (2, 'Bob')").unwrap();
        assert!(session.txn_id.is_some());
        exec_sql(&mut db, &mut session, "COMMIT").unwrap();
        assert!(session.txn_id.is_none());

        match exec_sql(&mut db, &mut session, "SELECT * FROM t_emp").unwrap() {
            ResultSet::Rows { rows, .. } => assert_eq!(rows.len(), 2),
            other => panic!("got {:?}", other),
        }
        let _ = db.shutdown();
    }

    #[test]
    fn test_e2e_rollback_undoes_insert() {
        let _guard = db_lock();
        let _ = std::fs::remove_file(crate::config::DB_FILE);
        let _ = std::fs::remove_file(crate::config::LOG_FILE);
        let mut db = Database::open().unwrap();
        let mut session = Session::new();

        exec_sql(&mut db, &mut session, "CREATE TABLE r_emp (id INT PRIMARY KEY, name VARCHAR(50))").unwrap();
        exec_sql(&mut db, &mut session, "BEGIN").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO r_emp VALUES (1, 'Alice')").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO r_emp VALUES (2, 'Bob')").unwrap();

        exec_sql(&mut db, &mut session, "ROLLBACK").unwrap();
        assert!(session.txn_id.is_none());

        // 回滚后两行都应该消失
        match exec_sql(&mut db, &mut session, "SELECT * FROM r_emp").unwrap() {
            ResultSet::Rows { rows, .. } => assert_eq!(rows.len(), 0, "回滚后应该没有数据"),
            other => panic!("got {:?}", other),
        }
        let _ = db.shutdown();
    }
    #[test]
    fn test_e2e_rollback_undoes_delete() {
        let _guard = db_lock();
        let _ = std::fs::remove_file(crate::config::DB_FILE);
        let _ = std::fs::remove_file(crate::config::LOG_FILE);
        let mut db = Database::open().unwrap();
        let mut session = Session::new();

        exec_sql(&mut db, &mut session, "CREATE TABLE r_emp2 (id INT PRIMARY KEY, name VARCHAR(50))").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO r_emp2 VALUES (1, 'Alice')").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO r_emp2 VALUES (2, 'Bob')").unwrap();

        exec_sql(&mut db, &mut session, "BEGIN").unwrap();
        exec_sql(&mut db, &mut session, "DELETE FROM r_emp2 WHERE id = 1").unwrap();
        exec_sql(&mut db, &mut session, "ROLLBACK").unwrap();

        match exec_sql(&mut db, &mut session, "SELECT name FROM r_emp2").unwrap() {
            ResultSet::Rows { rows, .. } => {
                assert_eq!(rows.len(), 2, "回滚后两行都应该还在");
                assert!(rows.iter().any(|r| r[0] == "Alice"), "Alice 应该被恢复");
            }
            other => panic!("got {:?}", other),
        }
        let _ = db.shutdown();
    }

    #[test]
    fn test_e2e_update_where() {
        let _guard = db_lock();
        let _ = std::fs::remove_file(crate::config::DB_FILE);
        let _ = std::fs::remove_file(crate::config::LOG_FILE);
        let mut db = Database::open().unwrap();
        let mut session = Session::new();

        exec_sql(&mut db, &mut session, "CREATE TABLE u_emp (id INT PRIMARY KEY, age INT, name VARCHAR(50))").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO u_emp VALUES (1, 20, 'Alice')").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO u_emp VALUES (2, 17, 'Bob')").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO u_emp VALUES (3, 25, 'Carter')").unwrap();

        // 只改一行：id = 1 同时改两列
        let r = exec_sql(&mut db, &mut session,
                         "UPDATE u_emp SET name = 'Alicia', age = 21 WHERE id = 1").unwrap();
        assert!(matches!(r, ResultSet::Affected(1)), "got {:?}", r);

        match exec_sql(&mut db, &mut session, "SELECT * FROM u_emp ORDER BY id").unwrap() {
            ResultSet::Rows { rows, .. } => {
                assert_eq!(rows.len(), 3, "UPDATE 不能改变行数");
                assert_eq!(rows[0], vec!["1".to_string(), "21".to_string(), "Alicia".to_string()]);
                assert_eq!(rows[1], vec!["2".to_string(), "17".to_string(), "Bob".to_string()]);
                assert_eq!(rows[2], vec!["3".to_string(), "25".to_string(), "Carter".to_string()]);
            }
            other => panic!("expected Rows, got {:?}", other),
        }

        // 没命中任何行 → 影响 0 行
        let r = exec_sql(&mut db, &mut session, "UPDATE u_emp SET age = 99 WHERE id = 404").unwrap();
        assert!(matches!(r, ResultSet::Affected(0)), "got {:?}", r);

        // 列名写错 → 报错，并且不影响已有数据
        let err = exec_sql(&mut db, &mut session, "UPDATE u_emp SET ghost = 1").unwrap_err();
        assert!(err.contains("not found"), "got: {}", err);

        let _ = db.shutdown();
    }

    #[test]
    fn test_e2e_update_rollback() {
        let _guard = db_lock();
        let _ = std::fs::remove_file(crate::config::DB_FILE);
        let _ = std::fs::remove_file(crate::config::LOG_FILE);
        let mut db = Database::open().unwrap();
        let mut session = Session::new();

        exec_sql(&mut db, &mut session, "CREATE TABLE ur_emp (id INT PRIMARY KEY, name VARCHAR(50))").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO ur_emp VALUES (1, 'Alice')").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO ur_emp VALUES (2, 'Bob')").unwrap();

        exec_sql(&mut db, &mut session, "BEGIN").unwrap();
        exec_sql(&mut db, &mut session, "UPDATE ur_emp SET name = 'Zed' WHERE id = 1").unwrap();

        // 事务里看得到新值
        match exec_sql(&mut db, &mut session, "SELECT name FROM ur_emp WHERE id = 1").unwrap() {
            ResultSet::Rows { rows, .. } => assert_eq!(rows[0][0], "Zed"),
            other => panic!("got {:?}", other),
        }

        exec_sql(&mut db, &mut session, "ROLLBACK").unwrap();
        assert!(session.txn_id.is_none());

        // 回滚后：旧值回来，行数不变
        match exec_sql(&mut db, &mut session, "SELECT * FROM ur_emp ORDER BY id").unwrap() {
            ResultSet::Rows { rows, .. } => {
                assert_eq!(rows.len(), 2, "UPDATE 回滚后行数不能变");
                assert_eq!(rows[0], vec!["1".to_string(), "Alice".to_string()]);
                assert_eq!(rows[1], vec!["2".to_string(), "Bob".to_string()]);
            }
            other => panic!("got {:?}", other),
        }
        let _ = db.shutdown();
    }

    #[test]
    fn test_e2e_select_order_by() {
        let _guard = db_lock();
        let _ = std::fs::remove_file(crate::config::DB_FILE);
        let _ = std::fs::remove_file(crate::config::LOG_FILE);
        let mut db = Database::open().unwrap();
        let mut session = Session::new();

        exec_sql(&mut db, &mut session, "CREATE TABLE o_emp (id INT PRIMARY KEY, age INT, name VARCHAR(50))").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO o_emp VALUES (3, 25, 'Carter')").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO o_emp VALUES (1, 20, 'Alice')").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO o_emp VALUES (2, 20, 'Bob')").unwrap();

        // 单列升序（默认）
        match exec_sql(&mut db, &mut session, "SELECT id FROM o_emp ORDER BY id").unwrap() {
            ResultSet::Rows { rows, .. } => {
                assert_eq!(rows, vec![
                    vec!["1".to_string()],
                    vec!["2".to_string()],
                    vec!["3".to_string()],
                ]);
            }
            other => panic!("got {:?}", other),
        }

        // 降序
        match exec_sql(&mut db, &mut session, "SELECT id FROM o_emp ORDER BY id DESC").unwrap() {
            ResultSet::Rows { rows, .. } => {
                assert_eq!(rows, vec![
                    vec!["3".to_string()],
                    vec!["2".to_string()],
                    vec!["1".to_string()],
                ]);
            }
            other => panic!("got {:?}", other),
        }

        // 多列：age 升序，age 相同再按 name 降序
        match exec_sql(&mut db, &mut session,
                       "SELECT name FROM o_emp ORDER BY age ASC, name DESC").unwrap() {
            ResultSet::Rows { rows, .. } => {
                assert_eq!(rows, vec![
                    vec!["Bob".to_string()],
                    vec!["Alice".to_string()],
                    vec!["Carter".to_string()],
                ]);
            }
            other => panic!("got {:?}", other),
        }

        // ORDER BY 用不在 SELECT 列表里的列 → 允许（排序在投影之前）
        match exec_sql(&mut db, &mut session, "SELECT name FROM o_emp ORDER BY id DESC").unwrap() {
            ResultSet::Rows { rows, .. } => {
                assert_eq!(rows, vec![
                    vec!["Carter".to_string()],
                    vec!["Bob".to_string()],
                    vec!["Alice".to_string()],
                ]);
            }
            other => panic!("got {:?}", other),
        }

        // 排序列不存在 → 报错
        let err = exec_sql(&mut db, &mut session, "SELECT * FROM o_emp ORDER BY ghost").unwrap_err();
        assert!(err.contains("not found"), "got: {}", err);

        let _ = db.shutdown();
    }
}