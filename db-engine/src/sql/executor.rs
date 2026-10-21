// src/sql/executor.rs

use crate::sql::ast::*;
use crate::database::Database;
use crate::table::{Column, ColumnType};
/// 执行结果：SELECT 返回行，其它语句返回受影响行数
use crate::row::RowBuilder;
   // 你之前只 import 了 Column，加个 ColumnType
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
fn execute_select(_c: Vec<SelectItem>, _t: String, _f: Option<Expr>, _db: &mut Database) -> Result<ResultSet, String> { todo!() }
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
}