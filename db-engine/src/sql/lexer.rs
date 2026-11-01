// src/sql/lexer.rs

use crate::sql::token::Token;

/// 词法分析器：把 SQL 字符串切成 Token 序列
pub struct Lexer {
    input: Vec<char>,   // 把 &str 转成 char 数组，方便按位置扫描
    pos: usize,         // 当前扫描位置
}

impl Lexer {
    pub fn new(input: &str) -> Self {
        Self { input: input.chars().collect(), pos: 0 }
    }

    /// 主入口：把整个输入切完，返回 Token 列表（末尾自动追加 Eof）
    pub fn tokenize(mut self) -> Result<Vec<Token>, String> {
        let mut tokens = Vec::new();
        loop {
            self.skip_whitespace();
            if self.pos >= self.input.len() {
                tokens.push(Token::Eof);
                return Ok(tokens);
            }
            let tok = self.next_token()?;
            tokens.push(tok);
        }
    }

    fn skip_whitespace(&mut self) {
        while self.pos < self.input.len() && self.input[self.pos].is_whitespace() {
            self.pos += 1;
        }
    }

    /// 查看当前字符，不移动位置
    fn peek(&self) -> Option<char> {
        self.input.get(self.pos).copied()
    }

    /// 获取当前字符并移动到下一个字符
    fn advance(&mut self) -> char {
        let c = self.input[self.pos];
        self.pos += 1;
        c
    }

