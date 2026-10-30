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
        distinct: bool,
        columns: Vec<SelectItem>,
        table: String,
        joins: Vec<JoinClause>,
        filter: Option<Expr>,
        group_by: Vec<String>,
        /// HAVING：对"分组后的每一组"再过滤。
        /// 与 WHERE 的区别：WHERE 在分组前、作用于原始行；HAVING 在分组后、作用于每组的聚合值。
        /// 注意 None ≠ "无条件"，而是"没写 HAVING"；写了就一定在 aggregate 分支执行。
        having: Option<Expr>,
        order_by: Vec<OrderByItem>,
        
        /// LIMIT n；None = 不限。用 Option 而不是 0 表示"不限"，
        /// 因为 LIMIT 0 是合法语句，含义是"返回 0 行"，两者不能混。
        limit: Option<usize>,
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

/// ORDER BY 的一项：排序键 + 方向
#[derive(Debug, Clone, PartialEq)]
pub struct OrderByItem {
    /// 排序键用 Expr 而不是 String，是为了让 `ORDER BY SUM(amount)`
    /// 和 `HAVING SUM(amount) > 100` 共用同一套表达式解析——
    /// 同一个 SQL 概念（聚合调用）在 AST 里只有一种表示。
    pub expr: Expr,
    pub asc: bool,   // true = ASC（默认），false = DESC
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
    /// UNIQUE 约束（与主键互斥：主键已隐含唯一）
    pub unique: bool,
    /// DEFAULT 值；None = 没写 DEFAULT。目前只支持字面量（数字 / 字符串）
    pub default: Option<LitValue>,
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
    /// IS NULL / IS NOT NULL——NULL 参与比较永远是 unknown，只能这样单独判断
    IsNull { expr: Box<Expr>, negated: bool },
    /// NOT expr——取反；unknown 取反后仍是 unknown（三值逻辑）
    Not(Box<Expr>),
    /// 表达式里的聚合函数调用：HAVING SUM(amount) > 100 的左边。
    /// 只允许出现在 HAVING 里；WHERE 里写 SUM(...) 会在执行期报错
    /// （语义上就不成立：WHERE 求值时还没有"组"）。
    Aggregate { func: AggFunc, column: String },
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