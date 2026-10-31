// src/sql/token.rs

/// SQL 词法单元
#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    // ===== 关键字 =====
    Select, From, Where,
    Insert, Into, Values,
    Update, Set,
    Delete,
    Create, Table, Drop,
    And, Or, Not, Like, Is,
    Join, On,
    Group, By, Order, Asc, Desc, Limit,Having,
    Begin, Commit, Rollback,
    Int, Varchar, Primary, Key, Null, As, Distinct,
    // 列约束关键字
    Unique, Default,
    // 聚合函数
    Sum, Count, Avg, Min, Max,
    // 访问控制（DCL）
    User, Password, Login, Grant, Revoke, To, All,

    Database, Use,

    // ===== 标识符与字面量 =====
    Ident(String),       // 表名、列名等用户自定义名
    Number(i64),         // 整数字面量
    Str(String),         // 字符串字面量（单引号或双引号均可）

    // ===== 符号 =====
    LParen,   RParen,
    Comma,    Semicolon,
    Star,     Dot,
    Eq,       Neq,
    Lt,       Le,
    Gt,       Ge,

    // ===== 哨兵 =====
    Eof,                  // 输入结束标记
}