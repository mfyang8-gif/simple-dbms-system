// src/server/mod.rs
use std::io::{self, Write};

use crate::database::DbRegistry;
use crate::sql::{run_sql, ResultSet, Session};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::io::{BufRead, BufReader};
use std::thread;



/// 启动 TCP 服务器（阻塞，永不返回）
pub fn run_server(reg: DbRegistry, addr: &str) -> std::io::Result<()> {
    let listener = TcpListener::bind(addr)?;
    println!("MiniDB listening on {}", listener.local_addr()?);

    let reg = Arc::new(Mutex::new(reg));

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let reg = Arc::clone(&reg);
                thread::spawn(move || {
                    if let Err(e) = handle_client(stream, reg) {
                        eprintln!("client error: {}", e);
                    }
                });
            }
            Err(e) => eprintln!("accept failed: {}", e),
        }
    }
    Ok(())
}

/// 一条连接 = 一个 Session（事务状态跟着连接走，不跨连接共享）
fn handle_client(stream: TcpStream, reg: Arc<Mutex<DbRegistry>>) -> std::io::Result<()> {
    let peer = stream
        .peer_addr()
        .map(|a| a.to_string())
        .unwrap_or_else(|_| "?".to_string());
    println!("[+] client connected: {}", peer);

    // 必须在 stream 被 move 进 BufReader 之前取对端地址
    let peer_is_local = stream.peer_addr()
        .map(|a| a.ip().is_loopback())
        .unwrap_or(false);

    let mut writer = stream.try_clone()?;
    let reader = BufReader::new(stream);
    let mut session = Session::new();
    session.peer_is_local = peer_is_local;

    for line in reader.lines() {
        let line = line?;
        let sql = line.trim();

        if sql.is_empty() {
            continue;
        }
        if sql.eq_ignore_ascii_case("quit") {
            // 每个请求都要有响应，quit 也不例外
            let _ = writer.write_all(b"OK 0\n");
            let _ = writer.flush();
            break;
        }
        // 锁只包住"执行"，不包住 socket IO（读和写都在锁外）
        let response = {
            let mut guard = reg.lock().unwrap();
            // 按会话的 current_db 选出要操作的库；默认库同时充当账号库
            match guard.resolve(session.current_db.as_deref()) {
                Ok((db, accounts)) => match run_sql(sql, db, accounts, &mut session) {
                    Ok(rs) => format_result(&rs),
                    Err(e) => format!("ERR {}", escape_value(&e)),
                },
                Err(e) => format!("ERR {}", escape_value(&e)),
            }
        };

        writer.write_all(response.as_bytes())?;
        writer.write_all(b"\n")?;
        writer.flush()?;
    }

    // 连接断开时，未提交事务必须回滚，否则脏数据会留给其他连接
    if session.txn_id.is_some() {
        let mut guard = reg.lock().unwrap();
        // 事务属于「当时那个库」：USE 已经禁止在事务中执行，所以 current_db 就是事务所在的库
        if let Ok((db, accounts)) = guard.resolve(session.current_db.as_deref()) {
            let _ = run_sql("ROLLBACK", db, accounts, &mut session);
        }
        println!("[!] {} disconnected with open txn -> rolled back", peer);
    }

    println!("[-] client disconnected: {}", peer);
    Ok(())
}
// ===================== 4.1 协议编码 =====================

/// 转义单个值，防止 TAB/换行破坏协议
///
/// 单次遍历完成，天然避免"先转 TAB 再转反斜杠"导致双重转义的问题。
fn escape_value(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            other => out.push(other),
        }
    }
    out
}

/// 协议里 NULL 的写法：跟 MySQL 的 LOAD DATA 习惯一致，用反斜杠 + N 两个字符表示。
/// 为什么不能直接写 "NULL" 四个字母：那一列就分不清是字符串 'NULL' 还是真的没值。
/// 字面上的字符串（反斜杠 N）经过 escape_value 会变成双反斜杠 + N，所以两者不会撞车。
pub const NULL_TOKEN: &str = "\\N";

/// 把执行结果编码成协议文本（不含最后那个换行，由发送方补）
///
/// Ok            → "OK 0"
/// Affected(n)   → "OK n"
/// Rows{c,r}     → "ROWS <列数> <行数>\n<表头>\n<每行>\nEND"
pub fn format_result(rs: &ResultSet) -> String {
    match rs {
        ResultSet::Ok => "OK 0".to_string(),
        ResultSet::Affected(n) => format!("OK {}", n),
        ResultSet::Rows { columns, rows } => {
            let mut out = String::new();
            out.push_str(&format!("ROWS {} {}\n", columns.len(), rows.len()));

            let header: Vec<String> = columns.iter().map(|c| escape_value(c)).collect();
            out.push_str(&header.join("\t"));
            out.push('\n');

            for row in rows {
                let cells: Vec<String> = row
                    .iter()
                    .map(|v| match v {
                        None => NULL_TOKEN.to_string(),
                        Some(s) => escape_value(s),
                    })
                    .collect();
                out.push_str(&cells.join("\t"));
                out.push('\n');
            }

            out.push_str("END");
            out
        }
    }
}

// ===================== 4.2 人类可读格式 =====================

