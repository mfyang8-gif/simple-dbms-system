// src/sql/mod.rs

pub mod token;
pub mod lexer;
pub mod ast;
mod parser;
pub mod executor;

use crate::database::Database;
pub use executor::{ResultSet, Session};

/// 门面：SQL 文本 → 词法 → 语法 → 执行
///
/// 外部（REPL / TCP 服务器）只依赖这一个函数，
/// Lexer / Parser 保持私有，将来换实现不影响调用方。
pub fn run_sql(
    sql: &str,
    db: &mut Database,
    session: &mut Session,
) -> Result<ResultSet, String> {
    let tokens = lexer::Lexer::new(sql).tokenize()?;
    let stmt = parser::Parser::new(tokens).parse()?;
    executor::execute(stmt, db, session)
}