// src/sql/parser.rs

use crate::sql::token::Token;
use crate::sql::ast::*;

pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, pos: 0 }
    }

    /// 主入口
    pub fn parse(&mut self) -> Result<Statement, String> {
        let stmt = self.parse_statement()?;
        // 语句解析完后，允许一个可选的 Semicolon，然后必须到 Eof
        if self.peek() == &Token::Semicolon { self.advance(); }
        if self.peek() != &Token::Eof {
            return Err(format!("expected end of statement, got {:?}", self.peek()));
        }
        Ok(stmt)
    }

    // ===== 分发器 =====
    fn parse_statement(&mut self) -> Result<Statement, String> {
        match self.peek().clone() {
            Token::Select  => self.parse_select(),
            Token::Insert  => self.parse_insert(),
            Token::Update  => self.parse_update(),
            Token::Delete  => self.parse_delete(),
            Token::Create  => self.parse_create(),
            Token::Drop    => self.parse_drop_table(),
            Token::Begin   => { self.advance(); Ok(Statement::Begin) }
            Token::Commit  => { self.advance(); Ok(Statement::Commit) }
            Token::Grant   => self.parse_grant(),    // ← 新增
            Token::Revoke  => self.parse_revoke(),   // ← 新增
            Token::Login   => self.parse_login(),    // ← 新增
            Token::Use     => self.parse_use(),      // ← 新增
            Token::Rollback => { self.advance(); Ok(Statement::Rollback) }
            tok => Err(format!("expected statement keyword, got {:?}", tok)),
        }
    }

    // ===== 辅助方法 =====
    fn peek(&self) -> &Token {
        self.tokens.get(self.pos).unwrap_or(&Token::Eof)
    }

    fn advance(&mut self) -> Token {
        let tok = self.tokens.get(self.pos).cloned().unwrap_or(Token::Eof);
        self.pos += 1;
        tok
    }

    fn expect(&mut self, expected: &Token) -> Result<(), String> {
        if self.peek() == expected {
            self.advance();
            Ok(())
        } else {
            Err(format!("expected {:?}, got {:?}", expected, self.peek()))
        }
    }

    /// 解析一个标识符（表名、列名）
    fn expect_ident(&mut self) -> Result<String, String> {
        match self.advance() {
            Token::Ident(s) => Ok(s),
            tok => Err(format!("expected identifier, got {:?}", tok)),
        }
    }

    fn parse_grant(&mut self) -> Result<Statement, String> {
        self.expect(&Token::Grant)?;              // dispatch 只 peek 没吃，这里补上
        let privs = self.parse_priv_list()?;      // 权限列表
        self.expect(&Token::On)?;
        let table = self.expect_ident()?;
        self.expect(&Token::To)?;
        let user = self.expect_ident()?;
        Ok(Statement::Grant { privs, table, user })
    }

    fn parse_revoke(&mut self) -> Result<Statement, String> {
        self.expect(&Token::Revoke)?;
        let privs = self.parse_priv_list()?;
        self.expect(&Token::On)?;
        let table = self.expect_ident()?;
        self.expect(&Token::From)?;
        let user = self.expect_ident()?;
        Ok(Statement::Revoke { privs, table, user })
    }

    fn parse_create_user(&mut self) -> Result<Statement, String> {
        self.expect(&Token::Create)?;
        self.expect(&Token::User)?;
        let name = self.expect_ident()?;         // alice → Token::Ident
        self.expect(&Token::Password)?;
        let password = self.expect_password()?;  // 's3cret' → Token::Str
        Ok(Statement::CreateUser { name, password })
    }
    fn expect_password(&mut self) -> Result<String, String> {
        match self.advance() {
            Token::Str(s) => Ok(s),
            tok => Err(format!("expected password string literal, got {:?}", tok)),
        }
    }

    // ===== TODO：你来写以下函数 =====
    // 先做最简单的版本（不含 WHERE / JOIN / GROUP BY），跑通基本流程

    fn parse_select(&mut self) -> Result<Statement, String> {
        self.expect(&Token::Select)?;
        // DISTINCT 紧跟在 SELECT 后面，只吃一个 token，
        // 之后还是普通的列列表（DISTINCT 不是"选择项"，是整条 SELECT 的修饰）。
        let distinct = if self.peek() == &Token::Distinct {
            self.advance();
            true
        } else {
            false
        };
       
        let columns = self.parse_select_items()?;
        self.expect(&Token::From)?;
        let table = self.expect_ident()?;
        // JOIN 子句（可选，支持 a JOIN b ON ... JOIN c ON ... 链式）
        let joins = self.parse_joins()?;
        // WHERE 在 JOIN 之后
        // WHERE 在 JOIN 之后
        // WHERE 在 JOIN 之后
        let filter = self.parse_where()?;
        // GROUP BY 在 WHERE 之后、HAVING 之前
        let group_by = self.parse_group_by()?;
        // HAVING 在 GROUP BY 之后、ORDER BY 之前（标准子句顺序）
        let having = self.parse_having()?;
        // ORDER BY（可选，支持多列 + ASC/DESC）
        let order_by = self.parse_order_by()?;
        // LIMIT 是整条 SELECT 的最后一个子句，必须在 ORDER BY 之后解析
        let limit = self.parse_limit()?;
        Ok(Statement::Select {
            distinct, columns, table, filter, joins, group_by, having, order_by, limit,
        })
    }

    /// 解析 `JOIN <table> ON <a.col> = <b.col>`，支持连续多个 JOIN。
    /// 当前只支持等值连接：任务书里的"多表连接"用等值已经够，
    /// 非等值/多条件 AND 需要把 JoinClause 换成 Expr，属于后续扩展。
    fn parse_joins(&mut self) -> Result<Vec<JoinClause>, String> {
        let mut joins = Vec::new();
        while self.peek() == &Token::Join {
            self.advance(); // JOIN
            let table = self.expect_ident()?;
            self.expect(&Token::On)?;

            // 左边 table.col
            let lt = self.expect_ident()?;
            self.expect(&Token::Dot)?;
            let lc = self.expect_ident()?;

            self.expect(&Token::Eq)?;

            // 右边 table.col
            let rt = self.expect_ident()?;
            self.expect(&Token::Dot)?;
            let rc = self.expect_ident()?;

            joins.push(JoinClause {
                table,
                left_col: format!("{}.{}", lt, lc),
                right_col: format!("{}.{}", rt, rc),
            });
        }
        Ok(joins)
    }
    /// 解析 `GROUP BY col [, col ...]`；没有则返回空 vec。
    /// 支持 `GROUP BY users.dept` 这种限定列（JOIN 之后按某张表的列分组要用）。
    /// 不支持 HAVING —— 写了 HAVING 会停在那里，被 parse() 末尾的 Eof 校验报
    /// "expected end of statement, got Ident(\"HAVING\")"。
    fn parse_group_by(&mut self) -> Result<Vec<String>, String> {
        let mut cols = Vec::new();
        if self.peek() != &Token::Group {
            return Ok(cols);
        }
        self.advance();              // GROUP
        self.expect(&Token::By)?;    // BY

        loop {
            let mut col = self.expect_ident()?;
            if self.peek() == &Token::Dot {
                self.advance();
                col = format!("{}.{}", col, self.expect_ident()?);
            }
            cols.push(col);

            if self.peek() != &Token::Comma { break; }
            self.advance();
        }
        Ok(cols)
    }
    /// 解析 ORDER BY <键> [ASC|DESC] [, ...]；没有则返回空 vec。
    ///
    /// 排序键交给 `parse_primary`，三种写法自动都支持：
    ///   ORDER BY dept            裸列   → Expr::Column
    ///   ORDER BY users.dept      限定列 → Expr::QualifiedColumn
    ///   ORDER BY SUM(amount)     聚合   → Expr::Aggregate
    /// 顺带把原来"手写 ident + 可选 Dot"那几行删了：
    /// table.col 的解析规则只该有一处实现，就在这里复用。
    fn parse_order_by(&mut self) -> Result<Vec<OrderByItem>, String> {
        let mut items = Vec::new();
        if self.peek() != &Token::Order {
            return Ok(items);
        }
        self.advance();              // ORDER
        self.expect(&Token::By)?;    // BY

        loop {
            let expr = self.parse_primary()?;
            let asc = match self.peek() {
                Token::Asc  => { self.advance(); true }
                Token::Desc => { self.advance(); false }
                _ => true,           // 默认升序
            };
            items.push(OrderByItem { expr, asc });

            if self.peek() != &Token::Comma { break; }
            self.advance();
        }
        Ok(items)
    }


    /// 解析 `HAVING <expr>`；没有则返回 None。
    ///
    /// 条件表达式复用 WHERE 那一整套（OR/AND/NOT/比较/IS NULL/LIKE），
    /// 唯一的增量是 `SUM(...)` 这类聚合调用——由 `parse_primary` 负责。
    /// 它和 WHERE 的差别不在语法，而在"在哪一步求值、列名空间是什么"。
    fn parse_having(&mut self) -> Result<Option<Expr>, String> {
        if self.peek() == &Token::Having {
            self.advance();
            Ok(Some(self.parse_expr_or()?))
        } else {
            Ok(None)
        }
    }

    /// 解析 `LIMIT n`；没有则返回 None。
    /// n 只接受非负整数字面量：lexer 没有 '-' 分支，`LIMIT -1` 在词法阶段就报错，
    /// 走到这里只可能是写了个别的东西（比如 LIMIT 'x'）。
    fn parse_limit(&mut self) -> Result<Option<usize>, String> {
        if self.peek() != &Token::Limit {
            return Ok(None);
        }
        self.advance(); // LIMIT
        match self.advance() {
            Token::Number(n) if n >= 0 => Ok(Some(n as usize)),
            tok => Err(format!("expected non-negative integer after LIMIT, got {:?}", tok)),
        }
    }
    /// UPDATE <table> SET col = value [, col = value ...] [WHERE ...]
    fn parse_update(&mut self) -> Result<Statement, String> {
        self.expect(&Token::Update)?;
        let table = self.expect_ident()?;
        self.expect(&Token::Set)?;

        let mut assignments = Vec::new();
        loop {
            let column = self.expect_ident()?;
            self.expect(&Token::Eq)?;
            let value = self.parse_primary()?;
            assignments.push((column, value));

            if self.peek() != &Token::Comma { break; }
            self.advance();
        }

        let filter = self.parse_where()?;
        Ok(Statement::Update { table, assignments, filter })
    }

    fn parse_insert(&mut self) -> Result<Statement, String> {
        self.expect(&Token::Insert)?;
        self.expect(&Token::Into)?;
        let table = self.expect_ident()?;

        // 可选的 (col1, col2, ...) 列表
        let mut columns = Vec::new();
        if self.peek() == &Token::LParen {
            // 这里要"往前看一步"判断是 (id, name) 还是 VALUES 的 (1, 'Alice')
            // 简化：如果紧跟 VALUES 关键字 → 说明这不是列名列表
            // 但 Token 流里 LParen 出现在 VALUES 之前就是列名，之后就是值
            // 更简单的策略：先 peek 后面是不是 Values 关键字
            self.advance(); // eat LParen
            loop {
                columns.push(self.expect_ident()?);
                if self.peek() != &Token::Comma { break; }
                self.advance();
            }
            self.expect(&Token::RParen)?;
        }

        self.expect(&Token::Values)?;
        self.expect(&Token::LParen)?;
        let mut values = Vec::new();
        loop {
            values.push(self.parse_primary()?);  // 数字、字符串、NULL 都算 primary
            if self.peek() != &Token::Comma { break; }
            self.advance();
        }
        self.expect(&Token::RParen)?;

        Ok(Statement::Insert { table, columns, values })
    }

    fn parse_delete(&mut self) -> Result<Statement, String> {
        self.expect(&Token::Delete)?;
        self.expect(&Token::From)?;
        let table = self.expect_ident()?;
        let filter = self.parse_where()?;
        Ok(Statement::Delete { table, filter })
    }
    fn parse_create(&mut self) -> Result<Statement, String> {
        if self.peek_at(1) == &Token::User {
            return self.parse_create_user();
        }
        if self.peek_at(1) == &Token::Database {
            return self.parse_create_database();
        }
        self.parse_create_table()      // 它自己会 expect(Create)
    }

    /// 往前看 offset 格，不消费
    fn peek_at(&self, offset: usize) -> &Token {
        self.tokens.get(self.pos + offset).unwrap_or(&Token::Eof)
    }
    /// CREATE DATABASE shop
    fn parse_create_database(&mut self) -> Result<Statement, String> {
        self.expect(&Token::Create)?;
        self.expect(&Token::Database)?;
        let name = self.expect_ident()?;
        Ok(Statement::CreateDatabase { name })
    }

    /// USE shop
    fn parse_use(&mut self) -> Result<Statement, String> {
        self.expect(&Token::Use)?;
        let name = self.expect_ident()?;
        Ok(Statement::Use { name })
    }
    fn parse_create_table(&mut self) -> Result<Statement, String> {
        self.expect(&Token::Create)?;
        self.expect(&Token::Table)?;
        let name = self.expect_ident()?;
        self.expect(&Token::LParen)?;

        let mut columns = Vec::new();
        loop {
            let col_name = self.expect_ident()?;
            // 类型
            let data_type = match self.advance() {
                Token::Int => DataType::Int,
                Token::Varchar => {
                    self.expect(&Token::LParen)?;
                    let size = match self.advance() {
                        Token::Number(n) => n as usize,
                        tok => return Err(format!("expected varchar size, got {:?}", tok)),
                    };
                    self.expect(&Token::RParen)?;
                    DataType::Varchar(size)
                }
                tok => return Err(format!("expected type, got {:?}", tok)),
            };
            // 可选约束：PRIMARY KEY / NOT NULL / UNIQUE / DEFAULT <值>（都可以没有）
            let mut primary_key = false;
            let mut not_null = false;
            let mut unique = false;
            let mut default: Option<LitValue> = None;
            loop {
                if self.peek() == &Token::Primary {
                    self.advance();
                    self.expect(&Token::Key)?;
                    primary_key = true;
                } else if self.peek() == &Token::Not {
                    self.advance();
                    self.expect(&Token::Null)?;
                    not_null = true;
                } else if self.peek() == &Token::Unique {
                    self.advance();
                    unique = true;
                } else if self.peek() == &Token::Default {
                    self.advance();
                    // DEFAULT 后面只能跟一个字面量：数字 / 字符串（NULL 当干没写）
                    default = match self.advance() {
                        Token::Number(n) => Some(LitValue::Number(n)),
                        Token::Str(s) => Some(LitValue::Str(s)),
                        Token::Null => None,   // `DEFAULT NULL` = 没写默认值
                        tok => return Err(format!("expected literal after DEFAULT, got {:?}", tok)),
                    };
                } else {
                    break;
                }
            }
            columns.push(ColumnDef { name: col_name, data_type, primary_key, not_null, unique, default });

            if self.peek() != &Token::Comma { break; }
            self.advance();
        }

        self.expect(&Token::RParen)?;
        Ok(Statement::CreateTable { name, columns })
    }

    fn parse_drop_table(&mut self) -> Result<Statement, String> {
        // expect(Drop)
        self.expect(&Token::Drop)?;
        // expect(Table)
        self.expect(&Token::Table)?;
        // name = expect_ident()
        let name = self.expect_ident()?;
        // Ok(Statement::DropTable { name })
        Ok(Statement::DropTable { name })

    }
    /// 读一个权限关键字 → Priv。
    /// 注意这里 match 的是 Token，不是字符串：lexer 已经把 SELECT 升格成 Token::Select 了。
    fn parse_one_priv(&mut self) -> Result<Priv, String> {
        match self.advance() {
            Token::Select => Ok(Priv::Select),
            Token::Insert => Ok(Priv::Insert),
            Token::Update => Ok(Priv::Update),
            Token::Delete => Ok(Priv::Delete),
            tok => Err(format!(
                "expected privilege (SELECT/INSERT/UPDATE/DELETE), got {:?}", tok)),
        }
    }

    /// 权限列表。ALL 是语法糖，在这里就地展开成四项 —— 执行层永远见不到 "ALL"。
    fn parse_priv_list(&mut self) -> Result<Vec<Priv>, String> {
        // ALL 单独前置判断，不塞进循环：它后面不允许跟逗号
        if self.peek() == &Token::All {
            self.advance();
            return Ok(vec![Priv::Select, Priv::Insert, Priv::Update, Priv::Delete]);
        }

        let mut out = Vec::new();
        loop {
            out.push(self.parse_one_priv()?);       // 先读一个权限（会消费）
            if self.peek() != &Token::Comma { break; }  // 再回头看逗号（不消费）
            self.advance();                          // 是逗号才吃掉，继续
        }
        Ok(out)
    }
    // ===== 辅助：解析 SELECT 列表 =====
    fn parse_select_items(&mut self) -> Result<Vec<SelectItem>, String> {
        // 如果是 Star → [SelectItem::Star]
        if self.peek() == &Token::Star {
            self.advance();
            return Ok(vec![SelectItem::Star]);
        }

        let mut columns = vec![];
        loop {
            // ① 先判断这一项是不是聚合函数（关键字已在 lexer 里映射好）
            let func = match self.peek() {
                Token::Sum   => Some(AggFunc::Sum),
                Token::Count => Some(AggFunc::Count),
                Token::Avg   => Some(AggFunc::Avg),
                Token::Min   => Some(AggFunc::Min),
                Token::Max   => Some(AggFunc::Max),
                _ => None,
            };

            if let Some(func) = func {
                self.advance();                  // 吃掉函数名
                self.expect(&Token::LParen)?;

                // COUNT(*) 是特例：数"行数"，不针对某一列。
                // SelectItem::Aggregate 的 column 是 String（不是 Option），
                // 这里用 "*" 当哨兵，Step 2 的 executor 看到 "*" 就数行、看到列名就数非 NULL。
                let column = if self.peek() == &Token::Star {
                    if func != AggFunc::Count {
                        return Err(format!("{:?}(*) is not allowed, only COUNT(*)", func));
                    }
                    self.advance();
                    "*".to_string()
                } else {
                    // 允许 SUM(orders.amount) 这种限定列
                    let mut name = self.expect_ident()?;
                    if self.peek() == &Token::Dot {
                        self.advance();
                        name = format!("{}.{}", name, self.expect_ident()?);
                    }
                    name
                };

                self.expect(&Token::RParen)?;
                columns.push(SelectItem::Aggregate { func, column });
            } else {
                // ② 普通列：name 或 table.name（与原来一致）
                let first = self.expect_ident()?;
                if self.peek() == &Token::Dot {
                    self.advance(); // eat '.'
                    let col = self.expect_ident()?;
                    columns.push(SelectItem::TableColumn { table: first, col });
                } else {
                    columns.push(SelectItem::Column(first));
                }
            }

            if self.peek() != &Token::Comma { break; }
            self.advance();
        }
        Ok(columns)
    }

    // ===== WHERE 表达式解析（递归 + 优先级） =====

    /// 顶层入口：遇到 WHERE 关键字后调用
    fn parse_where(&mut self) -> Result<Option<Expr>, String> {
        if self.peek() == &Token::Where {
            self.advance();
            Ok(Some(self.parse_expr_or()?))
        } else {
            Ok(None)
        }
    }

    /// OR 层（最低优先级）
    fn parse_expr_or(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_expr_and()?;
        while self.peek() == &Token::Or {
            self.advance();
            let right = self.parse_expr_and()?;
            left = Expr::Or(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    /// AND 层
    fn parse_expr_and(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_expr_not()?;
        while self.peek() == &Token::And {
            self.advance();
            let right = self.parse_expr_not()?;
            left = Expr::And(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    /// NOT 层：优先级卡在 AND 与比较之间，`NOT a = 1 AND b = 2` 读作 `(NOT a=1) AND b=2`
    fn parse_expr_not(&mut self) -> Result<Expr, String> {
        if self.peek() == &Token::Not {
            self.advance();
            // 递归一层，支持 NOT NOT x（虽然没什么用，但语法上不能说谎）
            let inner = self.parse_expr_not()?;
            return Ok(Expr::Not(Box::new(inner)));
        }
        self.parse_comparison()
    }

    /// 比较层（最高优先级）：column op value, 或 column LIKE 'pattern'
    fn parse_comparison(&mut self) -> Result<Expr, String> {
        let left = self.parse_primary()?;
        let tok = self.peek().clone();
        match tok {
            Token::Eq | Token::Neq | Token::Lt | Token::Le | Token::Gt | Token::Ge => {
                self.advance();
                let op = match tok {
                    Token::Eq => BinOp::Eq,
                    Token::Neq => BinOp::Neq,
                    Token::Lt => BinOp::Lt,
                    Token::Le => BinOp::Le,
                    Token::Gt => BinOp::Gt,
                    Token::Ge => BinOp::Ge,
                    _ => unreachable!(),
                };
                let right = self.parse_primary()?;
                Ok(Expr::BinOp { left: Box::new(left), op, right: Box::new(right) })
            }
            Token::Like => {
                self.advance();
                // 右边必须是一个字符串字面量作为 pattern
                if let Token::Str(pattern) = self.advance() {
                    Ok(Expr::Like { expr: Box::new(left), pattern })
                } else {
                    Err("expected string pattern after LIKE".to_string())
                }
            }
            Token::Is => {
                self.advance();
                // IS NULL / IS NOT NULL
                let negated = if self.peek() == &Token::Not {
                    self.advance();
                    true
                } else {
                    false
                };
                self.expect(&Token::Null)?;
                Ok(Expr::IsNull { expr: Box::new(left), negated })
            }
            _ => Ok(left), // 没有比较运算符，直接返回左操作数
        }
    }

    /// 解析"原子"表达式：列名、数字、字符串、NULL、聚合调用
    fn parse_primary(&mut self) -> Result<Expr, String> {
        match self.advance() {
            // 限定列 table.col：往前多看一个 Dot 才能区分 Column 和 QualifiedColumn
            Token::Ident(name) => {
                if self.peek() == &Token::Dot {
                    self.advance(); // eat '.'
                    let col = self.expect_ident()?;
                    Ok(Expr::QualifiedColumn { table: name, col })
                } else {
                    Ok(Expr::Column(name))
                }
            }
            // 聚合调用：函数名 token 已经在 advance() 里被吃掉了
            Token::Sum   => self.parse_agg_call(AggFunc::Sum),
            Token::Count => self.parse_agg_call(AggFunc::Count),
            Token::Avg   => self.parse_agg_call(AggFunc::Avg),
            Token::Min   => self.parse_agg_call(AggFunc::Min),
            Token::Max   => self.parse_agg_call(AggFunc::Max),
            Token::Number(n) => Ok(Expr::Literal(LitValue::Number(n))),
            Token::Str(s) => Ok(Expr::Literal(LitValue::Str(s))),
            Token::Null => Ok(Expr::Literal(LitValue::Null)),
            tok => Err(format!("unexpected token in expression: {:?}", tok)),
        }
    }

    /// 解析聚合调用的剩余部分 `( column )` 或 `( * )`。
    /// 函数名 token 已经被调用方吃掉了，这里从 `(` 开始。
    fn parse_agg_call(&mut self, func: AggFunc) -> Result<Expr, String> {
        self.expect(&Token::LParen)?;
        let column = if self.peek() == &Token::Star {
            // 只有 COUNT(*) 合法：SUM(*) 数什么？
            if func != AggFunc::Count {
                return Err(format!("{:?}(*) is not allowed, only COUNT(*)", func));
            }
            self.advance();
            "*".to_string()
        } else {
            // 允许 SUM(orders.amount) 这种限定列
            let mut name = self.expect_ident()?;
            if self.peek() == &Token::Dot {
                self.advance();
                name = format!("{}.{}", name, self.expect_ident()?);
            }
            name
        };
        self.expect(&Token::RParen)?;
        Ok(Expr::Aggregate { func, column })
    }
    fn parse_login(&mut self) -> Result<Statement, String> {
        self.expect(&Token::Login)?;
        let name = self.expect_ident()?;
        self.expect(&Token::Password)?;        // ← 你漏的就是这行
        let password = self.expect_password()?;
        Ok(Statement::Login { name, password })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sql::lexer::Lexer;

    fn parse_sql(sql: &str) -> Result<Statement, String> {
        let tokens = Lexer::new(sql).tokenize()?;
        Parser::new(tokens).parse()
    }

    #[test]
    fn test_create_table_unique_and_default() {
        let s = parse_sql(
            "CREATE TABLE t (code VARCHAR(20) UNIQUE, qty INT DEFAULT 0, name VARCHAR(10) NOT NULL DEFAULT 'x')").unwrap();
        match s {
            Statement::CreateTable { columns, .. } => {
                assert_eq!(columns.len(), 3);
                // code VARCHAR(20) UNIQUE
                assert!(columns[0].unique);
                assert_eq!(columns[0].default, None);
                // qty INT DEFAULT 0
                assert!(!columns[1].unique);
                assert_eq!(columns[1].default, Some(LitValue::Number(0)));
                // name NOT NULL DEFAULT 'x'
                assert!(columns[2].not_null);
                assert_eq!(columns[2].default, Some(LitValue::Str("x".into())));
            }
            other => panic!("expected CreateTable, got {:?}", other),
        }
    }

    #[test]
    fn test_select_star_from() {
        let stmt = parse_sql("SELECT * FROM users").unwrap();
        assert_eq!(stmt, Statement::Select {
            distinct: false,
            columns: vec![SelectItem::Star],
            table: "users".into(),
            filter: None,
            joins: vec![],
            group_by: vec![],
            order_by: vec![],
            limit: None,
            having: None,
        });
    }

    #[test]
    fn test_select_columns() {
        let stmt = parse_sql("SELECT name, age FROM users").unwrap();
        assert_eq!(stmt, Statement::Select {
            distinct: false,
            columns: vec![
                SelectItem::Column("name".into()),
                SelectItem::Column("age".into()),
            ],
            table: "users".into(),
            filter: None,
            joins: vec![],
            group_by: vec![],
            order_by: vec![],
            limit: None,
            having: None,   
        });
    }

    #[test]
    fn test_drop_table() {
        let stmt = parse_sql("DROP TABLE inventory_logs").unwrap();
        assert_eq!(stmt, Statement::DropTable { name: "inventory_logs".into() });
    }

    #[test]
    fn test_with_semicolon() {
        let stmt = parse_sql("SELECT * FROM t;").unwrap();
        assert_eq!(stmt, Statement::Select {
            distinct: false,
            columns: vec![SelectItem::Star],
            table: "t".into(),
            filter: None,
            joins: vec![],
            group_by: vec![],
            order_by: vec![],
            limit: None,
            having: None,   
        });
    }

    #[test]
    fn test_error_missing_from() {
        let err = parse_sql("SELECT * users").unwrap_err();
        assert!(err.contains("expected"), "should say 'expected From': {}", err);
    }

    #[test]
    fn test_where_simple_gt() {
        let stmt = parse_sql("SELECT * FROM users WHERE age > 18").unwrap();
        if let Statement::Select { filter: Some(f), .. } = stmt {
            assert_eq!(f, Expr::BinOp {
                left: Box::new(Expr::Column("age".into())),
                op: BinOp::Gt,
                right: Box::new(Expr::Literal(LitValue::Number(18))),
            });
        } else { panic!("expected filter"); }
    }

    #[test]
    fn test_where_and() {
        let stmt = parse_sql("SELECT * FROM t WHERE id = 1 AND name = 'x'").unwrap();
        if let Statement::Select { filter: Some(Expr::And(l, r)), .. } = stmt {
            assert_eq!(*l, Expr::BinOp {
                left: Box::new(Expr::Column("id".into())),
                op: BinOp::Eq,
                right: Box::new(Expr::Literal(LitValue::Number(1))),
            });
            assert_eq!(*r, Expr::BinOp {
                left: Box::new(Expr::Column("name".into())),
                op: BinOp::Eq,
                right: Box::new(Expr::Literal(LitValue::Str("x".into()))),
            });
        } else { panic!("expected AND"); }
    }

    #[test]
    fn test_where_or_and_precedence() {
        // a=1 OR b=2 AND c=3 → Or(a=1, And(b=2, c=3))
        let stmt = parse_sql("SELECT * FROM t WHERE a = 1 OR b = 2 AND c = 3").unwrap();
        if let Statement::Select { filter: Some(Expr::Or(l, r)), .. } = stmt {
            // 左边是 a=1
            assert!(matches!(*l, Expr::BinOp { op: BinOp::Eq, .. }));
            // 右边是 And 而不是 BinOp！这是优先级验证的核心
            assert!(matches!(*r, Expr::And(..)), "right should be And, got {:?}", r);
        } else { panic!("expected Or at top"); }
    }

    #[test]
    fn test_where_like() {
        let stmt = parse_sql("SELECT * FROM t WHERE name LIKE '%张%'").unwrap();
        if let Statement::Select { filter: Some(Expr::Like { expr, pattern }), .. } = stmt {
            assert_eq!(*expr, Expr::Column("name".into()));
            assert_eq!(pattern, "%张%");
        } else { panic!("expected LIKE"); }
    }

    #[test] fn test_delete_where() {
        let s = parse_sql("DELETE FROM users WHERE id = 1").unwrap();
        assert!(matches!(s, Statement::Delete { .. }));
    }
    #[test] fn test_insert_full() {
        let s = parse_sql("INSERT INTO users (id, name) VALUES (1, 'Alice')").unwrap();
        match s {
            Statement::Insert { table, columns, values } => {
                assert_eq!(table, "users");
                assert_eq!(columns, vec!["id".to_string(), "name".to_string()]);
                assert_eq!(values, vec![
                    Expr::Literal(LitValue::Number(1)),
                    Expr::Literal(LitValue::Str("Alice".to_string())),
                ]);
            }
            other => panic!("expected Insert, got {:?}", other),
        }
    }

    #[test] fn test_create_table() {
        let s = parse_sql("CREATE TABLE users (id INT PRIMARY KEY, name VARCHAR(50))").unwrap();
        match s {
            Statement::CreateTable { name, columns } => {
                assert_eq!(name, "users");
                assert_eq!(columns.len(), 2);
                assert_eq!(columns[0].name, "id");
                assert_eq!(columns[0].data_type, DataType::Int);
                assert!(columns[0].primary_key);
                assert_eq!(columns[1].name, "name");
                assert_eq!(columns[1].data_type, DataType::Varchar(50));
                assert!(!columns[1].primary_key);
            }
            other => panic!("expected CreateTable, got {:?}", other),
        }
    }

    #[test]
    fn test_update_with_where() {
        let s = parse_sql("UPDATE users SET name = 'Bob', age = 20 WHERE id = 1").unwrap();
        match s {
            Statement::Update { table, assignments, filter } => {
                assert_eq!(table, "users");
                assert_eq!(assignments, vec![
                    ("name".to_string(), Expr::Literal(LitValue::Str("Bob".into()))),
                    ("age".to_string(), Expr::Literal(LitValue::Number(20))),
                ]);
                assert!(matches!(filter, Some(Expr::BinOp { op: BinOp::Eq, .. })));
            }
            other => panic!("expected Update, got {:?}", other),
        }
    }

    #[test]
    fn test_order_by_multiple() {
        let s = parse_sql("SELECT * FROM t ORDER BY a DESC, b").unwrap();
        match s {
            Statement::Select { order_by, .. } => {
                assert_eq!(order_by, vec![
                    OrderByItem { expr: Expr::Column("a".into()), asc: false },
                    OrderByItem { expr: Expr::Column("b".into()), asc: true },
                ]);
            }
            other => panic!("expected Select, got {:?}", other),
        }
    }

    #[test]
    fn test_where_not() {
        // NOT 卡在 AND 与比较之间：`NOT age > 18` 取反的是整个比较，不是 age
        let s = parse_sql("SELECT * FROM t WHERE NOT age > 18").unwrap();
        match s {
            Statement::Select { filter: Some(Expr::Not(inner)), .. } => {
                assert!(matches!(*inner, Expr::BinOp { op: BinOp::Gt, .. }), "got {:?}", inner);
            }
            other => panic!("expected NOT, got {:?}", other),
        }

        // `NOT a IS NULL AND b = 2` → And(Not(IsNull), b=2)
        let s = parse_sql("SELECT * FROM t WHERE NOT a IS NULL AND b = 2").unwrap();
        match s {
            Statement::Select { filter: Some(Expr::And(l, r)), .. } => {
                assert!(matches!(*l, Expr::Not(_)), "got {:?}", l);
                assert!(matches!(*r, Expr::BinOp { op: BinOp::Eq, .. }), "got {:?}", r);
            }
            other => panic!("expected And, got {:?}", other),
        }
    }

    #[test]
    fn test_insert_null_value() {
        let s = parse_sql("INSERT INTO t (id, name) VALUES (1, NULL)").unwrap();
        match s {
            Statement::Insert { values, .. } => {
                assert_eq!(values, vec![
                    Expr::Literal(LitValue::Number(1)),
                    Expr::Literal(LitValue::Null),
                ]);
            }
            other => panic!("expected Insert, got {:?}", other),
        }
    }

    #[test]
    fn test_is_null_and_is_not_null() {
        let s = parse_sql("SELECT * FROM t WHERE remark IS NULL").unwrap();
        match s {
            Statement::Select { filter: Some(Expr::IsNull { expr, negated }), .. } => {
                assert_eq!(*expr, Expr::Column("remark".into()));
                assert!(!negated);
            }
            other => panic!("expected Select with IS NULL, got {:?}", other),
        }

        let s = parse_sql("SELECT * FROM t WHERE remark IS NOT NULL").unwrap();
        match s {
            Statement::Select { filter: Some(Expr::IsNull { negated, .. }), .. } => {
                assert!(negated);
            }
            other => panic!("expected Select with IS NOT NULL, got {:?}", other),
        }
    }

    #[test]
    fn test_update_set_null() {
        let s = parse_sql("UPDATE t SET name = NULL WHERE id = 1").unwrap();
        match s {
            Statement::Update { assignments, .. } => {
                assert_eq!(assignments, vec![
                    ("name".to_string(), Expr::Literal(LitValue::Null)),
                ]);
            }
            other => panic!("expected Update, got {:?}", other),
        }
    }
    #[test]
    fn test_qualified_column_in_select_and_where() {
        let stmt = parse_sql("SELECT users.id, users.name FROM users WHERE users.id = 1").unwrap();
        match stmt {
            Statement::Select { columns, filter: Some(Expr::BinOp { left, op, .. }), .. } => {
                assert_eq!(columns, vec![
                    SelectItem::TableColumn { table: "users".into(), col: "id".into() },
                    SelectItem::TableColumn { table: "users".into(), col: "name".into() },
                ]);
                assert_eq!(*left, Expr::QualifiedColumn { table: "users".into(), col: "id".into() });
                assert_eq!(op, BinOp::Eq);
            }
            other => panic!("expected qualified columns, got {:?}", other),
        }
    }

    #[test]
    fn test_join_one_clause() {
        let stmt = parse_sql(
            "SELECT * FROM logs JOIN materials ON logs.material_code = materials.material_code").unwrap();
        assert_eq!(stmt, Statement::Select {
            distinct: false,
            columns: vec![SelectItem::Star],
            table: "logs".into(),
            filter: None,
            joins: vec![JoinClause {
                table: "materials".into(),
                left_col: "logs.material_code".into(),
                right_col: "materials.material_code".into(),
            }],
            group_by: vec![],
            order_by: vec![],
            limit: None,
            having: None,
        });
    }
    #[test]
    fn test_aggregate_and_group_by() {
        let s = parse_sql(
            "SELECT dept, COUNT(*), SUM(amount), AVG(price) FROM sales GROUP BY dept").unwrap();
        match s {
            Statement::Select { columns, group_by, .. } => {
                assert_eq!(group_by, vec!["dept".to_string()]);
                assert_eq!(columns, vec![
                    SelectItem::Column("dept".into()),
                    SelectItem::Aggregate { func: AggFunc::Count, column: "*".into() },
                    SelectItem::Aggregate { func: AggFunc::Sum, column: "amount".into() },
                    SelectItem::Aggregate { func: AggFunc::Avg, column: "price".into() },
                ]);
            }
            other => panic!("expected Select, got {:?}", other),
        }
    }

    #[test]
    fn test_agg_star_only_for_count() {
        // SUM(*) 是非法语法，错误信息要能让人看懂
        let err = parse_sql("SELECT SUM(*) FROM t").unwrap_err();
        assert!(err.contains("COUNT(*)"), "got: {}", err);
    }
    #[test]
    fn test_limit() {
        let s = parse_sql("SELECT * FROM t LIMIT 3").unwrap();
        match s {
            Statement::Select { limit, .. } => assert_eq!(limit, Some(3)),
            other => panic!("got {:?}", other),
        }
        // LIMIT 0 是合法语句，必须解析成 Some(0)，不能变成 None
        let s = parse_sql("SELECT * FROM t LIMIT 0").unwrap();
        match s {
            Statement::Select { limit, .. } => assert_eq!(limit, Some(0)),
            other => panic!("got {:?}", other),
        }
    }
    #[test]
    fn test_having_parses_aggregate() {
        // ① 聚合出现在条件左边：SUM(amount) > 100
        let s = parse_sql(
            "SELECT dept, SUM(amount) FROM t GROUP BY dept HAVING SUM(amount) > 100").unwrap();
        match s {
            Statement::Select { having: Some(h), .. } => assert_eq!(
                h,
                Expr::BinOp {
                    left: Box::new(Expr::Aggregate { func: AggFunc::Sum, column: "amount".into() }),
                    op: BinOp::Gt,
                    right: Box::new(Expr::Literal(LitValue::Number(100))),
                }
            ),
            other => panic!("got {:?}", other),
        }

        // ② COUNT(*) 也要能在条件里出现
        let s = parse_sql(
            "SELECT dept FROM t GROUP BY dept HAVING COUNT(*) > 1").unwrap();
        match s {
            Statement::Select { having: Some(Expr::BinOp { left, .. }), .. } => {
                assert_eq!(*left, Expr::Aggregate { func: AggFunc::Count, column: "*".into() });
            }
            other => panic!("got {:?}", other),
        }

        // ③ 没有 HAVING 时是 None，不是"空条件"
        let s = parse_sql("SELECT dept FROM t GROUP BY dept").unwrap();
        match s {
            Statement::Select { having, .. } => assert_eq!(having, None),
            other => panic!("got {:?}", other),
        }
    }
    #[test]
    fn test_order_by_aggregate_and_qualified() {
        // ① 排序键是聚合调用
        let s = parse_sql(
            "SELECT dept, SUM(amount) FROM t GROUP BY dept ORDER BY SUM(amount) DESC").unwrap();
        match s {
            Statement::Select { order_by, .. } => assert_eq!(order_by, vec![
                OrderByItem {
                    expr: Expr::Aggregate { func: AggFunc::Sum, column: "amount".into() },
                    asc: false,
                },
            ]),
            other => panic!("got {:?}", other),
        }

        // ② 限定列不能因为换了实现就退化
        let s = parse_sql("SELECT * FROM t ORDER BY t.id DESC, name").unwrap();
        match s {
            Statement::Select { order_by, .. } => assert_eq!(order_by, vec![
                OrderByItem {
                    expr: Expr::QualifiedColumn { table: "t".into(), col: "id".into() },
                    asc: false,
                },
                OrderByItem { expr: Expr::Column("name".into()), asc: true },
            ]),
            other => panic!("got {:?}", other),
        }
    }
    #[test]
    fn test_distinct_parses() {
        let s = parse_sql("SELECT DISTINCT dept FROM t").unwrap();
        match s {
            Statement::Select { distinct, columns, .. } => {
                assert!(distinct);
                assert_eq!(columns, vec![SelectItem::Column("dept".into())]);
            }
            other => panic!("got {:?}", other),
        }

        // 没写 DISTINCT 必须是 false（防止"默认 true"这种低级错）
        let s = parse_sql("SELECT dept FROM t").unwrap();
        match s {
            Statement::Select { distinct, .. } => assert!(!distinct),
            other => panic!("got {:?}", other),
        }
    }
    #[test]
    fn test_access_control_parses() {
        // ① 单个权限
        match parse_sql("GRANT SELECT ON users TO alice").unwrap() {
            Statement::Grant { privs, table, user } => {
                assert_eq!(privs, vec![Priv::Select]);
                assert_eq!(table, "users");
                assert_eq!(user, "alice");
            }
            other => panic!("expected Grant, got {:?}", other),
        }

        // ② 多个权限 + 顺序
        match parse_sql("GRANT SELECT, INSERT ON users TO alice").unwrap() {
            Statement::Grant { privs, .. } => assert_eq!(privs, vec![Priv::Select, Priv::Insert]),
            other => panic!("expected Grant, got {:?}", other),
        }

        // ③ ALL 在语法层展开成 4 项
        match parse_sql("GRANT ALL ON users TO alice").unwrap() {
            Statement::Grant { privs, .. } => assert_eq!(
                privs,
                vec![Priv::Select, Priv::Insert, Priv::Update, Priv::Delete]
            ),
            other => panic!("expected Grant, got {:?}", other),
        }

        // ④ REVOKE 用的是 FROM
        assert!(matches!(
        parse_sql("REVOKE SELECT ON users FROM alice").unwrap(),
        Statement::Revoke { .. }
    ));

        // ⑤ CREATE USER：用户名是 Ident、口令是 Str —— 这条就是抓你刚才那个 bug 的
        assert_eq!(
            parse_sql("CREATE USER alice PASSWORD 's3cret'").unwrap(),
            Statement::CreateUser { name: "alice".into(), password: "s3cret".into() }
        );

        // ⑥ 口令不加引号 → 语法层拒绝（Q7 的落地）
        assert!(parse_sql("CREATE USER bob PASSWORD 123").is_err());
    }
}