fn pad_right(s: &str, width: usize) -> String {
    let n = s.chars().count();
    if n >= width {
        s.to_string()
    } else {
        format!("{}{}", s, " ".repeat(width - n))
    }
}

/// 终端显示用的单元格：NULL 就写 NULL，不写空
fn display_cell(cell: &Option<String>) -> String {
    cell.clone().unwrap_or_else(|| "NULL".to_string())
}

/// 给终端看的对齐表格（REPL 专用，不是协议格式）
pub fn format_for_human(rs: &ResultSet) -> String {
    match rs {
        ResultSet::Ok => "OK".to_string(),
        ResultSet::Affected(n) => format!("{} row(s) affected", n),
        ResultSet::Rows { columns, rows } => {
            if rows.is_empty() {
                return format!("(0 rows)\n{}", columns.join(" | "));
            }

            // 给人看时 NULL 就写成 NULL（宽度和对齐都按这个字符串算）
            let mut widths: Vec<usize> = columns.iter().map(|c| c.chars().count()).collect();
            for row in rows {
                for (i, cell) in row.iter().enumerate() {
                    if i < widths.len() {
                        widths[i] = widths[i].max(display_cell(cell).chars().count());
                    }
                }
            }

            let mut out = String::new();

            let header: Vec<String> = columns
                .iter()
                .enumerate()
                .map(|(i, c)| pad_right(c, widths[i]))
                .collect();
            out.push_str(&header.join(" | "));
            out.push('\n');

            let sep: Vec<String> = widths.iter().map(|w| "-".repeat(*w)).collect();
            out.push_str(&sep.join("-+-"));
            out.push('\n');

            for row in rows {
                let cells: Vec<String> = row
                    .iter()
                    .enumerate()
                    .map(|(i, c)| pad_right(&display_cell(c), widths[i]))
                    .collect();
                out.push_str(&cells.join(" | "));
                out.push('\n');
            }

            out.push_str(&format!("({} row(s))", rows.len()));
            out
        }
    }
}

// ===================== 4.2 REPL =====================

/// 本地交互式终端：读一行 SQL → 执行 → 打印
pub fn run_repl(reg: &mut DbRegistry) {
    let mut session = Session::new();
    let stdin = io::stdin();

    println!("MiniDB REPL ready. Type SQL, or 'exit' to quit.");

    loop {
        print!("minidb> ");
        let _ = io::stdout().flush();

        let mut line = String::new();
        match stdin.read_line(&mut line) {
            Ok(0) => break,          // EOF (Ctrl+Z / Ctrl+D)
            Ok(_) => {}
            Err(e) => {
                println!("read error: {}", e);
                break;
            }
        }

        let sql = line.trim();
        if sql.is_empty() {
            continue;
        }
        if sql.eq_ignore_ascii_case("exit") || sql.eq_ignore_ascii_case("quit") {
            break;
        }

        match reg.resolve(session.current_db.as_deref()) {
            Ok((db, accounts)) => match run_sql(sql, db, accounts, &mut session) {
                Ok(rs) => println!("{}", format_for_human(&rs)),
                Err(e) => println!("ERROR: {}", e),
            },
            Err(e) => println!("ERROR: {}", e),
        }
    }

    if session.txn_id.is_some() {
        println!("warning: 退出时有未提交的事务，重启恢复时会自动回滚");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_ok() {
        assert_eq!(format_result(&ResultSet::Ok), "OK 0");
        assert_eq!(format_result(&ResultSet::Affected(3)), "OK 3");
    }

    #[test]
    fn test_format_rows() {
        let rs = ResultSet::Rows {
            columns: vec!["id".into(), "name".into()],
            rows: vec![
                vec![Some("1".into()), Some("Alice".into())],
                vec![Some("2".into()), Some("Bob".into())],
            ],
        };
        assert_eq!(format_result(&rs), "ROWS 2 2\nid\tname\n1\tAlice\n2\tBob\nEND");
    }

    #[test]
    fn test_format_rows_null_cell() {
        let rs = ResultSet::Rows {
            columns: vec!["id".into(), "name".into()],
            rows: vec![
                vec![Some("1".into()), None],
                vec![Some("2".into()), Some("NULL".into())],   // 字面字符串 "NULL"
                vec![Some("3".into()), Some("\\N".into())],    // 字面字符串 "\N"
            ],
        };
        let out = format_result(&rs);
        // 真 NULL → \N；字符串 NULL → NULL；字符串 \N → \\N，三者互不混淆
        assert_eq!(out, "ROWS 2 3\nid\tname\n1\t\\N\n2\tNULL\n3\t\\\\N\nEND");
    }

    #[test]
    fn test_escape_value() {
        assert_eq!(escape_value("a\tb"), "a\\tb");
        assert_eq!(escape_value("line1\nline2"), "line1\\nline2");
        assert_eq!(escape_value("C:\\temp"), "C:\\\\temp");
    }

    #[test]
    fn test_format_for_human_has_header_and_count() {
        let rs = ResultSet::Rows {
            columns: vec!["id".into(), "name".into()],
            rows: vec![vec![Some("1".into()), None]],
        };
        let out = format_for_human(&rs);
        assert!(out.contains("id"));
        assert!(out.contains("NULL"), "got: {}", out);
        assert!(out.contains("(1 row(s))"));
    }

}