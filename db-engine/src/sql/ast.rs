// src/sql/ast.rs

/// 一条 SQL 语句解析后的顶层表示
#[derive(Debug, Clone, PartialEq)]
pub enum Statement {
    /// CREATE TABLE users (id INT PRIMARY KEY, name VARCHAR(50))
    CreateTable {
        name: String,
        columns: Vec<ColumnDef>,
    },
    /// DROP TABLE users
    DropTable {
        name: String,
    },
    /// INSERT INTO users (id, name) VALUES (1, 'Alice')
    Insert {
        table: String,
        columns: Vec<String>,       // 空 = 全列
        values: Vec<Expr>,
    },
    /// SELECT name, age FROM users WHERE age > 18
    Select {
        columns: Vec<SelectItem>,
        table: String,
        joins: Vec<JoinClause>,
        filter: Option<Expr>,
        group_by: Vec<String>,
    },
    /// UPDATE users SET name = 'Bob' WHERE id = 1
    Update {
        table: String,
        assignments: Vec<(String, Expr)>,
        filter: Option<Expr>,
    },
    /// DELETE FROM users WHERE id = 1
    Delete {
        table: String,
        filter: Option<Expr>,
    },
    /// BEGIN / COMMIT / ROLLBACK
    Begin,
    Commit,
    Rollback,
}

/// SELECT 列表里的每一项
#[derive(Debug, Clone, PartialEq)]
pub enum SelectItem {
    Star,                                    // *
    Column(String),                          // name
    TableColumn { table: String, col: String }, // users.name
    Aggregate { func: AggFunc, column: String }, // SUM(qty)
}

/// 聚合函数类型
#[derive(Debug, Clone, PartialEq)]
pub enum AggFunc {
    Sum, Count, Avg, Min, Max,
}

/// JOIN 子句
#[derive(Debug, Clone, PartialEq)]
pub struct JoinClause {
    pub table: String,
    pub left_col: String,    // logs.material_code
    pub right_col: String,   // materials.material_code
}

/// 列定义（CREATE TABLE 里用）
#[derive(Debug, Clone, PartialEq)]
pub struct ColumnDef {
    pub name: String,
    pub data_type: DataType,
    pub primary_key: bool,
    pub not_null: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DataType {
    Int,
    Varchar(usize),
}

/// 表达式（WHERE 条件、VALUES 里的值、SET 右边的值等）
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Column(String),                              // 列引用: age
    QualifiedColumn { table: String, col: String }, // 限定列: users.age
    Literal(LitValue),                           // 字面量
    BinOp { left: Box<Expr>, op: BinOp, right: Box<Expr> }, // 比较: age > 18
    And(Box<Expr>, Box<Expr>),                   // 逻辑与
    Or(Box<Expr>, Box<Expr>),                    // 逻辑或
    Like { expr: Box<Expr>, pattern: String },   // LIKE '%张%'
}

#[derive(Debug, Clone, PartialEq)]
pub enum LitValue {
    Number(i64),
    Str(String),
    Null,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BinOp {
    Eq, Neq, Lt, Le, Gt, Ge,
}