    fn next_token(&mut self) -> Result<Token, String> {
        let c = self.peek().unwrap();
        match c {
            // ===== 样板 1：单字符符号（直接返回） =====
            '(' => { self.advance(); Ok(Token::LParen) }
            ')' => { self.advance(); Ok(Token::RParen) }
            ',' => { self.advance(); Ok(Token::Comma) }
            ';' => { self.advance(); Ok(Token::Semicolon) }
            '*' => { self.advance(); Ok(Token::Star) }
            '.' => { self.advance(); Ok(Token::Dot) }

            // ===== 样板 2：双字符符号（需要 peek 下一个） =====
            '=' => { self.advance(); Ok(Token::Eq) }
            '!' => {
                self.advance();
                if self.peek() == Some('=') { self.advance(); Ok(Token::Neq) }
                else { Err("expected '=' after '!'".to_string()) }
            }
            '<' => {
                self.advance();
                if self.peek() == Some('=') { self.advance(); Ok(Token::Le) }
                else { Ok(Token::Lt) }
            }
            '>' => {
                self.advance();
                if self.peek() == Some('=') { self.advance(); Ok(Token::Ge) }
                else { Ok(Token::Gt) }
            }

            // ===== TODO 你来写以下 3 个分支 =====

            // ① 标识符 / 关键字：字母或下划线开头，后跟字母/数字/下划线
            //    读完整段 → 查关键字表 → 是关键字返回对应变体，否则返回 Ident(s)
            //    提示：用 self.advance() 逐字符读，循环条件 c.is_alphanumeric() || c == '_'
            //    关键字表：一个 match 把 "SELECT"→Token::Select 等映射好
            //    注意：SQL 关键字不区分大小写 → 读出来后 to_uppercase() 再 match
            c if c.is_alphabetic() || c == '_' => {
                let mut s = String::new();
                while let Some(ch) = self.peek() {
                    if ch.is_alphanumeric() || ch == '_' {
                        s.push(ch);
                        self.advance();
                    } else {
                        break;
                    }
                }
                match s.to_uppercase().as_str() {
                    "SELECT" => Ok(Token::Select),
                    "FROM" => Ok(Token::From),
                    "WHERE" => Ok(Token::Where),
                    "INSERT" => Ok(Token::Insert),
                    "INTO" => Ok(Token::Into),
                    "VALUES" => Ok(Token::Values),
                    "UPDATE" => Ok(Token::Update),
                    "SET" => Ok(Token::Set),
                    "DELETE" => Ok(Token::Delete),
                    "CREATE" => Ok(Token::Create),
                    "TABLE" => Ok(Token::Table),
                    "DROP" => Ok(Token::Drop),
                    "AND" => Ok(Token::And),
                    "OR" => Ok(Token::Or),
                    "NOT" => Ok(Token::Not),
                    "LIKE" => Ok(Token::Like),
                    "IS" => Ok(Token::Is),
                    "JOIN" => Ok(Token::Join),
                    "ON" => Ok(Token::On),
                    "GROUP" => Ok(Token::Group),
                    "BY" => Ok(Token::By),
                    "ORDER" => Ok(Token::Order),
                    "ASC" => Ok(Token::Asc),
                    "DESC" => Ok(Token::Desc),
                    "HAVING" => Ok(Token::Having),
                    "LIMIT" => Ok(Token::Limit),
                    "BEGIN" => Ok(Token::Begin),
                    "COMMIT" => Ok(Token::Commit),
                    "ROLLBACK" => Ok(Token::Rollback),
                    "INT" => Ok(Token::Int),
                    "VARCHAR" => Ok(Token::Varchar),
                    "PRIMARY" => Ok(Token::Primary),
                    "KEY" => Ok(Token::Key),
                    "NULL" => Ok(Token::Null),
                    "UNIQUE" => Ok(Token::Unique),
                    "DEFAULT" => Ok(Token::Default),
                    "AS" => Ok(Token::As),
                    "DISTINCT" => Ok(Token::Distinct),
                    // 聚合函数名也收进关键字表：
                    // 否则 "SUM" 会先被 lexer 当成 Ident，parser 得回头做字符串比较
                    "SUM" => Ok(Token::Sum),
                    "COUNT" => Ok(Token::Count),
                    "AVG" => Ok(Token::Avg),
                    "MIN" => Ok(Token::Min),
                    "MAX" => Ok(Token::Max),
                    "USER" => Ok(Token::User),
                    "PASSWORD" => Ok(Token::Password),
                    "LOGIN" => Ok(Token::Login),
                    "GRANT" => Ok(Token::Grant),
                    "REVOKE" => Ok(Token::Revoke),
                    "TO" => Ok(Token::To),
                    "ALL" => Ok(Token::All),
                    "DATABASE" => Ok(Token::Database),
                    "USE" => Ok(Token::Use),
                    "VIEW" => Ok(Token::View),
                    "SHOW" => Ok(Token::Show),
                    "TABLES" => Ok(Token::Tables),
                    "DESCRIBE" => Ok(Token::Describe),
                    "TRUNCATE" => Ok(Token::Truncate),
                    "ALTER" => Ok(Token::Alter),
                    "RENAME" => Ok(Token::Rename),
                    "COLUMN" => Ok(Token::Column),
                    _ => Ok(Token::Ident(s)),
                }

            }

            // ② 数字：以数字开头，连续读数字位，parse 成 i64 → Token::Number
            c if c.is_ascii_digit() => {
                let mut s = String::new();
                while let Some(ch) = self.peek() {
                    if ch.is_ascii_digit() {
                        s.push(ch);
                        self.advance();
                    } else {
                        break;
                    }
                }
                s.parse::<i64>().map(Token::Number).map_err(|e| e.to_string())
            }
            // ③ 字符串字面量：以 ' 或 " 开头，读到匹配的结束引号为止，
            //    中间内容（不含引号）→ Token::Str(s)
            //    处理转义：'' 表示一个单引号（SQL 标准），类似 "" 表示双引号
            '\'' | '"' => {
                let quote = self.advance();  // 记住是哪种引号
                let mut s = String::new();
                loop {
                    match self.peek() {
                        None => return Err("unterminated string literal".to_string()),
                        Some(ch) if ch == quote => {
                            self.advance();
                            // 连续两个引号 = 转义，加入一个字面引号
                            if self.peek() == Some(quote) {
                                s.push(quote);
                                self.advance();
                            } else {
                                break;  // 真正的字符串结束
                            }
                        }
                        Some(ch) => {
                            s.push(ch);
                            self.advance();
                        }
                    }
                }
                Ok(Token::Str(s))
            }

            _ => Err(format!("unexpected character: '{}'", c)),
        }
    }


}




