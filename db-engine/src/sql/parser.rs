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
            Token::Delete  => self.parse_delete(),
            Token::Create  => self.parse_create_table(),
            Token::Drop    => self.parse_drop_table(),
            Token::Begin   => { self.advance(); Ok(Statement::Begin) }
            Token::Commit  => { self.advance(); Ok(Statement::Commit) }
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

    fn expect_ident(&mut self) -> Result<String, String> {
        match self.advance() {
            Token::Ident(s) => Ok(s),
            tok => Err(format!("expected identifier, got {:?}", tok)),
        }
    }

    // ===== TODO：你来写以下函数 =====
    // 先做最简单的版本（不含 WHERE / JOIN / GROUP BY），跑通基本流程

    fn parse_select(&mut self) -> Result<Statement, String> {
        // expect(Select)
        self.expect(&Token::Select)?;
        let columns = self.parse_select_items()?;
        self.expect(&Token::From)?;
        // table = expect_ident()
        let table = self.expect_ident()?;
        // filter = None (先不做 WHERE)
        let filter = self.parse_where()?;
        // joins = vec![], group_by = vec![]
        // Ok(Statement::Select { ... })
        Ok(Statement::Select { columns, table, filter, joins: vec![], group_by: vec![] })

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
            // 可选约束：PRIMARY KEY / NOT NULL（都可以没有）
            let mut primary_key = false;
            let mut not_null = false;
            loop {
                if self.peek() == &Token::Primary {
                    self.advance();
                    self.expect(&Token::Key)?;
                    primary_key = true;
                } else if self.peek() == &Token::Not {
                    self.advance();
                    self.expect(&Token::Null)?;
                    not_null = true;
                } else {
                    break;
                }
            }
            columns.push(ColumnDef { name: col_name, data_type, primary_key, not_null });

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

    // ===== 辅助：解析 SELECT 列表 =====
    fn parse_select_items(&mut self) -> Result<Vec<SelectItem>, String> {
        // 如果是 Star → [SelectItem::Star]
        if self.peek() == &Token::Star {
            self.advance();
            return Ok(vec![SelectItem::Star]);
        }
        // 否则：逗号分隔的 Ident 列表 → [Column(name), ...]
        let mut columns = vec![];
        while self.peek() != &Token::Eof {
            columns.push(SelectItem::Column(self.expect_ident()?));
            // 如果不是逗号，则结束
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
        let mut left = self.parse_comparison()?;
        while self.peek() == &Token::And {
            self.advance();
            let right = self.parse_comparison()?;
            left = Expr::And(Box::new(left), Box::new(right));
        }
        Ok(left)
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
            _ => Ok(left), // 没有比较运算符，直接返回左操作数
        }
    }

    /// 解析"原子"表达式：列名、数字、字符串
    fn parse_primary(&mut self) -> Result<Expr, String> {
        match self.advance() {
            Token::Ident(name) => Ok(Expr::Column(name)),
            Token::Number(n) => Ok(Expr::Literal(LitValue::Number(n))),
            Token::Str(s) => Ok(Expr::Literal(LitValue::Str(s))),
            tok => Err(format!("unexpected token in expression: {:?}", tok)),
        }
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
    fn test_select_star_from() {
        let stmt = parse_sql("SELECT * FROM users").unwrap();
        assert_eq!(stmt, Statement::Select {
            columns: vec![SelectItem::Star],
            table: "users".into(),
            filter: None,
            joins: vec![],
            group_by: vec![],
        });
    }

    #[test]
    fn test_select_columns() {
        let stmt = parse_sql("SELECT name, age FROM users").unwrap();
        assert_eq!(stmt, Statement::Select {
            columns: vec![
                SelectItem::Column("name".into()),
                SelectItem::Column("age".into()),
            ],
            table: "users".into(),
            filter: None,
            joins: vec![],
            group_by: vec![],
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
            columns: vec![SelectItem::Star],
            table: "t".into(),
            filter: None,
            joins: vec![],
            group_by: vec![],
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
}