#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(sql: &str) -> Vec<Token> {
        Lexer::new(sql).tokenize().unwrap()
    }

    #[test]
    fn test_simple_select() {
        assert_eq!(
            tokens("SELECT * FROM users"),
            vec![Token::Select, Token::Star, Token::From, Token::Ident("users".into()), Token::Eof]
        );
    }

    #[test]
    fn test_where_condition() {
        assert_eq!(
            tokens("age > 18"),
            vec![Token::Ident("age".into()), Token::Gt, Token::Number(18), Token::Eof]
        );
    }

    #[test]
    fn test_keywords_case_insensitive() {
        // 小写关键字也能识别
        assert_eq!(
            tokens("select from where"),
            vec![Token::Select, Token::From, Token::Where, Token::Eof]
        );
    }

    #[test]
    fn test_string_literal_single_quote() {
        assert_eq!(
            tokens("'hello'"),
            vec![Token::Str("hello".into()), Token::Eof]
        );
    }

    #[test]
    fn test_string_literal_escaped_quote() {
        // 'it''s ok' → Str("it's ok")
        assert_eq!(
            tokens("'it''s ok'"),
            vec![Token::Str("it's ok".into()), Token::Eof]
        );
    }

    #[test]
    fn test_insert_values() {
        assert_eq!(
            tokens("INSERT INTO users VALUES (1, 'Alice')"),
            vec![
                Token::Insert, Token::Into, Token::Ident("users".into()),
                Token::Values, Token::LParen, Token::Number(1), Token::Comma,
                Token::Str("Alice".into()), Token::RParen, Token::Eof
            ]
        );
    }

    #[test]
    fn test_multi_condition_and_or() {
        assert_eq!(
            tokens("id = 1 AND name LIKE '%张%'"),
            vec![
                Token::Ident("id".into()), Token::Eq, Token::Number(1),
                Token::And,
                Token::Ident("name".into()), Token::Like, Token::Str("%张%".into()),
                Token::Eof
            ]
        );
    }

    #[test]
    fn test_show_keyword() {
        // SHOW TABLES 是两条独立关键字，不能退化成 Ident
        assert_eq!(
            tokens("SHOW TABLES"),
            vec![Token::Show, Token::Tables, Token::Eof]
        );
        // 表名恰好在别的语句里叫 tables 也不会被误伤：这里只验证识别方向
        assert_eq!(
            tokens("show tables"),
            vec![Token::Show, Token::Tables, Token::Eof]
        );
    }

    #[test]
    fn test_describe_keyword() {
        assert_eq!(
            tokens("DESCRIBE t"),
            vec![Token::Describe, Token::Ident("t".into()), Token::Eof]
        );
        // DESC 必须仍然是排序方向：DESCRIBE 不能被前缀吃成 DESC + RIBE
        assert_eq!(tokens("DESC"), vec![Token::Desc, Token::Eof]);
    }

    #[test]
    fn test_truncate_keyword() {
        assert_eq!(
            tokens("TRUNCATE TABLE t"),
            vec![Token::Truncate, Token::Table, Token::Ident("t".into()), Token::Eof]
        );
    }

    #[test]
    fn test_alter_keyword() {
        assert_eq!(
            tokens("ALTER TABLE t RENAME COLUMN a TO b"),
            vec![
                Token::Alter, Token::Table, Token::Ident("t".into()),
                Token::Rename, Token::Column, Token::Ident("a".into()),
                Token::To, Token::Ident("b".into()), Token::Eof
            ]
        );
    }

    #[test]
    fn test_is_null_keywords() {
        assert_eq!(
            tokens("name IS NULL"),
            vec![Token::Ident("name".into()), Token::Is, Token::Null, Token::Eof]
        );
        assert_eq!(
            tokens("name IS NOT NULL"),
            vec![
                Token::Ident("name".into()), Token::Is, Token::Not, Token::Null, Token::Eof
            ]
        );
    }

    #[test]
    fn test_comparison_operators() {
        assert_eq!(
            tokens("a >= 1 AND b != 2 AND c <= 3"),
            vec![
                Token::Ident("a".into()), Token::Ge, Token::Number(1),
                Token::And,
                Token::Ident("b".into()), Token::Neq, Token::Number(2),
                Token::And,
                Token::Ident("c".into()), Token::Le, Token::Number(3),
                Token::Eof
            ]
        );
    }
    #[test]
    fn test_agg_function_keywords() {
        assert_eq!(
            tokens("count(*) group by"),
            vec![
                Token::Count, Token::LParen, Token::Star, Token::RParen,
                Token::Group, Token::By, Token::Eof
            ]
        );
    }
    #[test]
    fn test_having_is_keyword() {
        assert_eq!(tokens("HAVING"), vec![Token::Having, Token::Eof]);
    }
    // ... existing code ...
    #[test]
    fn test_view_keyword() {
        assert_eq!(
            tokens("CREATE VIEW v AS SELECT * FROM t"),
            vec![
                Token::Create, Token::View, Token::Ident("v".into()), Token::As,
                Token::Select, Token::Star, Token::From, Token::Ident("t".into()),
                Token::Eof
            ]
        );
    }

}