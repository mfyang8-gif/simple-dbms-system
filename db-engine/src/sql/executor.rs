// src/sql/executor.rs

use crate::sql::ast::*;
use crate::database::Database;
use crate::table::{Column, ColumnType, DefaultVal};
use crate::types::RowId;
/// 执行结果：SELECT 返回行，其它语句返回受影响行数
use crate::record::Record;
use crate::row::Row;
use crate::table::Table;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
#[derive(Debug)]
pub enum ResultSet {
    /// None = SQL NULL（不能用空字符串代替，否则会把 NULL 当成 "" 输出）
    Rows { columns: Vec<String>, rows: Vec<Vec<Option<String>>> },
    Affected(usize),
    Ok,
}

/// 每个 TCP 连接一份，跨语句保持事务状态
pub struct Session {
    pub txn_id: Option<u64>,   // 当前活跃事务 ID；None 表示不在事务中
    /// 当前登录用户。None = 匿名。
    /// 它跟着「连接」走（server 每条连接一个 Session），
    /// 所以两个客户端可以同时是不同用户，事务和身份都互不干扰。
    pub user: Option<String>,
    /// 这条连接是否来自回环地址。零用户期的 bootstrap 只允许本地。
    /// REPL 与单元测试没有网络对端，构造时默认 true；
    /// 网络连接在 server/mod.rs 里用真实 peer 地址显式覆盖。
    pub peer_is_local: bool,
    /// 当前库名；None = 启动时打开的默认库
    pub current_db: Option<String>,
}

fn ast_col_to_column(def: ColumnDef) -> Column {
    let base = match def.data_type {
        DataType::Int => Column::int(&def.name),
        DataType::Varchar(n) => Column::varchar(&def.name, n),
    };
    let mut c = base;
    if def.primary_key { c = c.primary_key(); }
    if def.not_null    { c = c.not_null(); }
    if def.unique      { c = c.unique(); }
    // DEFAULT 只存字面量：数字按 INT 默认、字符串按 VARCHAR 默认；
    // 类型不匹配（如 INT 列 DEFAULT 'abc'）不在此拦截，写入时由存储层报错
    if let Some(lit) = def.default {
        c = match lit {
            LitValue::Number(n) => c.default_int(n as u32),
            LitValue::Str(s) => c.default_varchar(&s),
            LitValue::Null => c,
        };
    }
    c
}
#[derive(Debug, Clone, PartialEq)]
enum Value {
    Int(i64),
    Str(String),
    /// SQL 的“没值”：与空字符串、与 0 都不是一个东西
    Null,
}

impl Value {
    /// 人类可读形式（REPL / 错误信息用）
    fn to_string_readable(&self) -> String {
        match self {
            Value::Int(n) => n.to_string(),
            Value::Str(s) => s.clone(),
            Value::Null => "NULL".to_string(),
        }
    }

    /// 输出格：None 就是 NULL，到协议层才能区分“字符串 NULL”与“真的是 NULL”
    fn to_cell(&self) -> Option<String> {
        match self {
            Value::Int(n) => Some(n.to_string()),
            Value::Str(s) => Some(s.clone()),
            Value::Null => None,
        }
    }
}

/// 三值逻辑：SQL 里 `NULL = 1` 不是 false 而是 unknown，
/// 必须三值才算得对：`NULL OR TRUE` = TRUE，`NULL AND FALSE` = FALSE。
#[derive(Debug, Clone, Copy, PartialEq)]
enum Tri {
    True,
    False,
    Unknown,
}

impl Tri {
    fn and(self, other: Self) -> Self {
        use Tri::*;
        match (self, other) {
            // 只要有一边确定 false，整体就是 false，unknown 管不住它
            (False, _) | (_, False) => False,
            (Unknown, _) | (_, Unknown) => Unknown,
            (True, True) => True,
        }
    }

    fn or(self, other: Self) -> Self {
        use Tri::*;
        match (self, other) {
            (True, _) | (_, True) => True,
            (Unknown, _) | (_, Unknown) => Unknown,
            (False, False) => False,
        }
    }

    fn not(self) -> Self {
        match self {
            Tri::True => Tri::False,
            Tri::False => Tri::True,
            Tri::Unknown => Tri::Unknown,
        }
    }
}

/// 按 schema 把一行字节解析成 Vec<Value>
fn decode_row(table: &Table, record: &Record) -> Result<Vec<Value>, String> {
    let row = Row::from_record(record);
    // 带列数校验：行里的位图长度对不上就报错，而不是把值字节当位图静默读
    let mut cursor = row.cursor_with_cols(table.columns.len())
        .map_err(|e| format!("decode row: {}", e))?;
    let mut vals = Vec::new();
    for col in &table.columns {
        let v = match &col.col_type {
            ColumnType::Int => match cursor.get_int_opt() {
                Ok(Some(n)) => Value::Int(n as i64),
                Ok(None) => Value::Null,
                Err(e) => return Err(format!("decode col '{}': {}", col.name, e)),
            },
            ColumnType::Varchar(_) => match cursor.get_varchar_opt() {
                Ok(Some(s)) => Value::Str(s.to_string()),
                Ok(None) => Value::Null,
                Err(e) => return Err(format!("decode col '{}': {}", col.name, e)),
            },
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
    pub fn new() -> Self {
        Self { txn_id: None, user: None, peer_is_local: true, current_db: None }
    }
}
// ==================== 访问控制：内部表 ====================
// 为什么用「内部表」而不是另开一个 users.db：
// 能白拿 Table 的 WAL、Catalog、FreePageManager、事务和崩溃恢复。
// 代价只是这两张表会出现在 list_tables() 里（交付报告里写明即可）。
const USERS_TABLE: &str = "__minidb_users";
const PRIVS_TABLE: &str = "__minidb_privs";
/// 视图元数据表：definition 是规范化 SQL，base_tables/columns 是建视图时算好的快照
const VIEWS_TABLE: &str = "__minidb_views";
pub const INTERNAL_PREFIX: &str = "__minidb_";
/// 口令摘要：FNV-1a 64 位 → 16 个十六进制字符。
/// 零依赖、确定、可重复；但它**不是 KDF**（无 salt、无迭代），
/// 报告里必须写明“生产环境应换 bcrypt / argon2”。
fn hash_password(plain: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in plain.as_bytes() {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{:016x}", hash)
}

/// 只建账号表（建在「账号库」上）
fn ensure_users_table(db: &mut Database) -> Result<(), String> {
    if db.get_table_meta(USERS_TABLE).is_none() {
        db.create_table(USERS_TABLE, vec![
            Column::varchar("username", 64).primary_key(),
            Column::varchar("pwd_hash", 64).not_null(),
            Column::int("is_admin").not_null(),
        ]).map_err(|e| format!("create {}: {}", USERS_TABLE, e))?;
    }
    Ok(())
}

/// 只建权限表（建在「当前库」上）
fn ensure_privs_table(db: &mut Database) -> Result<(), String> {
    if db.get_table_meta(PRIVS_TABLE).is_none() {
        db.create_table(PRIVS_TABLE, vec![
            Column::varchar("grant_key", 200).primary_key(),
            Column::varchar("username", 64).not_null(),
            Column::varchar("table_name", 64).not_null(),
            Column::varchar("priv_name", 8).not_null(),
        ]).map_err(|e| format!("create {}: {}", PRIVS_TABLE, e))?;
    }
    Ok(())
}
/// 只建视图元数据表（建在「当前库」上）
fn ensure_views_table(db: &mut Database) -> Result<(), String> {
    if db.get_table_meta(VIEWS_TABLE).is_none() {
        db.create_table(VIEWS_TABLE, vec![
            Column::varchar("view_name", 64).primary_key(),
            Column::varchar("definition", 512).not_null(),
            Column::varchar("base_tables", 200).not_null(),
            Column::varchar("columns", 400).not_null(),
        ]).map_err(|e| format!("create {}: {}", VIEWS_TABLE, e))?;
    }
    Ok(())
}

/// 一条视图的元数据。
/// base_tables / columns 是「建视图那一刻」算好的快照，查询时不再解析 SQL 去推。
#[derive(Debug, Clone)]
struct ViewMeta {
    definition: String,     // 规范化 SQL，可直接重新 parse
    base_tables: Vec<String>,
    columns: Vec<String>,   // 视图的输出列名（* 已展开成具体列）
}

fn split_list(s: &str) -> Vec<String> {
    s.split(',').filter(|x| !x.is_empty()).map(|x| x.to_string()).collect()
}

/// 读一条视图元数据。**只读**：表不存在就返回 None，
/// 绝不能调 ensure_views_table（否则一次失败的 SELECT 会在库里凭空建表）。
fn load_view(db: &Database, name: &str) -> Result<Option<ViewMeta>, String> {
    if db.get_table_meta(VIEWS_TABLE).is_none() {
        return Ok(None);
    }
    let t = db.open_table(VIEWS_TABLE).map_err(|e| e.to_string())?;
    for (_, rec) in t.scan_all().map_err(|e| e.to_string())? {
        let vals = decode_row(&t, &rec)?;
        let (n, def, bases, cols) = match (vals.first(), vals.get(1), vals.get(2), vals.get(3)) {
            (Some(Value::Str(n)), Some(Value::Str(d)), Some(Value::Str(b)), Some(Value::Str(c))) => (n, d, b, c),
            _ => continue,
        };
        if n == name {
            return Ok(Some(ViewMeta {
                definition: def.clone(),
                base_tables: split_list(bases),
                columns: split_list(cols),
            }));
        }
    }
    Ok(None)
}

/// 执行一条 AST，返回结果集
// ==================== 访问控制：DCL 执行 ====================

fn execute_create_user(
    name: String,
    password: String,
    accounts: &mut Database,          // 账号库
    privs_db: Option<&mut Database>,  // 当前库；None 表示两者是同一个库
    session: &mut Session,
) -> Result<ResultSet, String> {
    ensure_users_table(accounts)?;   // ← 原 ensure_internal_tables(db)

    let mut users = accounts
        .open_table(USERS_TABLE)
        .map_err(|e| format!("open {}: {}", USERS_TABLE, e))?;

    // bootstrap：零用户时，第一个 CREATE USER 的用户自动成为 admin。
    // 判据是「表里有几行」，不是「有没有 is_admin=1 的行」——
    // 后者在 admin 被删光后会凭空冒出一个新 admin，等于权限可以“重新洗牌”。
    let is_first = users.scan_all().map_err(|e| e.to_string())?.is_empty();
    let is_admin = if is_first { 1 } else { 0 };

    let hash = hash_password(&password);
    let row = users
        .new_row()
        .put_varchar(&name).map_err(|e| e.to_string())?
        .put_varchar(&hash).map_err(|e| e.to_string())?
        .put_int(is_admin).map_err(|e| e.to_string())?
        .build().map_err(|e| e.to_string())?;

    // 加入当前事务：BEGIN; CREATE USER ...; ROLLBACK 时这个用户会一起消失。
    // 这是与表数据一致的事务语义，属于可接受边界（要写进报告）。
    accounts.insert_in_txn(&mut users, row, session.txn_id)
        .map_err(|e| format!("create user failed: {}", e))?;
    // 第二层防御：同名用户重建时，旧授权一律作废。
    // DROP USER 只能清到「当前库」，别的库里的残留在这里被兜住。
    let pdb = match privs_db { Some(p) => p, None => accounts };
    cleanup_privs_of(pdb, &name, session)?;
    // 非第一个用户的 admin 校验不在这里做：4-E 的 authorize 统一拦，
    // 避免同一件事写两遍再删一遍。
    Ok(ResultSet::Ok)
}
/// 删掉某个用户的全部授权（只在给定库里找）。返回删了几条。
/// 表不存在 → 说明这个库从没 GRANT 过，直接返回 0，不要顺手建表。
fn cleanup_privs_of(db: &mut Database, user: &str, session: &mut Session) -> Result<usize, String> {
    if db.get_table_meta(PRIVS_TABLE).is_none() {
        return Ok(0);
    }
    let mut t = db.open_table(PRIVS_TABLE).map_err(|e| e.to_string())?;

    let mut doomed: Vec<RowId> = Vec::new();
    for (rid, rec) in t.scan_all().map_err(|e| e.to_string())? {
        let vals = decode_row(&t, &rec)?;
        if let Some(Value::Str(u)) = vals.get(1) {
            if u == user {
                doomed.push(rid);
            }
        }
    }

    let n = doomed.len();
    for rid in doomed {
        t.delete(rid, db.txn_mgr(), session.txn_id)
            .map_err(|e| format!("cleanup privilege failed: {}", e))?;
    }
    Ok(n)
}

/// DROP USER bob
/// 不变量：① 不能删自己 ② 不能删掉最后一个 admin ③ 级联清当前库授权
fn execute_drop_user(
    name: String,
    accounts: &mut Database,
    privs_db: Option<&mut Database>,
    session: &mut Session,
) -> Result<ResultSet, String> {
    if accounts.get_table_meta(USERS_TABLE).is_none() {
        return Err(format!("user '{}' does not exist", name));
    }
    let mut users = accounts.open_table(USERS_TABLE).map_err(|e| e.to_string())?;

    // 一次扫描同时拿到：目标行、管理员总数、目标是不是管理员
    let mut doomed: Option<RowId> = None;
    let mut admin_count = 0usize;
    let mut target_is_admin = false;
    for (rid, rec) in users.scan_all().map_err(|e| e.to_string())? {
        let vals = decode_row(&users, &rec)?;
        let (u, adm) = match (vals.first(), vals.get(2)) {
            (Some(Value::Str(u)), Some(Value::Int(a))) => (u, *a != 0),
            _ => continue,
        };
        if adm { admin_count += 1; }
        if u == &name {
            doomed = Some(rid);
            target_is_admin = adm;
        }
    }

    let rid = doomed.ok_or_else(|| format!("user '{}' does not exist", name))?;

    // ① 不能删自己
    if session.user.as_deref() == Some(name.as_str()) {
        return Err("cannot drop the user you are logged in as".into());
    }
    // ② 不能删掉最后一个 admin（当前模型下它必然就是自己，但这条是不变量，留着）
    if target_is_admin && admin_count <= 1 {
        return Err("cannot drop the last admin".into());
    }

    users.delete(rid, accounts.txn_mgr(), session.txn_id)
        .map_err(|e| format!("drop user failed: {}", e))?;

    // ③ 级联：清当前库的授权（跨库残留由 CREATE USER 那层兜）
    let pdb = match privs_db { Some(p) => p, None => accounts };
    cleanup_privs_of(pdb, &name, session)?;

    Ok(ResultSet::Ok)
}
fn execute_login(
    name: String,
    password: String,
    accounts: &Database,          // ← 原 db，去掉 mut
    session: &mut Session,
) -> Result<ResultSet, String> {
    // 事务中禁止切身份：否则同一事务前半段是 A 的权限、后半段是 B 的，
    // 回滚时也无法解释“这条插入是谁做的”。
    if session.txn_id.is_some() {
        return Err("LOGIN is not allowed inside a transaction".into());
    }

    // LOGIN 是只读操作：零用户时不调 ensure_internal_tables
    // （否则“登录失败”会顺手在库里建出两张表，纯副作用）。
    if accounts.get_table_meta(USERS_TABLE).is_none() {
        return Err("invalid username or password".into());
    }

    let users = accounts
        .open_table(USERS_TABLE)
        .map_err(|e| format!("open {}: {}", USERS_TABLE, e))?;

    let want = hash_password(&password);
    for (_, record) in users.scan_all().map_err(|e| e.to_string())? {
        let vals = decode_row(&users, &record)?;
        let (u, h) = match (vals.first(), vals.get(1)) {
            (Some(Value::Str(u)), Some(Value::Str(h))) => (u, h),
            _ => continue,
        };
        if u == &name {
            if h != &want {
                return Err("invalid username or password".into());
            }
            session.user = Some(name);
            return Ok(ResultSet::Ok);
        }
    }

    // 与“密码错”用同一句：否则外部可以靠报错文案枚举出哪些用户名存在
    Err("invalid username or password".into())
}
// ==================== 访问控制：授权 ====================

fn priv_name(p: Priv) -> &'static str {
    match p {
        Priv::Select => "SELECT",
        Priv::Insert => "INSERT",
        Priv::Update => "UPDATE",
        Priv::Delete => "DELETE",
    }
}

/// 权限的主键：单列字符串。不能用三列复合主键——
/// 本引擎的唯一约束是逐列独立判定的（table/mod.rs:679）。
fn grant_key(user: &str, table: &str, p: Priv) -> String {
    format!("{}|{}|{}", user, table, priv_name(p))
}

fn user_exists(db: &Database, user: &str) -> Result<bool, String> {
    if db.get_table_meta(USERS_TABLE).is_none() {
        return Ok(false);
    }
    let t = db.open_table(USERS_TABLE).map_err(|e| e.to_string())?;
    for (_, rec) in t.scan_all().map_err(|e| e.to_string())? {
        let vals = decode_row(&t, &rec)?;
        if let Some(Value::Str(u)) = vals.first() {
            if u.as_str() == user {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn execute_grant(
    privs: Vec<Priv>,
    table: String,
    user: String,
    db: &mut Database,
    accounts: Option<&Database>,   // ← 新增
    session: &mut Session,
) -> Result<ResultSet, String> {
    ensure_privs_table(db)?;       // ← 原 ensure_internal_tables(db)

    // ① 表必须真实存在。否则会留下一条「幽灵权限」：
    //    今天给不存在的 del_log 授权，明年有人建了 del_log，权限突然生效。
    if db.get_table_meta(&table).is_none() {
        return Err(format!("table '{}' does not exist", table));
    }

    // ② 用户必须真实存在。否则就是「给未来的同名用户预埋权限」，
    //    本质上是后门：攻击者先 GRANT，再等你建出这个用户。
    // 用户存在性去账号库查；None 表示账号库就是当前库
    let user_ok = match accounts {
        Some(a) => user_exists(a, &user)?,
        None => user_exists(db, &user)?,
    };
    if !user_ok {
        return Err(format!("user '{}' does not exist", user));
    }
    let mut privs_tbl = db
        .open_table(PRIVS_TABLE)
        .map_err(|e| format!("open {}: {}", PRIVS_TABLE, e))?;

    // ③ 幂等：SQL 标准里重复 GRANT 不算错。先把已有主键收进集合，
    //    只插缺的那些——顺便避免了“靠解析错误字符串判断重复”的脆弱做法。
    let mut existing: HashSet<String> = HashSet::new();
    for (_, rec) in privs_tbl.scan_all().map_err(|e| e.to_string())? {
        let vals = decode_row(&privs_tbl, &rec)?;
        if let Some(Value::Str(k)) = vals.first() {
            existing.insert(k.clone());
        }
    }

    let mut inserted = 0usize;
    for p in privs {
        let key = grant_key(&user, &table, p);
        // insert 返回 false = 集合里已经有这条 → 这一次什么都不用做
        // 这一行同时解决了两种情况：库里已有 和 本语句内重复
        if !existing.insert(key.clone()) {
            continue;
        }
        let row = privs_tbl
            .new_row()
            .put_varchar(&key).map_err(|e| e.to_string())?
            .put_varchar(&user).map_err(|e| e.to_string())?
            .put_varchar(&table).map_err(|e| e.to_string())?
            .put_varchar(priv_name(p)).map_err(|e| e.to_string())?
            .build().map_err(|e| e.to_string())?;

        db.insert_in_txn(&mut privs_tbl, row, session.txn_id)
            .map_err(|e| format!("grant failed: {}", e))?;
        inserted += 1;
    }

    // 返回实际新增条数：重复 GRANT 会得到 0，客户端能看出“这次什么都没变”
    Ok(ResultSet::Affected(inserted))
}

fn execute_revoke(
    privs: Vec<Priv>,
    table: String,
    user: String,
    db: &mut Database,
    session: &mut Session,
) -> Result<ResultSet, String> {
    // 从来没 GRANT 过（表都不存在）→ 撤权是空操作，不是错误。
    // REVOKE 的语义是“确保这条权限不存在”，已经达成就返回 0。
    if db.get_table_meta(PRIVS_TABLE).is_none() {
        return Ok(ResultSet::Affected(0));
    }

    let mut privs_tbl = db
        .open_table(PRIVS_TABLE)
        .map_err(|e| format!("open {}: {}", PRIVS_TABLE, e))?;

    let targets: HashSet<String> = privs.iter().map(|p| grant_key(&user, &table, *p)).collect();

    // 先收齐 row_id，再统一删。这个实现里 scan_all() 返回的是 Vec 快照，
    // 边扫边删也能跑；但代码要按接口契约写，不能依赖“当前实现恰好物化了”。
    let mut doomed: Vec<RowId> = Vec::new();
    for (rid, rec) in privs_tbl.scan_all().map_err(|e| e.to_string())? {
        let vals = decode_row(&privs_tbl, &rec)?;
        if let Some(Value::Str(k)) = vals.first() {
            if targets.contains(k) {
                doomed.push(rid);
            }
        }
    }

    let removed = doomed.len();
    for rid in doomed {
        privs_tbl
            .delete(rid, db.txn_mgr(), session.txn_id)
            .map_err(|e| format!("revoke failed: {}", e))?;
    }
    Ok(ResultSet::Affected(removed))
}
// ==================== 多库：CREATE DATABASE / USE ====================

/// 从当前库的路径推出「同目录下的另一个库」的两个文件路径。
/// 不硬编码 "./data"：open_at 允许任意目录，库文件必须跟着当前库走。
fn sibling_paths(db: &Database, name: &str) -> Result<(String, String), String> {
    let dir = std::path::Path::new(db.db_path())
        .parent()
        .ok_or("cannot determine database directory")?;
    let db_file = dir.join(format!("{}.db", name));
    let log_file = dir.join(format!("{}.log", name));
    let db_file = db_file.to_str().ok_or("invalid database path")?;
    let log_file = log_file.to_str().ok_or("invalid database path")?;
    Ok((db_file.to_string(), log_file.to_string()))
}

fn execute_create_database(name: String, db: &Database) -> Result<ResultSet, String> {
    // ① 保留前缀：内部表用 __minidb_，库名不能占这个命名空间
    if name.starts_with(INTERNAL_PREFIX) {
        return Err(format!("database name '{}' is reserved", name));
    }
    // ② 不依赖上游：lexer 的标识符规则已经挡住了 '/' '\' '.'，
    //    但这里是「文件路径的最后一道关」，必须自己验一遍
    if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return Err(format!("invalid database name '{}'", name));
    }

    let (db_file, log_file) = sibling_paths(db, &name)?;

    // ③ 已存在：用文件存在性判断，绝不能用 open_at 试探
    //    （open_at 是「打开或创建」，试探等于顺手把库建出来）
    if std::path::Path::new(&db_file).exists() {
        return Err(format!("database '{}' already exists", name));
    }

    // ④ 建库 = 初始化一个新文件。open_at 内部会 init_new 并 flush 系统页
    let mut new_db = Database::open_at(&db_file, &log_file)
        .map_err(|e| format!("create database '{}': {}", name, e))?;
    new_db.shutdown()
        .map_err(|e| format!("close new database '{}': {}", name, e))?;

    Ok(ResultSet::Ok)
}

fn execute_use(name: String, db: &Database, session: &mut Session) -> Result<ResultSet, String> {
    if session.txn_id.is_some() {
        return Err("USE is not allowed inside a transaction".into());
    }
    let (db_file, _log_file) = sibling_paths(db, &name)?;

    // 用存在性判断，不是 open_at：后者会把不存在的库创建出来
    if !std::path::Path::new(&db_file).exists() {
        return Err(format!("database '{}' does not exist", name));
    }

    session.current_db = Some(name);
    Ok(ResultSet::Ok)
}
// ==================== 访问控制：唯一闸门 ====================

fn user_count(db: &Database) -> Result<usize, String> {
    if db.get_table_meta(USERS_TABLE).is_none() {
        return Ok(0);
    }
    let t = db.open_table(USERS_TABLE).map_err(|e| e.to_string())?;
    Ok(t.scan_all().map_err(|e| e.to_string())?.len())
}

fn is_admin(db: &Database, user: &str) -> Result<bool, String> {
    if db.get_table_meta(USERS_TABLE).is_none() {
        return Ok(false);
    }
    let t = db.open_table(USERS_TABLE).map_err(|e| e.to_string())?;
    for (_, rec) in t.scan_all().map_err(|e| e.to_string())? {
        let vals = decode_row(&t, &rec)?;
        if let (Some(Value::Str(u)), Some(Value::Int(a))) = (vals.first(), vals.get(2)) {
            if u.as_str() == user {
                return Ok(*a != 0);
            }
        }
    }
    Ok(false)
}

// 检查某用户对某表是否有某权限
fn has_priv(db: &Database, user: &str, table: &str, p: Priv) -> Result<bool, String> {
    if db.get_table_meta(PRIVS_TABLE).is_none() {
        return Ok(false);
    }
    let t = db.open_table(PRIVS_TABLE).map_err(|e| e.to_string())?;
    let key = grant_key(user, table, p);
    for (_, rec) in t.scan_all().map_err(|e| e.to_string())? {
        let vals = decode_row(&t, &rec)?;
        if let Some(Value::Str(k)) = vals.first() {
            if k == &key {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// `db` = 语句要操作的库；`accounts` = 实例级账号库（None 表示就是 db 自己）
fn authorize(
    stmt: &Statement,
    db: &Database,
    accounts: Option<&Database>,
    session: &Session,
) -> Result<(), String> {
    // 账号相关的查询统一走这里：有独立系统库就读它，否则读当前库
    let acc: &Database = accounts.unwrap_or(db);
    // ① 事务控制与 LOGIN 本身一律放行。
    //    BEGIN/COMMIT/ROLLBACK 必须放行：连接断开时 server 会自动发一条 ROLLBACK
    //    （server/mod.rs:74-78），那是在“未登录”状态下发的，拦了就会留下脏事务。
    let need: Option<Priv> = match stmt {
        Statement::Login { .. }
        // USE 只改会话状态，不读写任何页，也不涉及表和权限
        | Statement::Use { .. }
        | Statement::Begin | Statement::Commit | Statement::Rollback => return Ok(()),
        Statement::Select { .. } => Some(Priv::Select),
        Statement::Insert { .. } => Some(Priv::Insert),
        Statement::Update { .. } => Some(Priv::Update),
        Statement::Delete { .. } => Some(Priv::Delete),
        // DDL / DCL：不存在“表级权限”，只有 admin 能做
        Statement::CreateTable { .. } | Statement::DropTable { .. }
        | Statement::CreateUser { .. } | Statement::DropUser { .. }
        | Statement::Grant { .. } | Statement::Revoke { .. }
        // 建库 / 建视图 / 删视图：名字都不是表名，走「admin required」
        | Statement::CreateDatabase { .. }
        | Statement::CreateView { .. }
        | Statement::DropView { .. } => None,


    };

    // ② 这条语句碰到的表（内部表保护要用）
    let touched: Option<&str> = match stmt {
        Statement::Select { table, .. } | Statement::Insert { table, .. }
        | Statement::Update { table, .. } | Statement::Delete { table, .. }
        | Statement::Grant { table, .. } | Statement::Revoke { table, .. } => Some(table),
        Statement::CreateTable { name, .. } => Some(name),
        Statement::DropTable { name } => Some(name),
        Statement::CreateUser { .. } | Statement::DropUser { .. } | Statement::Login { .. }
        | Statement::Begin | Statement::Commit | Statement::Rollback
        // 库名 / 视图名 / 用户名都不是表名，不能塞进 touched
        | Statement::CreateView { .. } | Statement::DropView { .. } => None,
        | Statement::CreateDatabase { .. } | Statement::Use { .. } => None,
    };

    // ③ bootstrap：库里一个用户都没有 → 只有本地连接可以不设防。
    //    这是 MongoDB「localhost exception」的同构做法：不试图消灭窗口
    //    （只要初始管理员由网络请求触发，先到者必然得手），
    //    而是把窗口的可达范围从 0.0.0.0 收敛到 127.0.0.1。
    if user_count(acc)? == 0 {
        if session.peer_is_local {
            return Ok(());
        }
        return Err("database has no users yet; bootstrap must come from localhost".into());
    }

    // ④ 一旦有了用户，匿名就没有任何权限
    let user = match &session.user {
        Some(u) => u,
        None => return Err("please LOGIN first".into()),
    };

    let admin = is_admin(acc, user)?;

    // ⑤ 内部表保护：admin 可以 SELECT（验收/运维要看），但谁都不能直接写。
    //    能写就等于可以手工插一条伪造的 pwd_hash，或删掉最后一个 admin ——
    //    内部表只能由 DCL 代码维护。
    if let Some(t) = touched {
        if t.starts_with(INTERNAL_PREFIX) {
            if admin && need == Some(Priv::Select) {
                return Ok(());
            }
            return Err(format!("cannot access internal table '{}' directly", t));
        }
    }

    // ⑥ admin 全权
    if admin {
        return Ok(());
    }

    // ⑦ 表级 ACL
    match (touched, need) {
        (Some(t), Some(p)) => {
            if has_priv(db, user, t, p)? {
                Ok(())
            } else {
                // 不区分“表不存在”和“没权限”：否则外部能靠报错文案探测库结构
                Err(format!("permission denied: {} on table '{}'", priv_name(p), t))
            }
        }
        // DDL/DCL 且不是 admin
        (_, None) => Err("permission denied: admin required".to_string()),
        // 兜底也是 Err：宁可误拒，不可误放
        (None, Some(_)) => Err("permission denied".to_string()),
    }
}
/// 把「查视图」改写成「查基表」。
///
/// 只做三件事：
///   1. 外层 * → 视图的列清单（不是基表的全部列）
///   2. 视图定义里的 WHERE 保留，与外层 WHERE 用 AND 合并
///   3. 表名换成基表名
/// 其余子句（DISTINCT / GROUP BY / HAVING / ORDER BY / LIMIT）原样下推，
/// 因为建视图时已经拒绝了会让合并失效的写法。
fn expand_views(stmt: Statement, db: &Database) -> Result<Statement, String> {
    // ① 视图只读：写操作在入口就拒，别等到 open_table 报「表不存在」
    let write_target = match &stmt {
        Statement::Insert { table, .. }
        | Statement::Update { table, .. }
        | Statement::Delete { table, .. } => Some(table.clone()),
        _ => None,
    };
    if let Some(t) = write_target {
        if load_view(db, &t)?.is_some() {
            return Err(format!("view '{}' is read-only", t));
        }
        return Ok(stmt);
    }

    // ② 只有 SELECT 需要展开；其它语句原样返回
    let target = match &stmt {
        Statement::Select { table, .. } => table.clone(),
        _ => return Ok(stmt),
    };
    // 是普通表 → 不动
    if db.get_table_meta(&target).is_some() {
        return Ok(stmt);
    }
    // 既不是表也不是视图 → 原样返回，让 execute_select 报原来的错误文案
    let Some(meta) = load_view(db, &target)? else {
        return Ok(stmt);
    };

    let Statement::Select {
        distinct, columns, table: _, joins, filter, group_by, having, order_by, limit,
    } = stmt else {
        unreachable!("上面已经确认是 Select")
    };

    // ① 重新解析定义（存的是规范化 SQL，parser 往返测试保证等价）
    let def = {
        let tokens = crate::sql::lexer::Lexer::new(&meta.definition)
            .tokenize()
            .map_err(|e| format!("view '{}' has invalid definition: {}", target, e))?;
        crate::sql::parser::Parser::new(tokens)
            .parse()
            .map_err(|e| format!("view '{}' has invalid definition: {}", target, e))?
    };
    let Statement::Select {
        columns: inner_cols, table: inner_table, filter: inner_filter, ..
    } = def else {
        return Err(format!("view '{}' has invalid definition", target));
    };

    // ② 延迟失效检查（Q6 的乙方案）：基表被删了，查询时才报
    if db.get_table_meta(&inner_table).is_none() {
        return Err(format!(
            "view '{}' is invalid: base table '{}' not found",
            target, inner_table
        ));
    }

    // ③ 视图不参与 JOIN（合并语义会变复杂，入口处拒绝）
    if !joins.is_empty() {
        return Err(format!("view '{}' cannot be used in JOIN", target));
    }

    // ④ 外层 SELECT 列表：* 展开成视图的列，v.id 规范成 id
    let mut out_cols: Vec<SelectItem> = Vec::new();
    for c in columns {
        match c {
            SelectItem::Star => {
                for ic in &inner_cols {
                    if matches!(ic, SelectItem::Star) {
                        return Err(format!("view '{}' has an unresolved '*'", target));
                    }
                    out_cols.push(ic.clone());
                }
            }
            SelectItem::TableColumn { table: t, col } if t == target => {
                out_cols.push(SelectItem::Column(col));
            }
            other => out_cols.push(other),
        }
    }

    // ⑤ 外层引用的列必须属于这个视图（视图外的列不许透视）
    let view_cols: HashSet<&str> = meta.columns.iter().map(|s| s.as_str()).collect();
    for c in &out_cols {
        let name = match c {
            SelectItem::Column(n) => n.as_str(),
            SelectItem::TableColumn { col, .. } => col.as_str(),
            SelectItem::Aggregate { column, .. } => column.as_str(),
            SelectItem::Star => continue,
        };
        if !view_cols.contains(name) {
            return Err(format!("column '{}' not found in view '{}'", name, target));
        }
    }

    // ⑥ 合并过滤条件：定义的条件必须保留，否则视图退化成「基表的别名」
    let merged_filter = match (inner_filter, filter) {
        (Some(a), Some(b)) => Some(Expr::And(Box::new(a), Box::new(b))),
        (Some(a), None) => Some(a),
        (None, b) => b,
    };

    Ok(Statement::Select {
        distinct,
        columns: out_cols,
        table: inner_table,
        joins: vec![],
        filter: merged_filter,
        group_by,
        having,
        order_by,
        limit,
    })
}



pub fn execute(
    stmt: Statement,
    db: &mut Database,
    accounts: Option<&mut Database>,
    session: &mut Session,
) -> Result<ResultSet, String> {
    // 视图展开必须在 authorize 之前：
    // 展开后 touched 就是基表，权限闸门天然做到「能看视图 ⟺ 能看基表」。
    // db 以 & 传入，调用结束后可变借用立刻解冻，下面 authorize 借用不受影响。
    let stmt = expand_views(stmt, db)?;
    // 这里把 &mut 降级成 & 送进 authorize：两个不可变借用可以共存
    authorize(&stmt, db, accounts.as_deref(), session)?;
    match stmt {
        Statement::CreateTable { name, columns } => {
            execute_create_table(name, columns, db)
        }
        Statement::DropTable { name } => execute_drop_table(name, db),
        Statement::Insert { table, columns, rows } => execute_insert(table, columns, rows, db, session),
        Statement::Select { distinct, columns, table, joins, filter, group_by, having, order_by, limit, .. } => {
            execute_select(columns, distinct, table, joins, filter, group_by, having, order_by, limit, db)
        },
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
                db.log_path(),
            ).map_err(|e| format!("rollback failed: {}", e))?;

            // 写 Abort 日志 + 从活跃表摘除
            db.txn_mgr().lock().unwrap().abort(txn_id);
            session.txn_id = None;
            Ok(ResultSet::Ok)
        }
        Statement::DropView { name } => execute_drop_view(name, db, session),
        Statement::Update { table, assignments, filter } => execute_update(table, assignments, filter, db, session),
        Statement::CreateUser { name, password } => {
            // accounts 为 None 时，账号库就是当前库
            match accounts {
                Some(acc) => execute_create_user(name, password, acc, Some(db), session),
                None => execute_create_user(name, password, db, None, session),
            }
        }
        Statement::DropUser { name } => match accounts {
            Some(acc) => execute_drop_user(name, acc, Some(db), session),
            None => execute_drop_user(name, db, None, session),
        },
        Statement::Login { name, password } => {
            execute_login(name, password, accounts.as_deref().unwrap_or(db), session)
        }
        Statement::Grant { privs, table, user } => {
            execute_grant(privs, table, user, db, accounts.as_deref(), session)
        }
        Statement::Revoke { privs, table, user } => execute_revoke(privs, table, user, db, session),
        Statement::CreateDatabase { name } => execute_create_database(name, db),
        Statement::Use { name } => execute_use(name, db, session),
        // Step 3 接入真正的实现，先占位保证编译
        Statement::CreateView { name, query } => execute_create_view(name, *query, db, session),
    }
    }


fn execute_create_view(
    name: String,
    query: Statement,
    db: &mut Database,
    session: &mut Session,
) -> Result<ResultSet, String> {
    // ① 名字查重：表名、视图名、保留前缀
    if db.get_table_meta(&name).is_some() {
        return Err(format!("table '{}' already exists", name));
    }
    if name.starts_with(INTERNAL_PREFIX) {
        return Err(format!("'{}' is a reserved name", name));
    }
    ensure_views_table(db)?;
    if load_view(db, &name)?.is_some() {
        return Err(format!("view '{}' already exists", name));
    }

    // ② 定义必须是「可下推」的简单查询：单表 + 投影 + WHERE。
    //    合并展开只在这些子句上语义等价，其余一律在入口拒绝（宁可拒绝，不可错误地允许）。
    let Statement::Select { distinct, columns, table, joins, filter, group_by, having, order_by, limit } = query
    else {
        return Err("view definition must be a SELECT".to_string());
    };
    if distinct {
        return Err("view definition does not support DISTINCT".into());
    }
    if !joins.is_empty() {
        return Err("view definition does not support JOIN".into());
    }
    if !group_by.is_empty() || having.is_some() {
        return Err("view definition does not support GROUP BY / HAVING".into());
    }
    if !order_by.is_empty() {
        return Err("view definition does not support ORDER BY".into());
    }
    if limit.is_some() {
        return Err("view definition does not support LIMIT".into());
    }

    // ③ 基表必须真实存在，且不能是视图（禁止视图套视图），也不能是内部表
    if table.starts_with(INTERNAL_PREFIX) {
        return Err(format!("cannot create a view over internal table '{}'", table));
    }
    if load_view(db, &table)?.is_some() {
        return Err(format!("view '{}' cannot be based on another view", table));
    }
    let base = db.open_table(&table)
        .map_err(|_| format!("table '{}' does not exist", table))?;
    let base_cols: Vec<String> = base.columns.iter().map(|c| c.name.clone()).collect();

    // ④ 展开 *：此刻手里有基表 schema，就地换成具体列名
    let mut out_columns: Vec<SelectItem> = Vec::new();
    let mut out_names: Vec<String> = Vec::new();
    for item in columns {
        match item {
            SelectItem::Star => {
                for c in &base_cols {
                    out_columns.push(SelectItem::Column(c.clone()));
                    out_names.push(c.clone());
                }
            }
            SelectItem::Column(c) => {
                if !base_cols.iter().any(|x| x == &c) {
                    return Err(format!("column '{}' not found in table '{}'", c, table));
                }
                out_names.push(c.clone());
                out_columns.push(SelectItem::Column(c));
            }
            // t.id 统一规范成 id：视图的列名不该带基表前缀
            SelectItem::TableColumn { table: t, col } => {
                if t != table || !base_cols.iter().any(|x| x == &col) {
                    return Err(format!("column '{}.{}' not found in table '{}'", t, col, table));
                }
                out_names.push(col.clone());
                out_columns.push(SelectItem::Column(col));
            }
            SelectItem::Aggregate { .. } => {
                return Err("view definition does not support aggregate".into());
            }
        }
    }

    // ⑤ 存「规范化 SQL」而不是用户原文：parser 手上只有 token，拿不到原文
    let definition = crate::sql::parser::select_to_sql(&Statement::Select {
        distinct: false,
        columns: out_columns,
        table: table.clone(),
        joins: vec![],
        filter,
        group_by: vec![],
        having: None,
        order_by: vec![],
        limit: None,
    })?;

    // ⑥ 落盘：走 insert_in_txn，BEGIN / ROLLBACK 能一起回滚
    let mut views = db.open_table(VIEWS_TABLE).map_err(|e| e.to_string())?;
    let row = views
        .new_row()
        .put_varchar(&name).map_err(|e| e.to_string())?
        .put_varchar(&definition).map_err(|e| e.to_string())?
        .put_varchar(&table).map_err(|e| e.to_string())?
        .put_varchar(&out_names.join(",")).map_err(|e| e.to_string())?
        .build().map_err(|e| e.to_string())?;
    db.insert_in_txn(&mut views, row, session.txn_id)
        .map_err(|e| format!("create view failed: {}", e))?;

    Ok(ResultSet::Ok)
}
fn execute_drop_table(name: String, db: &mut Database) -> Result<ResultSet, String> {
    // 视图不是表：给一句能指导下一步的提示
    if db.get_table_meta(&name).is_none() && load_view(db, &name)?.is_some() {
        return Err(format!("'{}' is a view, use DROP VIEW", name));
    }
    db.drop_table(&name).map_err(|e| format!("drop_table failed: {}", e))?;
    Ok(ResultSet::Ok)
}

/// DROP VIEW v —— 只删 __minidb_views 里那一行，基表数据一行不动。
fn execute_drop_view(name: String, db: &mut Database, session: &mut Session) -> Result<ResultSet, String> {
    if db.get_table_meta(VIEWS_TABLE).is_none() {
        return Err(format!("view '{}' does not exist", name));
    }
    let mut views = db.open_table(VIEWS_TABLE).map_err(|e| e.to_string())?;

    let mut doomed: Option<RowId> = None;
    for (rid, rec) in views.scan_all().map_err(|e| e.to_string())? {
        let vals = decode_row(&views, &rec)?;
        if let Some(Value::Str(n)) = vals.first() {
            if n.as_str() == name {
                doomed = Some(rid);
                break;
            }
        }
    }
    let rid = doomed.ok_or_else(|| format!("view '{}' does not exist", name))?;

    views.delete(rid, db.txn_mgr(), session.txn_id)
        .map_err(|e| format!("drop view failed: {}", e))?;
    Ok(ResultSet::Ok)
}
fn execute_insert(
    table_name: String,
    columns: Vec<String>,
    rows: Vec<Vec<Expr>>,
    db: &mut Database,
    session: &mut Session,
) -> Result<ResultSet, String> {
    // 1. 打开表（拿到 schema）
    let mut table = db.open_table(&table_name)
        .map_err(|e| format!("open_table failed: {}", e))?;

    // 2. 目标列：没给列表 = 全部列按 schema 顺序；给了列表就按列名找下标（允许乱序、允许只写一部分）
    let targets: Vec<usize> = if columns.is_empty() {
        (0..table.columns.len()).collect()
    } else {
        let names: Vec<String> = table.columns.iter().map(|c| c.name.clone()).collect();
        columns.iter().map(|name| {
            names.iter().position(|c| c == name)
                .ok_or_else(|| format!("column '{}' not found in INSERT", name))
        }).collect::<Result<Vec<usize>, String>>()?
    };

    // 3. 列名校验只做一次：所有行共用同一套 targets / slot_of
    let mut slot_of: Vec<Option<usize>> = vec![None; table.columns.len()];
    for (v_idx, &col_idx) in targets.iter().enumerate() {
        if slot_of[col_idx].is_some() {
            return Err(format!("duplicate column '{}' in INSERT", table.columns[col_idx].name));
        }
        slot_of[col_idx] = Some(v_idx);
    }

    // 4. 先把每一行都编码出来：类型 / 值个数 / NOT NULL 的错误在任何一行入库前暴露
    let mut built: Vec<Row> = Vec::with_capacity(rows.len());
    for values in &rows {
        if values.len() != targets.len() {
            return Err(format!("expected {} values, got {}", targets.len(), values.len()));
        }
        let mut builder = table.new_row();
        for (i, col) in table.columns.iter().enumerate() {
            builder = match slot_of[i] {
                None => match &col.default_value {
                    Some(DefaultVal::Int(v)) => builder.put_int(*v),
                    Some(DefaultVal::Varchar(s)) => builder.put_varchar(s),
                    None => builder.put_null(),
                },
                Some(v_idx) => match &values[v_idx] {
                    Expr::Literal(LitValue::Number(n)) => builder.put_int(*n as u32),
                    Expr::Literal(LitValue::Str(s)) => builder.put_varchar(s),
                    Expr::Literal(LitValue::Null) => builder.put_null(),
                    other => {
                        return Err(format!("type mismatch on column '{}': got {:?}", col.name, other));
                    }
                },
            }
                .map_err(|e| e.to_string())?;
        }
        built.push(builder.build().map_err(|e| format!("row build failed: {}", e))?);
    }

    // 5. 落库：单行沿用自动提交；多行套一个隐式事务，
    //    保证「要么全进去，要么一行都不留」——否则第 2 行主键冲突时第 1 行已经提交了
    let count = built.len();
    let mut implicit_txn = None;
    let txn = match session.txn_id {
        Some(id) => Some(id),
        None if count > 1 => {
            let (id, _) = db.txn_mgr().lock().unwrap().begin();
            implicit_txn = Some(id);
            Some(id)
        }
        None => None,
    };

    let mut failure: Option<std::io::Error> = None;
    for row in built {
        if let Err(e) = db.insert_in_txn(&mut table, row, txn) {
            failure = Some(e);
            break;
        }
    }

    match (failure, implicit_txn) {
        (None, Some(id)) => {
            db.txn_mgr().lock().unwrap().commit(id);
            Ok(ResultSet::Affected(count))
        }
        (None, None) => Ok(ResultSet::Affected(count)),
        (Some(e), Some(id)) => {
            // 隐式事务回滚：把已经插进去的行全部 undo 掉
            let last_lsn = db.txn_mgr().lock().unwrap().last_lsn(id);
            let _ = crate::recovery::restore::undo_txn(
                id,
                last_lsn,
                db.buffer_pool(),
                db.log_manager(),
                db.log_path(),
            );
            db.txn_mgr().lock().unwrap().abort(id);
            Err(format!("insert failed: {}", e))
        }
        (Some(e), None) => Err(format!("insert failed: {}", e)),
    }
}
/// 在列名空间里解析一个列引用 → 下标。
///
/// 规则（JOIN 名称解析的全部内容）：
///   1. 先按完整名精确匹配：多表时列名空间是 ["users.id", "users.name", "orders.id"]
///   2. 再按短名匹配（去掉 "表名." 前缀）：
///        命中 0 个 → not found
///        命中 1 个 → 放行（单表写 users.id 也靠这条兜底）
///        命中 >=2 → ambiguous
///
/// 注意：不能用 `position(|c| c == name)` 一把梭——那会静默取第一个同名列。
fn resolve_column(name: &str, col_names: &[String]) -> Result<usize, String> {
    // ① 精确匹配：多表时是 "users.id"，单表时是裸名 "id"
    if let Some(i) = col_names.iter().position(|c| c == name) {
        return Ok(i);
    }

    // ② 带前缀却没精确命中 → 表名写错或该表没这列，不能按短名回退。
    //    否则 "other.name" 会在单表里被误认成 "name"。
    if name.contains('.') {
        return Err(format!("column '{}' not found", name));
    }

    // ③ 裸名 → 短名匹配 + 歧义检测
    let hits: Vec<usize> = col_names
        .iter()
        .enumerate()
        .filter(|(_, c)| short_name(c) == name)
        .map(|(i, _)| i)
        .collect();

    match hits.len() {
        0 => Err(format!("column '{}' not found", name)),
        1 => Ok(hits[0]),
        _ => Err(format!("ambiguous column '{}'", name)),
    }
}
/// "users.id" → "id"；本来就是裸名则原样返回
fn short_name(full: &str) -> &str {
    full.rsplit_once('.').map(|(_, c)| c).unwrap_or(full)
}
/// 把"排序键表达式"归一成列名，再交给 `resolve_column` 查下标。
///
/// 为什么需要这一步：排序发生在两个不同的列名空间里——
///   普通查询 → col_names = ["t.a", "t.b"]
///   聚合查询 → out_names = ["dept", "SUM(amount)"]
/// 把 Expr 先翻成"名字"，两条路就能共用同一个 resolve_column。
fn order_key_name(e: &Expr) -> Result<String, String> {
    match e {
        Expr::Column(name) => Ok(name.clone()),
        Expr::QualifiedColumn { table, col } => Ok(format!("{}.{}", table, col)),
        // 关键一行：聚合键和 SELECT 列表里的聚合用同一个 agg_name 拼名字，
        // 所以 "ORDER BY SUM(amount)" 能精确命中输出列 "SUM(amount)"
        Expr::Aggregate { func, column } => Ok(agg_name(func, column)),
        other => Err(format!(
            "ORDER BY only supports a column or an aggregate function, got {:?}", other)),
    }
}
/// 评估标量表达式（列名查表 → 下标 → 从当前行取值）
fn eval_scalar(e: &Expr, col_names: &[String], vals: &[Value]) -> Result<Value, String> {
    match e {
        // 裸列名：单表时唯一；JOIN 里同名列会在这里报 ambiguous
        Expr::Column(name) => {
            let idx = resolve_column(name, col_names)?;
            let v = vals.get(idx)
                .ok_or_else(|| format!("column '{}' is not available yet", name))?;
            Ok(v.clone())
        }
        // 限定列 users.id：JOIN 的标准写法，精确命中，永远不会歧义
        Expr::QualifiedColumn { table, col } => {
            let full = format!("{}.{}", table, col);
            let idx = resolve_column(&full, col_names)?;
            let v = vals.get(idx)
                .ok_or_else(|| format!("column '{}' is not available yet", full))?;
            Ok(v.clone())
        }
        Expr::Literal(LitValue::Number(n)) => Ok(Value::Int(*n)),
        Expr::Literal(LitValue::Str(s)) => Ok(Value::Str(s.clone())),
        Expr::Literal(LitValue::Null) => Ok(Value::Null),
        // 聚合出现在条件里（HAVING SUM(amount) > 100 的左边）。
        // 到这一步"每个组"已经算完了，行里存的就是聚合结果，
        // 所以这里只需要把 (函数名, 列名) 翻译成输出列名 "SUM(amount)"，再去列名空间查下标。
        Expr::Aggregate { func, column } => {
            let name = agg_name(func, column);
            let idx = resolve_column(&name, col_names).map_err(|_| {
                format!(
                    "aggregate '{}' is only available in HAVING and must also appear in the SELECT list",
                    name)
            })?;
            let v = vals.get(idx)
                .ok_or_else(|| format!("aggregate '{}' is not available yet", name))?;
            Ok(v.clone())
        }
        other => Err(format!("unsupported scalar: {:?}", other)),
    }
}
/// 比较两个值。只要有一边是 NULL，结果就是 unknown（不是 false）。
fn compare(l: &Value, op: &BinOp, r: &Value) -> Result<Tri, String> {
    if matches!(l, Value::Null) || matches!(r, Value::Null) {
        return Ok(Tri::Unknown);
    }
    let ord = match (l, r) {
        (Value::Int(a), Value::Int(b)) => a.cmp(b),
        (Value::Str(a), Value::Str(b)) => a.cmp(b),
        _ => return Err(format!("cannot compare {:?} with {:?}", l, r)),
    };
    Ok(Tri::from(match op {
        BinOp::Eq => ord == Ordering::Equal,
        BinOp::Neq => ord != Ordering::Equal,
        BinOp::Lt => ord == Ordering::Less,
        BinOp::Le => ord != Ordering::Greater,
        BinOp::Gt => ord == Ordering::Greater,
        BinOp::Ge => ord != Ordering::Less,
    }))
}

/// LIKE 匹配（支持 % 和 _ 两个通配符，双指针回溯法）
///
/// `%` = 0 个或多个字符，需要"先少匹配、失败再多一个"的回溯状态；
/// `_` = 恰好 1 个字符，匹配就前进、没有第二种选择，因此不需要额外状态。
/// 不支持 ESCAPE：模式里的 `\` 就是普通字符。
fn like_match(text: &str, pattern: &str) -> bool {
    let t: Vec<char> = text.chars().collect();
    let p: Vec<char> = pattern.chars().collect();
    let (mut ti, mut pi) = (0usize, 0usize);
    let (mut last_star_p, mut last_star_t) = (usize::MAX, 0usize);

    while ti < t.len() {
        // `_` 与"字符相等"的处理完全一样：占用对端一个字符后双双前进
        if pi < p.len() && (p[pi] == t[ti] || p[pi] == '_') {
            ti += 1; pi += 1;                      // 匹配成功，双双前进
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

/// 评估整个过滤表达式 → Tri（递归下降——和 Parser 的树一一对应）
fn eval_expr(expr: &Expr, col_names: &[String], vals: &[Value]) -> Result<Tri, String> {
    match expr {
        Expr::BinOp { left, op, right } => {
            let l = eval_scalar(left, col_names, vals)?;
            let r = eval_scalar(right, col_names, vals)?;
            compare(&l, op, &r)
        }
        Expr::And(a, b) => Ok(eval_expr(a, col_names, vals)?.and(eval_expr(b, col_names, vals)?)),
        Expr::Or(a, b) => Ok(eval_expr(a, col_names, vals)?.or(eval_expr(b, col_names, vals)?)),
        Expr::Not(a) => Ok(eval_expr(a, col_names, vals)?.not()),
        Expr::Like { expr, pattern } => {
            let v = eval_scalar(expr, col_names, vals)?;
            // NULL LIKE 'x%' 也是 unknown
            if matches!(v, Value::Null) {
                return Ok(Tri::Unknown);
            }
            Ok(Tri::from(like_match(&v.to_string_readable(), pattern)))
        }
        Expr::IsNull { expr, negated } => {
            // IS NULL 是唯一能把 NULL 判成真值的运算符，结果永远不是 unknown
            let v = eval_scalar(expr, col_names, vals)?;
            let is_null = matches!(v, Value::Null);
            Ok(Tri::from(is_null != *negated))
        }
        other => Err(format!("unsupported filter expression: {:?}", other)),
    }
}

impl From<bool> for Tri {
    fn from(b: bool) -> Self {
        if b { Tri::True } else { Tri::False }
    }
}
/// 排序专用比较：NULL 排最前（MySQL `ORDER BY asc` 的行为）。
/// 类型不一致时按类型定序，保证排序全函数都有定义、不 panic。
fn cmp_for_sort(a: &Value, b: &Value) -> Ordering {
    match (a, b) {
        (Value::Null, Value::Null) => Ordering::Equal,
        (Value::Null, _) => Ordering::Less,
        (_, Value::Null) => Ordering::Greater,
        (Value::Int(x), Value::Int(y)) => x.cmp(y),
        (Value::Str(x), Value::Str(y)) => x.cmp(y),
        (Value::Int(_), Value::Str(_)) => Ordering::Less,
        (Value::Str(_), Value::Int(_)) => Ordering::Greater,
    }
}
// 求值一条 JOIN 的 ON 条件（当前只支持等值）。
/// 内连接语义：任一边是 NULL 都算“不匹配”——NULL = NULL 是 unknown，不是 true。
fn eval_join_cond(join: &JoinClause, col_names: &[String], vals: &[Value]) -> Result<bool, String> {
    let li = resolve_column(&join.left_col, col_names)?;//
    let ri = resolve_column(&join.right_col, col_names)?;

    let l = vals.get(li)
        .ok_or_else(|| format!("JOIN ON references a table not joined yet: {}", join.left_col))?;
    let r = vals.get(ri)
        .ok_or_else(|| format!("JOIN ON references a table not joined yet: {}", join.right_col))?;

    if matches!(l, Value::Null) || matches!(r, Value::Null) {
        return Ok(false);
    }
    Ok(l == r)
}

/// 递归拼接：depth = “正在接第几张表”（tables[0] 是主表）。
/// 接第 depth 张表时，用 joins[depth-1] 剪枝；接完所有表就把当前组合收进 out。
fn join_dfs(
    rows_by_table: &[Vec<Vec<Value>>],
    joins: &[JoinClause],
    depth: usize,
    current: &mut Vec<Value>,
    col_names: &[String],
    out: &mut Vec<Vec<Value>>,
) -> Result<(), String> {
    if depth == rows_by_table.len() {
        out.push(current.clone());
        return Ok(());
    }

    for row in &rows_by_table[depth] {
        let saved = current.len();
        current.extend_from_slice(row);

        let matched = if depth == 0 {
            true   // 主表不需要 ON
        } else {
            eval_join_cond(&joins[depth - 1], col_names, current)?
        };

        if matched {
            join_dfs(rows_by_table, joins, depth + 1, current, col_names, out)?;
        }
        // 回溯：把这张表的值摘掉，换下一行
        current.truncate(saved);
    }
    Ok(())
}

/// 聚合累加器：边遍历组内行边算，不把整组值留在内存里。
#[derive(Default)]
struct AggState {
    /// 非 NULL 的个数：COUNT(col) 只数它；SUM/AVG 判断"空集"也靠它
    non_null: usize,
    sum: i64,
    /// 出现过字符串：SUM/AVG 遇到 VARCHAR 要报错，而不是把字节当数字
    has_str: bool,
    min: Option<Value>,
    max: Option<Value>,
}

impl AggState {
    fn update(&mut self, v: &Value) {
        // NULL 不参与任何聚合（COUNT(*) 用的是行数，不经过这里）
        if matches!(v, Value::Null) {
            return;
        }
        self.non_null += 1;
        match v {
            Value::Int(n) => self.sum += *n,
            Value::Str(_) => self.has_str = true,
            Value::Null => {}   // 上面已 return，这里只为让 match 穷尽
        }
        if self.min.as_ref().map_or(true, |m| cmp_for_sort(v, m) == Ordering::Less) {
            self.min = Some(v.clone());
        }
        if self.max.as_ref().map_or(true, |m| cmp_for_sort(v, m) == Ordering::Greater) {
            self.max = Some(v.clone());
        }
    }

    fn finish(&self, func: &AggFunc) -> Result<Value, String> {
        match func {
            // COUNT(col)：只数非 NULL
            AggFunc::Count => Ok(Value::Int(self.non_null as i64)),
            AggFunc::Min => Ok(self.min.clone().unwrap_or(Value::Null)),
            AggFunc::Max => Ok(self.max.clone().unwrap_or(Value::Null)),
            AggFunc::Sum | AggFunc::Avg => {
                // ★ 空集（含全 NULL）返回 NULL，不是 0
                if self.non_null == 0 {
                    return Ok(Value::Null);
                }
                if self.has_str {
                    return Err(format!("{:?} requires an INT column", func));
                }
                if matches!(func, AggFunc::Avg) {
                    // 只有 INT，没有小数：整数除法，向零截断（教学取舍，写进边界表）
                    Ok(Value::Int(self.sum / self.non_null as i64))
                } else {
                    Ok(Value::Int(self.sum))
                }
            }
        }
    }
}

/// 输出列名：SUM(amount) / COUNT(*) 这种"函数式"名字
fn agg_name(func: &AggFunc, column: &str) -> String {
    let f = match func {
        AggFunc::Sum => "SUM",
        AggFunc::Count => "COUNT",
        AggFunc::Avg => "AVG",
        AggFunc::Min => "MIN",
        AggFunc::Max => "MAX",
    };
    format!("{}({})", f, column)
}

/// 分组 + 聚合：输入"WHERE 过滤后的完整行"，输出"每个组一行的结果"。
///
/// 三种情形共用这一条路：
///   - 有聚合 + 有 GROUP BY：按 key 分组，每组一行
///   - 有聚合 + 无 GROUP BY：全表一个组（空表也要产出一个空组）
///   - 无聚合 + 有 GROUP BY：就是 DISTINCT 的效果（每组取一行）
fn aggregate_rows(
    columns: &[SelectItem],
    group_by: &[String],
    col_names: &[String],
    multi: bool,
    decoded: Vec<Vec<Value>>,
) -> Result<(Vec<String>, Vec<Vec<Value>>), String> {
    // ① 分组列 → 下标
    let group_idx: Vec<usize> = group_by
        .iter()
        .map(|c| resolve_column(c, col_names))
        .collect::<Result<Vec<_>, _>>()?;

    // ② 先把 SELECT 列表校验完，再动数据：避免算到一半才发现语义错
    for item in columns {
        match item {
            SelectItem::Star => {
                return Err("SELECT * cannot be combined with GROUP BY or aggregate functions".into());
            }
            SelectItem::Column(name) => {
                let i = resolve_column(name, col_names)?;
                if !group_idx.contains(&i) {
                    return Err(format!(
                        "column '{}' must appear in GROUP BY or be used in an aggregate", name));
                }
            }
            SelectItem::TableColumn { table, col } => {
                let full = format!("{}.{}", table, col);
                let i = resolve_column(&full, col_names)?;
                if !group_idx.contains(&i) {
                    return Err(format!(
                        "column '{}' must appear in GROUP BY or be used in an aggregate", full));
                }
            }
            SelectItem::Aggregate { column, .. } => {
                // "*" 是 COUNT(*) 的哨兵，不需要解析；其它列名早点报 not found
                if column != "*" {
                    resolve_column(column, col_names)?;
                }
            }
        }
    }

    // ③ 建组：key 用 Option<String>，NULL 会聚成同一组（SQL 语义），
    //    又不会跟字符串 "NULL" 撞车。Vec 保"首次出现顺序"，HashMap 只负责定位。
    let mut groups: Vec<Vec<Vec<Value>>> = Vec::new();
    let mut index: HashMap<Vec<Option<String>>, usize> = HashMap::new();
    for row in decoded {
        let key: Vec<Option<String>> = group_idx.iter().map(|&i| row[i].to_cell()).collect();
        match index.get(&key) {
            Some(&g) => groups[g].push(row),
            None => {
                index.insert(key, groups.len());
                groups.push(vec![row]);
            }
        }
    }

    // ④ 没有 GROUP BY 的聚合是"全表一个组"：即使一行都没有，也要补一个空组，
    //    否则 COUNT(*) 会返回 0 行，而不是 1 行 0。
    if group_idx.is_empty() && groups.is_empty() {
        groups.push(Vec::new());
    }

    // ⑤ 输出列名只算一次（与组数无关；放进逐组循环会重复 push）
    let mut out_names: Vec<String> = Vec::with_capacity(columns.len());
    for item in columns {
        out_names.push(match item {
            SelectItem::Star => unreachable!("已在 ② 拦掉"),
            SelectItem::Column(name) => {
                let i = resolve_column(name, col_names)?;
                if multi { col_names[i].clone() } else { short_name(&col_names[i]).to_string() }
            }
            SelectItem::TableColumn { table, col } => {
                let full = format!("{}.{}", table, col);
                let i = resolve_column(&full, col_names)?;
                if multi { col_names[i].clone() } else { col.to_string() }
            }
            SelectItem::Aggregate { func, column } => agg_name(func, column),
        });
    }

    // ⑥ 逐组算值
    let mut out_rows: Vec<Vec<Value>> = Vec::with_capacity(groups.len());
    for rows in &groups {
        let mut vals: Vec<Value> = Vec::with_capacity(columns.len());
        for item in columns {
            match item {
                SelectItem::Star => unreachable!("已在 ② 拦掉"),
                // 分组列：同组内取值相同，取第一行即可
                SelectItem::Column(name) => {
                    let i = resolve_column(name, col_names)?;
                    let first = rows.first().ok_or("internal: plain column in an empty group")?;
                    vals.push(first[i].clone());
                }
                SelectItem::TableColumn { table, col } => {
                    let full = format!("{}.{}", table, col);
                    let i = resolve_column(&full, col_names)?;
                    let first = rows.first().ok_or("internal: plain column in an empty group")?;
                    vals.push(first[i].clone());
                }
                SelectItem::Aggregate { func, column } => {
                    if column == "*" {
                        // 只有 COUNT(*) 能走到这里（parser 已保证）
                        vals.push(Value::Int(rows.len() as i64));
                    } else {
                        let i = resolve_column(column, col_names)?;
                        let mut st = AggState::default();
                        for r in rows {
                            st.update(&r[i]);
                        }
                        vals.push(st.finish(func)?);
                    }
                }
            }
        }
        out_rows.push(vals);
    }

    Ok((out_names, out_rows))
}

/// DISTINCT：按"输出列的值"去重，保留首次出现的那一行。
///
/// key_idx 是"输出列在整行里的下标"，两个分支各传各的：
///   普通查询  → proj（SELECT 了哪几列）
///   聚合查询  → 0..out_names.len()（整行就是输出）
///
/// 用 to_cell() 得到的 Option<String> 当 key，和 aggregate_rows 的分组键同一套规则：
/// NULL 是 None，字符串 'NULL' 是 Some("NULL")，两者不会撞车。
///
/// 调用前必须已经排好序：DISTINCT 保留"首个"，所以去掉哪些重复行取决于顺序，
/// 但保留下来的行仍然维持排序后的相对顺序。
fn take_distinct(rows: Vec<Vec<Value>>, key_idx: &[usize]) -> Vec<Vec<Value>> {
    let mut seen: HashSet<Vec<Option<String>>> = HashSet::new();
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let key: Vec<Option<String>> = key_idx.iter().map(|&i| row[i].to_cell()).collect();
        if seen.insert(key) {
            out.push(row);
        }
    }
    out
}
fn execute_select(
    columns: Vec<SelectItem>,
    distinct: bool,
    table_name: String,
    joins: Vec<JoinClause>,
    filter: Option<Expr>,
    group_by: Vec<String>,
    having: Option<Expr>,
    order_by: Vec<OrderByItem>,
    limit: Option<usize>,
    db: &mut Database,
) -> Result<ResultSet, String> {
    let multi = !joins.is_empty();

    // ① 打开所有表：主表在前，JOIN 表按书写顺序跟在后面
    let mut tables: Vec<(String, Table)> = Vec::with_capacity(joins.len() + 1);
    tables.push((
        table_name.clone(),
        db.open_table(&table_name)
            .map_err(|e| format!("open_table failed: {}", e))?,
    ));
    for j in &joins {
        let t = db.open_table(&j.table)
            .map_err(|e| format!("open_table '{}' failed: {}", j.table, e))?;
        tables.push((j.table.clone(), t));
    }
    // ② 列名空间：一律 "表名.列名"。
    //    这样单表写 j_a.name 也能精确命中（标准 SQL），不用靠短名猜。
    let mut col_names: Vec<String> = Vec::new();
    for (tname, t) in &tables {
        for c in &t.columns {
            col_names.push(format!("{}.{}", tname, c.name));
        }
    }
    let width = col_names.len();

    // ③ 逐表扫描 + 解码。存储层到这里就够了：scan_all 交出全部行
    let mut rows_by_table: Vec<Vec<Vec<Value>>> = Vec::with_capacity(tables.len());
    for (_, t) in &tables {
        let scanned = t.scan_all().map_err(|e| format!("scan_all failed: {}", e))?;
        let mut rows = Vec::with_capacity(scanned.len());
        for (_, rec) in &scanned {
            rows.push(decode_row(t, rec)?);
        }
        rows_by_table.push(rows);
    }

    // ④ 拼接：单表直接取，多表走带 ON 剪枝的嵌套循环
    let mut decoded: Vec<Vec<Value>> = Vec::new();
    if multi {
        let mut cur: Vec<Value> = Vec::with_capacity(width);
        join_dfs(&rows_by_table, &joins, 0, &mut cur, &col_names, &mut decoded)?;
    } else {
        decoded = rows_by_table.pop().unwrap_or_default();
    }

    // ⑤ WHERE：在拼接后的完整行上求值（先筛行，后删列）
    if let Some(f) = &filter {
        let mut kept = Vec::with_capacity(decoded.len());
        for vals in decoded {
            if eval_expr(f, &col_names, &vals)? == Tri::True {
                kept.push(vals);
            }
        }
        decoded = kept;
    }


    // ⑥ 聚合分支：只要出现聚合函数、或写了 GROUP BY，就走"先分组、再算值"
    //    位置很关键：在 WHERE 之后（先筛行），在投影/ORDER BY 之前（后面排序的是聚合结果）
    let has_agg = columns.iter().any(|c| matches!(c, SelectItem::Aggregate { .. }));

    // HAVING 必须有"组"才成立：GROUP BY 造组，或者聚合把全表当一个组。
    // 两者都没有时如果放过去，HAVING 会被静默忽略 —— 那是最坏的结果。
    if having.is_some() && !has_agg && group_by.is_empty() {
        return Err("HAVING requires GROUP BY or an aggregate function in the SELECT list".into());
    }

    if has_agg || !group_by.is_empty() {
        let (out_names, mut out_rows) =
            aggregate_rows(&columns, &group_by, &col_names, multi, decoded)?;

        // ⑥-1 HAVING：在排序和 LIMIT 之前筛"组"。
        // 这里的 `?` 在 execute_select 函数体里，不在闭包里，
        // 所以 SUM(VARCHAR) 这种错误会一路回到客户端，而不是被当成"不匹配"。
        if let Some(h) = &having {
            let mut kept: Vec<Vec<Value>> = Vec::with_capacity(out_rows.len());
            for vals in out_rows {
                if eval_expr(h, &out_names, &vals)? == Tri::True {
                    kept.push(vals);
                }
            }
            out_rows = kept;
        }

        let mut sort_keys: Vec<(usize, bool)> = Vec::new();
        for item in &order_by {
            let name = order_key_name(&item.expr)?;
            let idx = resolve_column(&name, &out_names).map_err(|_| {
                format!("ORDER BY '{}' must be a column in the select list when aggregating", name)
            })?;
            sort_keys.push((idx, item.asc));
        }
        if !sort_keys.is_empty() {
            out_rows.sort_by(|a, b| {
                for &(idx, asc) in &sort_keys {
                    let ord = cmp_for_sort(&a[idx], &b[idx]);
                    if ord != Ordering::Equal {
                        return if asc { ord } else { ord.reverse() };
                    }
                }
                Ordering::Equal
            });
        }
        // ⑩ DISTINCT 对聚合结果同样生效：
        //    SELECT DISTINCT SUM(amount) FROM t GROUP BY dept → 两个组 SUM 相同时只留一行
        //    位置：排序之后、LIMIT 之前（先截断再去重会少给行）
        if distinct {
            let all_cols: Vec<usize> = (0..out_names.len()).collect();
            out_rows = take_distinct(out_rows, &all_cols);
        }

        if let Some(limit) = limit {
            out_rows = out_rows.into_iter().take(limit).collect();
        }

        let rows: Vec<Vec<Option<String>>> = out_rows
            .iter()
            .map(|vals| vals.iter().map(|v| v.to_cell()).collect())
            .collect();
        return Ok(ResultSet::Rows { columns: out_names, rows });
    }
    // ↓ 以下原来的 ⑥⑦⑧ 顺延成 ⑦⑧⑨（只是注释编号）



    // ⑥ 投影：Star → 全列；Column → 短名解析；TableColumn → 限定名解析
    let proj: Vec<usize> = match &columns[..] {
        [SelectItem::Star] => (0..width).collect(),
        items => {
            let mut idxs = Vec::with_capacity(items.len());
            for item in items {
                match item {
                    SelectItem::Column(name) => idxs.push(resolve_column(name, &col_names)?),
                    SelectItem::TableColumn { table, col } => {
                        let full = format!("{}.{}", table, col);
                        idxs.push(resolve_column(&full, &col_names)?);
                    }
                    other => return Err(format!("unsupported select item: {:?}", other)),
                }
            }
            idxs
        }
    };
    // DISTINCT 时，排序键必须在输出列里。
    // 否则"这一行按哪个 amount 排"没有定义（见答辩要点：DISTINCT 之后只剩输出列）。
    if distinct {
        for item in &order_by {
            let name = order_key_name(&item.expr)?;
            let in_output = resolve_column(&name, &col_names)
                .ok()
                .map_or(false, |i| proj.contains(&i));
            if !in_output {
                return Err(format!(
                    "ORDER BY '{}' must be a column in the select list when using DISTINCT", name));
            }
        }
    }

    // ⑦ ORDER BY（在投影之前排序：可能按没被 SELECT 的列排）
    let mut sort_keys: Vec<(usize, bool)> = Vec::new();
    for item in &order_by {
        // 走到这里说明 SELECT 里没有任何聚合函数，也就没有"组"可算。
        // 必须在这里挡住：否则会掉进 resolve_column 报 "column 'SUM(amount)' not found"，
        // 让人以为列名写错了，而不是"这个位置不能放聚合"。
        if matches!(&item.expr, Expr::Aggregate { .. }) {
            return Err(
                "ORDER BY cannot use an aggregate function unless the query aggregates".into());
        }
        let name = order_key_name(&item.expr)?;
        sort_keys.push((resolve_column(&name, &col_names)?, item.asc));
    }
    if !sort_keys.is_empty() {
        decoded.sort_by(|a, b| {
            for &(idx, asc) in &sort_keys {
                let ord = cmp_for_sort(&a[idx], &b[idx]);
                if ord != Ordering::Equal {
                    return if asc { ord } else { ord.reverse() };
                }
            }
            Ordering::Equal
        });
    }
    // ⑨ DISTINCT：按 proj 指定的输出列去重。
    //    必须在 LIMIT 之前 —— 否则 SELECT DISTINCT dept ... LIMIT 2
    //    会先取到 A,A 再去重，只剩 1 行。
    if distinct {
        decoded = take_distinct(decoded, &proj);
    }

    if let Some(limit) = limit {
        decoded = decoded.into_iter().take(limit).collect();
    }

    // ⑧ 投影落地 + 输出列名（多表时就是 "表名.列名"）
    let rows: Vec<Vec<Option<String>>> = decoded
        .iter()
        .map(|vals| proj.iter().map(|&i| vals[i].to_cell()).collect())
        .collect();
    // 单表输出裸列名（与旧版一致），多表输出 "表名.列名"
    let out_col_names: Vec<String> = proj
        .iter()
        .map(|&i| {
            if multi {
                col_names[i].clone()
            } else {
                short_name(&col_names[i]).to_string()
            }
        })
        .collect();
    Ok(ResultSet::Rows { columns: out_col_names, rows })
}
// 删除 WHERE 为 TRUE 的行（无 WHERE 时删全表）
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
            // 同样是"只删 WHERE 为 TRUE 的行"：age = NULL 的行不会被 DELETE ... WHERE age > 18 删掉
            if eval_expr(f, &all_col_names, &vals)? != Tri::True {
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

/// 唯一性预检：把"改完之后"每个唯一列的取值算出来，逐列检查新值之间、以及新值与"没被改的旧行"是否撞车。
/// 必须在动任何数据之前跑完，否则删了旧行才发现冲突，表就残了。
/// 与存储层 check_unique_constraints 一致：**逐列独立判定**，不能把多列当复合键。
fn check_update_unique_keys(table: &Table, plan: &[(RowId, Vec<Value>)]) -> Result<(), String> {
    let unique_cols: Vec<usize> = table.columns.iter().enumerate()
        .filter(|(_, c)| c.primary_key || c.unique)
        .map(|(i, _)| i)
        .collect();
    if unique_cols.is_empty() {
        return Ok(());
    }

    let changing: HashSet<RowId> = plan.iter().map(|(id, _)| *id).collect();
    // occupied[pos] = 该唯一列上已被占用的非 NULL 值（没被改的旧行 + 已排队的改后新行）
    let mut occupied: Vec<HashSet<String>> = unique_cols.iter().map(|_| HashSet::new()).collect();

    // ① 没被 UPDATE 碰到的行 → 它们的键继续被占用（含 NULL 的格不占，SQL 标准下 NULL 不冲突）
    for (row_id, record) in table.scan_all().map_err(|e| format!("scan_all failed: {}", e))? {
        if changing.contains(&row_id) {
            continue;
        }
        let vals = decode_row(table, &record)?;
        for (pos, &ci) in unique_cols.iter().enumerate() {
            if let Some(s) = vals[ci].to_cell() {
                occupied[pos].insert(s);
            }
        }
    }

    // ② 新行逐列占坑，某列 insert 失败 = 自己跟自己或跟旧行重复
    for (_, new_vals) in plan {
        for (pos, &ci) in unique_cols.iter().enumerate() {
            if let Some(s) = new_vals[ci].to_cell() {
                if !occupied[pos].insert(s) {
                    let col = &table.columns[ci];
                    let kind = if col.primary_key { "PRIMARY KEY" } else { "UNIQUE" };
                    return Err(format!("Duplicate value for {} column '{}'", kind, col.name));
                }
            }
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
            if eval_expr(f, &all_col_names, &old_vals)? != Tri::True {
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

    // ③ 先按 schema 编码成 Row（类型/长度/NOT NULL 错误在这一步暴露），再预检唯一键
    let mut rows: Vec<(RowId, Row)> = Vec::with_capacity(plan.len());
    for (row_id, new_vals) in &plan {
        let mut builder = table.new_row();
        for (i, v) in new_vals.iter().enumerate() {
            builder = match v {
                Value::Int(n) => builder.put_int(*n as u32),
                Value::Str(s) => builder.put_varchar(s),
                Value::Null => builder.put_null(),   // SET col = NULL；列是 NOT NULL 会被 Table 层拒
            }
            .map_err(|e| format!("column '{}': {}", all_col_names[i], e))?;
        }
        let row = builder.build().map_err(|e| format!("build row failed: {}", e))?;
        rows.push((*row_id, row));
    }
    check_update_unique_keys(&table, &plan)?;

    // ④ 落盘：优先原地覆盖；只有记录变长放不下时才退化成「删旧 + 插新」
    let count = rows.len();
    for (row_id, row) in rows {
        let in_place = table
            .try_update_in_place(row_id, &row, db.txn_mgr(), session.txn_id)
            .map_err(|e| format!("update row failed: {}", e))?;
        if in_place {
            continue;
        }
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
        execute(stmt, db, None, session)
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

    /// e2e 测试专用库：与生产 ./data/db.db 完全隔离。
    ///
    /// 以前每个测试都 remove_file(config::DB_FILE)，跑一趟 cargo test 就会把
    /// 手工建的库全删掉。现在所有 e2e 统一走这两个文件，生产库不再被碰。
    const TEST_DB: &str = "./data/test_executor.db";
    const TEST_LOG: &str = "./data/test_executor.log";

    fn open_test_db() -> Database {
        let _ = std::fs::remove_file(TEST_DB);
        let _ = std::fs::remove_file(TEST_LOG);
        Database::open_at(TEST_DB, TEST_LOG).expect("open test db")
    }

    /// 测试断言辅助：一个单元格 Option<String> → 可读字符串（NULL 就写成 "NULL"）。
    /// 这样断言仍然能写成 `cell(&rows[0][2]) == "Alice"`，不用到处 `unwrap()`。
    fn cell(c: &Option<String>) -> String {
        c.clone().unwrap_or_else(|| "NULL".to_string())
    }

    /// 一行 Option<String> → 一行 String
    fn cells(row: &[Option<String>]) -> Vec<String> {
        row.iter().map(cell).collect()
    }

    /// 整个结果集 → String（NULL 显示为 "NULL"）
    fn cells_all(rows: &[Vec<Option<String>>]) -> Vec<Vec<String>> {
        rows.iter().map(|r| cells(r)).collect()
    }

    #[test]
    fn test_e2e_create_and_insert() {
        let _guard = db_lock();
        let mut db = open_test_db();
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
        let mut db = open_test_db();
        let mut session = Session::new();

        exec_sql(&mut db, &mut session, "CREATE TABLE q_users (id INT PRIMARY KEY, name VARCHAR(50))").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO q_users VALUES (1, 'Alice')").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO q_users VALUES (2, 'Bob')").unwrap();

        let r = exec_sql(&mut db, &mut session, "SELECT * FROM q_users").unwrap();
        match r {
            ResultSet::Rows { columns, rows } => {
                assert_eq!(columns, vec!["id".to_string(), "name".to_string()]);
                assert_eq!(rows.len(), 2);
                assert_eq!(cells(&rows[0]), vec!["1".to_string(), "Alice".to_string()]);
                assert_eq!(cells(&rows[1]), vec!["2".to_string(), "Bob".to_string()]);
            }
            other => panic!("expected Rows, got {:?}", other),
        }

        let _ = db.shutdown();
    }
    #[test]
    fn test_e2e_select_where_gt() {
        let _guard = db_lock();
        let mut db = open_test_db();
        let mut session = Session::new();

        exec_sql(&mut db, &mut session, "CREATE TABLE w_emp (id INT PRIMARY KEY, age INT, name VARCHAR(50))").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO w_emp VALUES (1, 20, 'Alice')").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO w_emp VALUES (2, 17, 'Bob')").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO w_emp VALUES (3, 25, 'Carter')").unwrap();

        let r = exec_sql(&mut db, &mut session, "SELECT * FROM w_emp WHERE age > 18").unwrap();
        match r {
            ResultSet::Rows { rows, .. } => {
                assert_eq!(rows.len(), 2);                     // Alice 和 Carter
                assert_eq!(cell(&rows[0][2]), "Alice".to_string());
                assert_eq!(cell(&rows[1][2]), "Carter".to_string());
            }
            other => panic!("expected Rows, got {:?}", other),
        }
        let _ = db.shutdown();
    }

    #[test]
    fn test_e2e_select_where_and_like() {
        let _guard = db_lock();
        let mut db = open_test_db();
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
                assert_eq!(cell(&rows[0][2]), "Alice".to_string());
                assert_eq!(cell(&rows[1][2]), "Alan".to_string());
            }
            other => panic!("expected Rows, got {:?}", other),
        }
        let _ = db.shutdown();
    }

    #[test]
    fn test_select_unknown_column_errors() {
        let _guard = db_lock();
        let mut db = open_test_db();
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
        let mut db = open_test_db();
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
                assert_eq!(cells_all(&rows), vec![
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
        let mut db = open_test_db();
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
                assert!(!rows.iter().any(|row| cell(&row[0]) == "Bob"));
            }
            other => panic!("expected Rows, got {:?}", other),
        }
        let _ = db.shutdown();
    }
    #[test]
    fn test_e2e_begin_commit() {
        let _guard = db_lock();
        let mut db = open_test_db();
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
        let mut db = open_test_db();
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
        let mut db = open_test_db();
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
                assert!(rows.iter().any(|r| cell(&r[0]) == "Alice"), "Alice 应该被恢复");
            }
            other => panic!("got {:?}", other),
        }
        let _ = db.shutdown();
    }

    #[test]
    fn test_e2e_update_where() {
        let _guard = db_lock();
        let mut db = open_test_db();
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
                assert_eq!(cells(&rows[0]), vec!["1".to_string(), "21".to_string(), "Alicia".to_string()]);
                assert_eq!(cells(&rows[1]), vec!["2".to_string(), "17".to_string(), "Bob".to_string()]);
                assert_eq!(cells(&rows[2]), vec!["3".to_string(), "25".to_string(), "Carter".to_string()]);
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
    fn test_e2e_select_order_by() {
        let _guard = db_lock();
        let mut db = open_test_db();
        let mut session = Session::new();

        exec_sql(&mut db, &mut session, "CREATE TABLE o_emp (id INT PRIMARY KEY, age INT, name VARCHAR(50))").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO o_emp VALUES (3, 25, 'Carter')").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO o_emp VALUES (1, 20, 'Alice')").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO o_emp VALUES (2, 20, 'Bob')").unwrap();

        // 单列升序（默认）
        match exec_sql(&mut db, &mut session, "SELECT id FROM o_emp ORDER BY id").unwrap() {
            ResultSet::Rows { rows, .. } => {
                assert_eq!(cells_all(&rows), vec![
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
                assert_eq!(cells_all(&rows), vec![
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
                assert_eq!(cells_all(&rows), vec![
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
                assert_eq!(cells_all(&rows), vec![
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

    // ==================== NULL 专道 ====================

    /// NULL 的完整链路：写入 → 位图编码 → 读回 → IS NULL 过滤
    #[test]
    fn test_e2e_null_roundtrip_and_is_null() {
        let _guard = db_lock();
        let mut db = open_test_db();
        let mut session = Session::new();

        exec_sql(&mut db, &mut session,
                 "CREATE TABLE n_emp (id INT PRIMARY KEY, name VARCHAR(50), age INT)").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO n_emp VALUES (1, 'Alice', 20)").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO n_emp VALUES (2, NULL, 30)").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO n_emp VALUES (3, 'Bob', NULL)").unwrap();

        // SELECT *：第 2 行的 name、第 3 行的 age 都该是 None，不能是空字符串
        match exec_sql(&mut db, &mut session, "SELECT * FROM n_emp ORDER BY id").unwrap() {
            ResultSet::Rows { rows, .. } => {
                assert_eq!(rows.len(), 3);
                assert_eq!(rows[1], vec![Some("2".into()), None, Some("30".into())]);
                assert_eq!(rows[2], vec![Some("3".into()), Some("Bob".into()), None]);
            }
            other => panic!("got {:?}", other),
        }

        // IS NULL 是唯一能把 NULL 判成真的运算符
        match exec_sql(&mut db, &mut session, "SELECT id FROM n_emp WHERE name IS NULL").unwrap() {
            ResultSet::Rows { rows, .. } => assert_eq!(cells_all(&rows), vec![vec!["2".to_string()]]),
            other => panic!("got {:?}", other),
        }
        match exec_sql(&mut db, &mut session, "SELECT id FROM n_emp WHERE age IS NOT NULL ORDER BY id").unwrap() {
            ResultSet::Rows { rows, .. } => assert_eq!(cells_all(&rows), vec![
                vec!["1".to_string()],
                vec!["2".to_string()],
            ]),
            other => panic!("got {:?}", other),
        }

        let _ = db.shutdown();
    }

    /// 往 NOT NULL 列（包括 PRIMARY KEY）写 NULL → 存储层拒
    #[test]
    fn test_e2e_not_null_rejects_null() {
        let _guard = db_lock();
        let mut db = open_test_db();
        let mut session = Session::new();

        exec_sql(&mut db, &mut session,
                 "CREATE TABLE nn_emp (id INT PRIMARY KEY, name VARCHAR(20) NOT NULL, age INT)").unwrap();

        let err = exec_sql(&mut db, &mut session, "INSERT INTO nn_emp VALUES (1, NULL, 20)").unwrap_err();
        assert!(err.contains("NOT NULL"), "got: {}", err);

        // 主键隐含 NOT NULL
        let err = exec_sql(&mut db, &mut session, "INSERT INTO nn_emp VALUES (NULL, 'Alice', 20)").unwrap_err();
        assert!(err.contains("NOT NULL"), "got: {}", err);

        // age 可空 → 正常写入
        exec_sql(&mut db, &mut session, "INSERT INTO nn_emp VALUES (2, 'Bob', NULL)").unwrap();
        match exec_sql(&mut db, &mut session, "SELECT age FROM nn_emp WHERE id = 2").unwrap() {
            ResultSet::Rows { rows, .. } => assert_eq!(rows[0][0], None),
            other => panic!("got {:?}", other),
        }

        let _ = db.shutdown();
    }

    /// 三值逻辑：NULL 参与比较一律 unknown，WHERE / NOT / OR 都要按三值算
    #[test]
    fn test_e2e_three_valued_logic_where() {
        let _guard = db_lock();
        let mut db = open_test_db();
        let mut session = Session::new();

        exec_sql(&mut db, &mut session, "CREATE TABLE t3 (id INT PRIMARY KEY, age INT)").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO t3 VALUES (1, 20)").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO t3 VALUES (2, NULL)").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO t3 VALUES (3, 10)").unwrap();

        let ids = |sql: &str, db: &mut Database, session: &mut Session| -> Vec<String> {
            match exec_sql(db, session, sql).unwrap() {
                ResultSet::Rows { rows, .. } => rows.iter().map(|r| cell(&r[0])).collect(),
                other => panic!("got {:?}", other),
            }
        };

        // age > 18：NULL 那行既不满足也不违反，就是不选
        assert_eq!(ids("SELECT id FROM t3 WHERE age > 18 ORDER BY id", &mut db, &mut session),
                   vec!["1".to_string()]);
        // NOT (age > 18)：unknown 取反仍是 unknown → 还是只剩 id=3
        assert_eq!(ids("SELECT id FROM t3 WHERE NOT age > 18 ORDER BY id", &mut db, &mut session),
                   vec!["3".to_string()]);
        // age = NULL 永远 unknown（初学 SQL 最常踩的坑）→ 0 行
        assert!(ids("SELECT id FROM t3 WHERE age = NULL", &mut db, &mut session).is_empty());
        // unknown OR true = true → NULL 行靠 IS NULL 拉回来
        assert_eq!(
            ids("SELECT id FROM t3 WHERE age > 18 OR age IS NULL ORDER BY id", &mut db, &mut session),
            vec!["1".to_string(), "2".to_string()]
        );

        let _ = db.shutdown();
    }

    /// UPDATE 可以把列改成 NULL；改 NOT NULL 列时被拒且不动数据
    #[test]
    fn test_e2e_update_set_null() {
        let _guard = db_lock();
        let mut db = open_test_db();
        let mut session = Session::new();

        exec_sql(&mut db, &mut session,
                 "CREATE TABLE u4 (id INT PRIMARY KEY, name VARCHAR(20), age INT)").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO u4 VALUES (1, 'Alice', 20)").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO u4 VALUES (2, 'Bob', 30)").unwrap();

        let r = exec_sql(&mut db, &mut session, "UPDATE u4 SET name = NULL WHERE id = 1").unwrap();
        assert!(matches!(r, ResultSet::Affected(1)), "got {:?}", r);

        match exec_sql(&mut db, &mut session, "SELECT * FROM u4 ORDER BY id").unwrap() {
            ResultSet::Rows { rows, .. } => {
                assert_eq!(rows[0], vec![Some("1".into()), None, Some("20".into())]);
                assert_eq!(rows[1], vec![Some("2".into()), Some("Bob".into()), Some("30".into())]);
            }
            other => panic!("got {:?}", other),
        }

        // 主键是 NOT NULL：报错，而且回退在“删旧行”之前，原数据一点没动
        let err = exec_sql(&mut db, &mut session, "UPDATE u4 SET id = NULL WHERE id = 2").unwrap_err();
        assert!(err.contains("NOT NULL"), "got: {}", err);
        match exec_sql(&mut db, &mut session, "SELECT * FROM u4 ORDER BY id").unwrap() {
            ResultSet::Rows { rows, .. } => {
                assert_eq!(rows.len(), 2, "报错前不能把行删掉");
                assert_eq!(cell(&rows[1][1]), "Bob");
            }
            other => panic!("got {:?}", other),
        }

        let _ = db.shutdown();
    }

    /// 排序时 NULL 排最前（MySQL `ORDER BY ... ASC` 的行为），DESC 那么它自然落到最后
    #[test]
    fn test_e2e_order_by_null_position() {
        let _guard = db_lock();
        let mut db = open_test_db();
        let mut session = Session::new();

        exec_sql(&mut db, &mut session, "CREATE TABLE o5 (id INT PRIMARY KEY, age INT)").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO o5 VALUES (1, 20)").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO o5 VALUES (2, NULL)").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO o5 VALUES (3, 10)").unwrap();

        match exec_sql(&mut db, &mut session, "SELECT id FROM o5 ORDER BY age").unwrap() {
            ResultSet::Rows { rows, .. } => assert_eq!(cells_all(&rows), vec![
                vec!["2".to_string()],   // NULL 最小，排第一
                vec!["3".to_string()],
                vec!["1".to_string()],
            ]),
            other => panic!("got {:?}", other),
        }
        match exec_sql(&mut db, &mut session, "SELECT id FROM o5 ORDER BY age DESC").unwrap() {
            ResultSet::Rows { rows, .. } => assert_eq!(cell(&rows[2][0]), "2", "DESC 时 NULL 排最后"),
            other => panic!("got {:?}", other),
        }

        let _ = db.shutdown();
    }

    /// DELETE 同样只删 WHERE 为 TRUE 的行：age 为 NULL 的行不会被 `age > 18` 误删
    #[test]
    fn test_e2e_delete_where_null() {
        let _guard = db_lock();
        let mut db = open_test_db();
        let mut session = Session::new();

        exec_sql(&mut db, &mut session, "CREATE TABLE d6 (id INT PRIMARY KEY, age INT)").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO d6 VALUES (1, 20)").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO d6 VALUES (2, NULL)").unwrap();

        let r = exec_sql(&mut db, &mut session, "DELETE FROM d6 WHERE age > 18").unwrap();
        assert!(matches!(r, ResultSet::Affected(1)), "got {:?}", r);

        match exec_sql(&mut db, &mut session, "SELECT * FROM d6").unwrap() {
            ResultSet::Rows { rows, .. } => {
                assert_eq!(rows.len(), 1, "NULL 行必须活着");
                assert_eq!(rows[0], vec![Some("2".into()), None]);
            }
            other => panic!("got {:?}", other),
        }

        let _ = db.shutdown();
    }

    /// 带列名列表的 INSERT：可以乱序、可以只写一部分，剩下的列补 NULL
    #[test]
    fn test_e2e_insert_column_list() {
        let _guard = db_lock();
        let mut db = open_test_db();
        let mut session = Session::new();

        exec_sql(&mut db, &mut session,
                 "CREATE TABLE c7 (id INT PRIMARY KEY, name VARCHAR(20), age INT)").unwrap();

        // 乱序 + 只写两列：name 没被点名 → NULL
        exec_sql(&mut db, &mut session, "INSERT INTO c7 (age, id) VALUES (30, 7)").unwrap();
        match exec_sql(&mut db, &mut session, "SELECT * FROM c7").unwrap() {
            ResultSet::Rows { rows, .. } => assert_eq!(rows[0], vec![
                Some("7".into()), None, Some("30".into()),
            ]),
            other => panic!("got {:?}", other),
        }

        // 列名不存在 / 列名重复 / 数量对不上 / 漏掉主键——全部在动数据之前拒掉
        let err = exec_sql(&mut db, &mut session, "INSERT INTO c7 (ghost) VALUES (1)").unwrap_err();
        assert!(err.contains("not found"), "got: {}", err);
        let err = exec_sql(&mut db, &mut session, "INSERT INTO c7 (id, id) VALUES (8, 9)").unwrap_err();
        assert!(err.contains("duplicate column"), "got: {}", err);
        let err = exec_sql(&mut db, &mut session, "INSERT INTO c7 (id, age) VALUES (8)").unwrap_err();
        assert!(err.contains("expected 2 values"), "got: {}", err);
        let err = exec_sql(&mut db, &mut session, "INSERT INTO c7 (name) VALUES ('solo')").unwrap_err();
        assert!(err.contains("NOT NULL"), "got: {}", err);

        // 上面四条都失败了，表里还是只有那一行
        match exec_sql(&mut db, &mut session, "SELECT id FROM c7").unwrap() {
            ResultSet::Rows { rows, .. } => assert_eq!(cells_all(&rows), vec![vec!["7".to_string()]]),
            other => panic!("got {:?}", other),
        }

        let _ = db.shutdown();
    }
    #[test]
    fn test_e2e_insert_multi_row() {
        let _guard = db_lock();
        let mut db = open_test_db();
        let mut session = Session::new();

        exec_sql(&mut db, &mut session,
                 "CREATE TABLE mr_t (id INT PRIMARY KEY, name VARCHAR(20))").unwrap();

        // 多组括号 = 多行
        assert!(matches!(
            exec_sql(&mut db, &mut session,
                     "INSERT INTO mr_t VALUES (1, 'A'), (2, 'B'), (3, 'C')").unwrap(),
            ResultSet::Affected(3)
        ));

        // 带列名列表，且列序与 schema 不同 → 每一行都按同一套 targets 归一
        assert!(matches!(
            exec_sql(&mut db, &mut session,
                     "INSERT INTO mr_t (name, id) VALUES ('D', 4), ('E', 5)").unwrap(),
            ResultSet::Affected(2)
        ));

        match exec_sql(&mut db, &mut session, "SELECT COUNT(*) FROM mr_t").unwrap() {
            ResultSet::Rows { rows, .. } => assert_eq!(cells_all(&rows), vec![vec!["5".to_string()]]),
            other => panic!("got {:?}", other),
        }

        // 第 2 行主键冲突 → 整条语句回滚，第 6 行不能留下
        assert_err_contains(
            exec_sql(&mut db, &mut session,
                     "INSERT INTO mr_t VALUES (6, 'F'), (1, 'dup')"),
            "Duplicate value",
        );
        match exec_sql(&mut db, &mut session, "SELECT COUNT(*) FROM mr_t").unwrap() {
            ResultSet::Rows { rows, .. } => {
                assert_eq!(cells_all(&rows), vec![vec!["5".to_string()]],
                           "失败的 INSERT 不该留下半截数据");
            }
            other => panic!("got {:?}", other),
        }

        let _ = db.shutdown();
    }
    /// DEFAULT：INSERT 没点名的列自动填默认值；显式给值（含显式 NULL）不覆盖默认值语义
    #[test]
    fn test_e2e_default_fill() {
        let _guard = db_lock();
        let mut db = open_test_db();
        let mut session = Session::new();

        exec_sql(&mut db, &mut session,
                 "CREATE TABLE dd (id INT PRIMARY KEY, qty INT DEFAULT 0, tag VARCHAR(10) DEFAULT 'none', note VARCHAR(10))").unwrap();

        // 只点名 id：qty 填 0、tag 填 'none'，note 无默认→ NULL
        exec_sql(&mut db, &mut session, "INSERT INTO dd (id) VALUES (1)").unwrap();
        match exec_sql(&mut db, &mut session, "SELECT * FROM dd WHERE id = 1").unwrap() {
            ResultSet::Rows { rows, .. } => assert_eq!(rows[0], vec![
                Some("1".into()), Some("0".into()), Some("none".into()), None,
            ]),
            other => panic!("got {:?}", other),
        }

        // 显式给值覆盖默认
        exec_sql(&mut db, &mut session, "INSERT INTO dd (id, qty) VALUES (2, 99)").unwrap();
        match exec_sql(&mut db, &mut session, "SELECT qty, tag FROM dd WHERE id = 2").unwrap() {
            ResultSet::Rows { rows, .. } => assert_eq!(rows[0], vec![Some("99".into()), Some("none".into())]),
            other => panic!("got {:?}", other),
        }

        // SQL 语义：显式写 NULL 就是 NULL，不会被默认值替（跟“没列这个字段”是两回事）
        exec_sql(&mut db, &mut session, "INSERT INTO dd (id, tag) VALUES (3, NULL)").unwrap();
        match exec_sql(&mut db, &mut session, "SELECT tag FROM dd WHERE id = 3").unwrap() {
            ResultSet::Rows { rows, .. } => assert_eq!(rows[0][0], None, "显式 NULL 不应被默认值替"),
            other => panic!("got {:?}", other),
        }

        let _ = db.shutdown();
    }

    /// UNIQUE：非主键的唯一列撞车报错；但 NULL 不参与唯一性判定（InnoDB 行为）
    #[test]
    fn test_e2e_unique_constraint() {
        let _guard = db_lock();
        let mut db = open_test_db();
        let mut session = Session::new();

        exec_sql(&mut db, &mut session,
                 "CREATE TABLE uq (id INT PRIMARY KEY, code VARCHAR(20) UNIQUE)").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO uq VALUES (1, 'A')").unwrap();

        // 重复的 code='A' → 拒
        let err = exec_sql(&mut db, &mut session, "INSERT INTO uq VALUES (2, 'A')").unwrap_err();
        assert!(err.contains("Duplicate"), "got: {}", err);

        // code 为 NULL 的行允许多次（SQL 标准：NULL 互不相等）
        exec_sql(&mut db, &mut session, "INSERT INTO uq (id) VALUES (3)").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO uq (id) VALUES (4)").unwrap();

        match exec_sql(&mut db, &mut session, "SELECT id FROM uq ORDER BY id").unwrap() {
            ResultSet::Rows { rows, .. } => assert_eq!(rows.len(), 3, "一行 A + 两行 NULL code"),
            other => panic!("got {:?}", other),
        }

        let _ = db.shutdown();
    }

    #[test]
    fn test_e2e_join_two_tables() {
        let _guard = db_lock();
        let mut db = open_test_db();
        let mut session = Session::new();

        exec_sql(&mut db, &mut session,
                 "CREATE TABLE j_users (id INT PRIMARY KEY, name VARCHAR(20))").unwrap();
        exec_sql(&mut db, &mut session,
                 "CREATE TABLE j_orders (id INT PRIMARY KEY, user_id INT, amount INT)").unwrap();

        exec_sql(&mut db, &mut session, "INSERT INTO j_users VALUES (1, 'Alice')").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO j_users VALUES (2, 'Bob')").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO j_orders VALUES (10, 1, 100)").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO j_orders VALUES (11, 1, 200)").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO j_orders VALUES (12, 2, 50)").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO j_orders VALUES (13, 3, 70)").unwrap();

        // ① SELECT *：5 列、3 行（Alice 两条订单，user_id=3 无匹配被丢弃）
        let r = exec_sql(&mut db, &mut session,
                         "SELECT * FROM j_users JOIN j_orders ON j_users.id = j_orders.user_id").unwrap();
        match r {
            ResultSet::Rows { columns, rows } => {
                assert_eq!(columns, vec![
                    "j_users.id", "j_users.name",
                    "j_orders.id", "j_orders.user_id", "j_orders.amount",
                ]);
                assert_eq!(rows.len(), 3);
                assert_eq!(cells(&rows[0]), vec!["1", "Alice", "10", "1", "100"]);
                assert_eq!(cells(&rows[1]), vec!["1", "Alice", "11", "1", "200"]);
                assert_eq!(cells(&rows[2]), vec!["2", "Bob", "12", "2", "50"]);
            }
            other => panic!("expected Rows, got {:?}", other),
        }

        // ② 限定列投影 + WHERE + ORDER BY（JOIN 后依然先筛行后删列）
        let r = exec_sql(&mut db, &mut session,
                         "SELECT j_users.name, j_orders.amount FROM j_users JOIN j_orders \
             ON j_users.id = j_orders.user_id WHERE j_orders.amount > 60 \
             ORDER BY j_orders.amount DESC").unwrap();
        match r {
            ResultSet::Rows { columns, rows } => {
                assert_eq!(columns, vec!["j_users.name", "j_orders.amount"]);
                assert_eq!(cells_all(&rows), vec![
                    vec!["Alice".to_string(), "200".to_string()],
                    vec!["Alice".to_string(), "100".to_string()],
                ]);
            }
            other => panic!("expected Rows, got {:?}", other),
        }

        // ③ 裸 id 在两表都存在 → 必须报歧义，不能静默取第一个
        let err = exec_sql(&mut db, &mut session,
                           "SELECT id FROM j_users JOIN j_orders ON j_users.id = j_orders.user_id").unwrap_err();
        assert!(err.contains("ambiguous"), "got: {}", err);

        // ④ 裸 amount 只在 j_orders 里 → 唯一，正常放行
        let r = exec_sql(&mut db, &mut session,
                         "SELECT amount FROM j_users JOIN j_orders ON j_users.id = j_orders.user_id").unwrap();
        match r {
            ResultSet::Rows { columns, .. } => assert_eq!(columns, vec!["j_orders.amount"]),
            other => panic!("expected Rows, got {:?}", other),
        }

        // ⑤ 单表写限定名：标准 SQL 允许，输出列名仍是裸名
        let r = exec_sql(&mut db, &mut session, "SELECT j_users.name FROM j_users").unwrap();
        match r {
            ResultSet::Rows { columns, rows } => {
                assert_eq!(columns, vec!["name"]);
                assert_eq!(rows.len(), 2);
            }
            other => panic!("expected Rows, got {:?}", other),
        }

        // ⑥ 错误前缀不能被短名兜住（否则 other.name 会误命中 name）
        let err = exec_sql(&mut db, &mut session, "SELECT nope.name FROM j_users").unwrap_err();
        assert!(err.contains("not found"), "got: {}", err);

        let _ = db.shutdown();
    }
    #[test]
    fn test_e2e_group_by_and_aggregates() {
        let _guard = db_lock();
        let mut db = open_test_db();
        let mut session = Session::new();

        exec_sql(&mut db, &mut session,
                 "CREATE TABLE g_sales (id INT PRIMARY KEY, dept VARCHAR(10), amount INT, note VARCHAR(10))").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO g_sales VALUES (1, 'A', 100, 'x')").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO g_sales VALUES (2, 'A', 200, NULL)").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO g_sales VALUES (3, 'B', 50, NULL)").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO g_sales VALUES (4, 'B', 70, 'y')").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO g_sales VALUES (5, NULL, 30, 'z')").unwrap();

        // ① 分组 + 五个函数；COUNT(note) 必须跳过 NULL
        let r = exec_sql(&mut db, &mut session,
                         "SELECT dept, COUNT(*), COUNT(note), SUM(amount), AVG(amount), MIN(amount), MAX(amount) \
             FROM g_sales GROUP BY dept ORDER BY dept").unwrap();
        match r {
            ResultSet::Rows { columns, rows } => {
                assert_eq!(columns, vec![
                    "dept".to_string(), "COUNT(*)".to_string(), "COUNT(note)".to_string(),
                    "SUM(amount)".to_string(), "AVG(amount)".to_string(),
                    "MIN(amount)".to_string(), "MAX(amount)".to_string(),
                ]);
                // ORDER BY dept ASC：NULL 组排最前（cmp_for_sort 的约定）
                assert_eq!(cells(&rows[0]), vec!["NULL", "1", "1", "30", "30", "30", "30"]);
                assert_eq!(cells(&rows[1]), vec!["A", "2", "1", "300", "150", "100", "200"]);
                assert_eq!(cells(&rows[2]), vec!["B", "2", "1", "120", "60", "50", "70"]);
            }
            other => panic!("expected Rows, got {:?}", other),
        }

        // ② 没有 GROUP BY：空输入也必须交出一行（COUNT=0，SUM=NULL）
        let r = exec_sql(&mut db, &mut session,
                         "SELECT COUNT(*), SUM(amount) FROM g_sales WHERE id > 999").unwrap();
        match r {
            ResultSet::Rows { rows, .. } => {
                assert_eq!(rows.len(), 1);
                assert_eq!(cells(&rows[0]), vec!["0", "NULL"]);
            }
            other => panic!("expected Rows, got {:?}", other),
        }

        // ③ 非分组列 → 必须报错，不能静默取第一行
        let err = exec_sql(&mut db, &mut session,
                           "SELECT id, COUNT(*) FROM g_sales GROUP BY dept").unwrap_err();
        assert!(err.contains("GROUP BY"), "got: {}", err);

        // ④ SELECT * 与 GROUP BY 混用 → 报错
        let err = exec_sql(&mut db, &mut session,
                           "SELECT * FROM g_sales GROUP BY dept").unwrap_err();
        assert!(err.contains("SELECT *"), "got: {}", err);

        // ⑤ 无聚合、只有 GROUP BY → 等价去重
        let r = exec_sql(&mut db, &mut session, "SELECT dept FROM g_sales GROUP BY dept").unwrap();
        match r {
            ResultSet::Rows { rows, .. } => assert_eq!(rows.len(), 3, "A/B/NULL 三组"),
            other => panic!("expected Rows, got {:?}", other),
        }

        let _ = db.shutdown();
    }
    #[test]
    fn test_e2e_limit() {
        let _guard = db_lock();
        let mut db = open_test_db();
        let mut session = Session::new();

        exec_sql(&mut db, &mut session, "CREATE TABLE l_num (id INT PRIMARY KEY)").unwrap();
        for i in 1..=5 {
            exec_sql(&mut db, &mut session, &format!("INSERT INTO l_num VALUES ({})", i)).unwrap();
        }

        // ① ORDER BY + LIMIT：截的必须是"排序后的前 2 行"
        let r = exec_sql(&mut db, &mut session,
                         "SELECT id FROM l_num ORDER BY id DESC LIMIT 2").unwrap();
        match r {
            ResultSet::Rows { rows, .. } => {
                assert_eq!(cells_all(&rows), vec![
                    vec!["5".to_string()], vec!["4".to_string()],
                ], "先截断再排序会得到 1,2 —— 这就是错位");
            }
            other => panic!("got {:?}", other),
        }

        // ② LIMIT 0：0 行，但列名要保留
        let r = exec_sql(&mut db, &mut session, "SELECT id FROM l_num LIMIT 0").unwrap();
        match r {
            ResultSet::Rows { columns, rows } => {
                assert_eq!(columns, vec!["id"]);
                assert_eq!(rows.len(), 0);
            }
            other => panic!("got {:?}", other),
        }

        // ③ 聚合分支也必须截断（只改普通路径的话，这条会挂）
        let r = exec_sql(&mut db, &mut session, "SELECT COUNT(*) FROM l_num LIMIT 0").unwrap();
        match r {
            ResultSet::Rows { rows, .. } => {
                assert_eq!(rows.len(), 0, "聚合路径的 LIMIT 0 没生效");
            }
            other => panic!("got {:?}", other),
        }

        let _ = db.shutdown();
    }
    #[test]
    fn test_e2e_having() {
        let _guard = db_lock();
        let mut db = open_test_db();
        let mut session = Session::new();

        exec_sql(&mut db, &mut session,
                 "CREATE TABLE h_sales (id INT PRIMARY KEY, dept VARCHAR(10), amount INT)").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO h_sales VALUES (1, 'A', 100)").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO h_sales VALUES (2, 'A', 200)").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO h_sales VALUES (3, 'B', 50)").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO h_sales VALUES (4, 'C', NULL)").unwrap();

        // ① 分组 + HAVING：A 组 300 留下；B 组 50 被筛掉；
        //    C 组 SUM 是 NULL → 比较得 Unknown → 也必须被筛掉（不是 false 也不是 true）
        let r = exec_sql(&mut db, &mut session,
                         "SELECT dept, SUM(amount) FROM h_sales GROUP BY dept HAVING SUM(amount) > 100").unwrap();
        match r {
            ResultSet::Rows { columns, rows } => {
                assert_eq!(columns, vec!["dept".to_string(), "SUM(amount)".to_string()]);
                assert_eq!(cells_all(&rows), vec![vec!["A".to_string(), "300".to_string()]]);
            }
            other => panic!("got {:?}", other),
        }

        // ② 我们的取舍：HAVING 引用没出现在 SELECT 里的聚合 → 明确报错
        let err = exec_sql(&mut db, &mut session,
                           "SELECT dept FROM h_sales GROUP BY dept HAVING COUNT(*) > 1").unwrap_err();
        assert!(err.contains("SELECT list"), "错误信息要说清楚限制，got: {}", err);

        // ③ 空输入：没有 HAVING 是 1 行 0；加了 HAVING 就把那一行筛掉了
        let r = exec_sql(&mut db, &mut session,
                         "SELECT COUNT(*) FROM h_sales WHERE id > 999").unwrap();
        match r {
            ResultSet::Rows { rows, .. } => assert_eq!(cells_all(&rows), vec![vec!["0".to_string()]]),
            other => panic!("got {:?}", other),
        }
        let r = exec_sql(&mut db, &mut session,
                         "SELECT COUNT(*) FROM h_sales WHERE id > 999 HAVING COUNT(*) > 0").unwrap();
        match r {
            ResultSet::Rows { rows, .. } => {
                assert_eq!(rows.len(), 0, "空组存在，但被 HAVING 筛掉了");
            }
            other => panic!("got {:?}", other),
        }

        // ④ 没组也没聚合 → 必须报错，不能当 WHERE 用
        let err = exec_sql(&mut db, &mut session,
                           "SELECT id FROM h_sales HAVING id > 1").unwrap_err();
        assert!(err.contains("HAVING"), "got: {}", err);

        // ⑤ 这一条是给"retain + =="写的陷阱检查：
        //    类型不匹配必须报错出来。如果 HAVING 用 retain/== 实现，
        //    这里会拿到 0 行而不是 Err，unwrap_err 就 panic 了。
        let err = exec_sql(&mut db, &mut session,
                           "SELECT dept, SUM(amount) FROM h_sales GROUP BY dept HAVING SUM(amount) > 'x'").unwrap_err();
        assert!(err.contains("cannot compare"), "错误不能被吞成 0 行，got: {}", err);

        let _ = db.shutdown();
    }
    #[test]
    fn test_e2e_order_by_aggregate() {
        let _guard = db_lock();
        let mut db = open_test_db();
        let mut session = Session::new();

        exec_sql(&mut db, &mut session,
                 "CREATE TABLE o_agg (id INT PRIMARY KEY, dept VARCHAR(10), amount INT)").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO o_agg VALUES (1, 'A', 100)").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO o_agg VALUES (2, 'A', 200)").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO o_agg VALUES (3, 'B', 500)").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO o_agg VALUES (4, 'B', 50)").unwrap();

        // ① ORDER BY SUM(amount) DESC：A=300、B=550 → B 在前
        let r = exec_sql(&mut db, &mut session,
                         "SELECT dept, SUM(amount) FROM o_agg GROUP BY dept ORDER BY SUM(amount) DESC").unwrap();
        match r {
            ResultSet::Rows { rows, .. } => assert_eq!(cells_all(&rows), vec![
                vec!["B".to_string(), "550".to_string()],
                vec!["A".to_string(), "300".to_string()],
            ]),
            other => panic!("got {:?}", other),
        }

        // ② 排序键换成聚合结果本身以外的东西：按分组列排
        let r = exec_sql(&mut db, &mut session,
                         "SELECT dept, SUM(amount) FROM o_agg GROUP BY dept ORDER BY dept DESC").unwrap();
        match r {
            ResultSet::Rows { rows, .. } => assert_eq!(cells_all(&rows), vec![
                vec!["B".to_string(), "550".to_string()],
                vec!["A".to_string(), "300".to_string()],
            ]),
            other => panic!("got {:?}", other),
        }

        // ③ 引用了不在 SELECT 里的聚合 → 明确报错（不是 "column not found"）
        let err = exec_sql(&mut db, &mut session,
                           "SELECT dept FROM o_agg GROUP BY dept ORDER BY SUM(amount)").unwrap_err();
        assert!(err.contains("ORDER BY"), "got: {}", err);

        // ④ 非聚合查询里用聚合排序 → 明确报错
        let err = exec_sql(&mut db, &mut session,
                           "SELECT id FROM o_agg ORDER BY SUM(amount)").unwrap_err();
        assert!(err.contains("aggregate"), "got: {}", err);

        let _ = db.shutdown();
    }
    #[test]
    fn test_e2e_distinct() {
        let _guard = db_lock();
        let mut db = open_test_db();
        let mut session = Session::new();

        exec_sql(&mut db, &mut session,
                 "CREATE TABLE dq (id INT PRIMARY KEY, dept VARCHAR(10), amount INT)").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO dq VALUES (1, 'A', 100)").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO dq VALUES (2, 'A', 100)").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO dq VALUES (3, 'A', 200)").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO dq VALUES (4, 'B', 100)").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO dq VALUES (5, NULL, 100)").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO dq VALUES (6, NULL, 100)").unwrap();

        // ① 单列去重：A / B / NULL 各一行；NULL 排最前（cmp_for_sort 的规则）
        let r = exec_sql(&mut db, &mut session,
                         "SELECT DISTINCT dept FROM dq ORDER BY dept").unwrap();
        match r {
            ResultSet::Rows { rows, .. } => assert_eq!(cells_all(&rows), vec![
                vec!["NULL".to_string()],
                vec!["A".to_string()],
                vec!["B".to_string()],
            ]),
            other => panic!("got {:?}", other),
        }

        // ② 多列去重：按 (dept, amount) 组合，(A,100) 那两行只留一行
        let r = exec_sql(&mut db, &mut session,
                         "SELECT DISTINCT dept, amount FROM dq ORDER BY dept, amount").unwrap();
        match r {
            ResultSet::Rows { rows, .. } => assert_eq!(cells_all(&rows), vec![
                vec!["NULL".to_string(), "100".to_string()],
                vec!["A".to_string(), "100".to_string()],
                vec!["A".to_string(), "200".to_string()],
                vec!["B".to_string(), "100".to_string()],
            ]),
            other => panic!("got {:?}", other),
        }

        // ③ 顺序陷阱：DISTINCT 必须在 LIMIT 之前。
        //    插入顺序是 A,A,A,B,NULL,NULL。
        //    先 LIMIT 2 拿到 A,A → 去重 → 只剩 1 行（错）；
        //    先去重拿到 A,B,NULL → LIMIT 2 → A,B（对）。
        let r = exec_sql(&mut db, &mut session,
                         "SELECT DISTINCT dept FROM dq LIMIT 2").unwrap();
        match r {
            ResultSet::Rows { rows, .. } => assert_eq!(cells_all(&rows), vec![
                vec!["A".to_string()],
                vec!["B".to_string()],
            ]),
            other => panic!("got {:?}", other),
        }

        // ④ SELECT DISTINCT *：按整行去重，主键各不相同 → 6 行全留
        let r = exec_sql(&mut db, &mut session, "SELECT DISTINCT * FROM dq").unwrap();
        match r {
            ResultSet::Rows { rows, .. } => assert_eq!(rows.len(), 6),
            other => panic!("got {:?}", other),
        }

        // ⑤ DISTINCT + ORDER BY 非输出列 → 标准禁止，因为排序键在结果里不存在
        let err = exec_sql(&mut db, &mut session,
                           "SELECT DISTINCT dept FROM dq ORDER BY amount").unwrap_err();
        assert!(err.contains("select list"), "got: {}", err);

        let _ = db.shutdown();
    }

    #[test]
    fn test_e2e_like_underscore() {
        let _guard = db_lock();
        let mut db = open_test_db();
        let mut session = Session::new();

        exec_sql(&mut db, &mut session,
                 "CREATE TABLE lk (id INT PRIMARY KEY, name VARCHAR(20))").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO lk VALUES (1, 'Alice')").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO lk VALUES (2, 'AAlice')").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO lk VALUES (3, 'Alxce')").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO lk VALUES (4, 'Al')").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO lk VALUES (5, 'Bob')").unwrap();

        // ① `_` 恰好一个字符：'A_ice' 只匹配 5 个字符的 Alice，
        //    6 个字符的 AAlice 不能匹配（长度不对）
        let r = exec_sql(&mut db, &mut session,
                         "SELECT name FROM lk WHERE name LIKE 'A_ice'").unwrap();
        match r {
            ResultSet::Rows { rows, .. } => assert_eq!(cells_all(&rows), vec![
                vec!["Alice".to_string()],
            ]),
            other => panic!("got {:?}", other),
        }

        // ② `_` 单独用：'_ob' = 任意 1 个字符 + "ob"
        let r = exec_sql(&mut db, &mut session,
                         "SELECT name FROM lk WHERE name LIKE '_ob'").unwrap();
        match r {
            ResultSet::Rows { rows, .. } => assert_eq!(cells_all(&rows), vec![
                vec!["Bob".to_string()],
            ]),
            other => panic!("got {:?}", other),
        }

        // ③ `%` 的回溯没被破坏：'Al%ce' 的前缀要吃掉不同长度的中间段
        let r = exec_sql(&mut db, &mut session,
                         "SELECT name FROM lk WHERE name LIKE 'Al%ce'").unwrap();
        match r {
            ResultSet::Rows { rows, .. } => assert_eq!(cells_all(&rows), vec![
                vec!["Alice".to_string()],
                vec!["Alxce".to_string()],
            ]),
            other => panic!("got {:?}", other),
        }

        // ④ 长度不匹配：'Al__' 要求恰好 4 个字符，表里没有 → 0 行
        let r = exec_sql(&mut db, &mut session,
                         "SELECT name FROM lk WHERE name LIKE 'Al__'").unwrap();
        match r {
            ResultSet::Rows { rows, .. } => assert_eq!(rows.len(), 0),
            other => panic!("got {:?}", other),
        }

        let _ = db.shutdown();
    }
    /// 断言某条 SQL 必须失败，且错误信息包含指定片段。
    /// 直接比较整串太脆（改一个字就红），只比关键片段。
    fn assert_err_contains(res: Result<ResultSet, String>, needle: &str) {
        match res {
            Ok(other) => panic!("expected error containing '{}', got {:?}", needle, other),
            Err(e) => assert!(e.contains(needle), "expected '{}', got: {}", needle, e),
        }
    }

    #[test]
    fn test_e2e_access_control() {
        let _guard = db_lock();
        let mut db = open_test_db();
        let mut session = Session::new();

        // ① 零用户：bootstrap 不设防
        exec_sql(&mut db, &mut session, "CREATE TABLE acl_t (id INT)").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO acl_t VALUES (1)").unwrap();

        // ② 第一个用户自动成为 admin
        exec_sql(&mut db, &mut session, "CREATE USER alice PASSWORD 'pw123'").unwrap();

        // ③ 建用户 ≠ 登录：同一条连接仍然是匿名
        assert_err_contains(
            exec_sql(&mut db, &mut session, "SELECT * FROM acl_t"),
            "please LOGIN first",
        );

        // ④ 登录后 admin 全权
        exec_sql(&mut db, &mut session, "LOGIN alice PASSWORD 'pw123'").unwrap();
        assert!(matches!(
            exec_sql(&mut db, &mut session, "SELECT * FROM acl_t").unwrap(),
            ResultSet::Rows { .. }
        ));

        // ⑤ 第二个用户是普通用户
        exec_sql(&mut db, &mut session, "CREATE USER bob PASSWORD 'pw456'").unwrap();
        exec_sql(&mut db, &mut session, "LOGIN bob PASSWORD 'pw456'").unwrap();

        // ⑥ 没授权 → 四种 DML 全拒
        assert_err_contains(exec_sql(&mut db, &mut session, "SELECT * FROM acl_t"),
                            "permission denied: SELECT");
        assert_err_contains(exec_sql(&mut db, &mut session, "INSERT INTO acl_t VALUES (2)"),
                            "permission denied: INSERT");
        assert_err_contains(exec_sql(&mut db, &mut session, "UPDATE acl_t SET id = 9 WHERE id = 1"),
                            "permission denied: UPDATE");
        assert_err_contains(exec_sql(&mut db, &mut session, "DELETE FROM acl_t WHERE id = 1"),
                            "permission denied: DELETE");

        // ⑦ DDL 只有 admin 能做（报的是 admin required，不是表级权限）
        assert_err_contains(exec_sql(&mut db, &mut session, "CREATE TABLE acl_x (id INT)"),
                            "admin required");

        // ⑧ 内部表：普通用户连读都不行
        assert_err_contains(exec_sql(&mut db, &mut session, "SELECT * FROM __minidb_users"),
                            "cannot access internal table");

        // ⑨ admin 授权；重复 GRANT 幂等（第二次 0 行）
        exec_sql(&mut db, &mut session, "LOGIN alice PASSWORD 'pw123'").unwrap();
        assert!(matches!(
            exec_sql(&mut db, &mut session, "GRANT SELECT, INSERT ON acl_t TO bob").unwrap(),
            ResultSet::Affected(2)
        ));
        assert!(matches!(
            exec_sql(&mut db, &mut session, "GRANT SELECT, INSERT ON acl_t TO bob").unwrap(),
            ResultSet::Affected(0)
        ));

        // ⑩ 授权立刻生效；没授的权限仍然拒
        exec_sql(&mut db, &mut session, "LOGIN bob PASSWORD 'pw456'").unwrap();
        assert!(matches!(
            exec_sql(&mut db, &mut session, "SELECT * FROM acl_t").unwrap(),
            ResultSet::Rows { .. }
        ));
        assert!(matches!(
            exec_sql(&mut db, &mut session, "INSERT INTO acl_t VALUES (2)").unwrap(),
            ResultSet::Affected(1)
        ));
        assert_err_contains(exec_sql(&mut db, &mut session, "DELETE FROM acl_t WHERE id = 2"),
                            "permission denied: DELETE");

        // ⑪ 撤权立刻生效
        exec_sql(&mut db, &mut session, "LOGIN alice PASSWORD 'pw123'").unwrap();
        assert!(matches!(
            exec_sql(&mut db, &mut session, "REVOKE SELECT ON acl_t FROM bob").unwrap(),
            ResultSet::Affected(1)
        ));
        exec_sql(&mut db, &mut session, "LOGIN bob PASSWORD 'pw456'").unwrap();
        assert_err_contains(exec_sql(&mut db, &mut session, "SELECT * FROM acl_t"),
                            "permission denied: SELECT");

        // ⑫ admin 也不许直接写内部表（可读不可写）
        exec_sql(&mut db, &mut session, "LOGIN alice PASSWORD 'pw123'").unwrap();
        assert_err_contains(
            exec_sql(&mut db, &mut session, "DELETE FROM __minidb_users WHERE username = 'bob'"),
            "cannot access internal table",
        );

        // ⑬ 口令错误：用户存在与不存在返回同一句话（防用户名枚举）
        let mut anon = Session::new();
        assert_err_contains(exec_sql(&mut db, &mut anon, "LOGIN alice PASSWORD 'wrong'"),
                            "invalid username or password");

        // ⑭ 事务中禁止切换身份
        exec_sql(&mut db, &mut session, "BEGIN").unwrap();
        assert_err_contains(exec_sql(&mut db, &mut session, "LOGIN bob PASSWORD 'pw456'"),
                            "not allowed inside a transaction");
        exec_sql(&mut db, &mut session, "ROLLBACK").unwrap();

        // 清理
        let _ = exec_sql(&mut db, &mut session, "DROP TABLE acl_t");
        let _ = db.shutdown();
    }

    #[test]
    fn test_e2e_access_control_persists_after_reopen() {
        let _guard = db_lock();
        let mut db = open_test_db();
        let mut session = Session::new();

        exec_sql(&mut db, &mut session, "CREATE TABLE acp_t (id INT)").unwrap();
        exec_sql(&mut db, &mut session, "CREATE USER root2 PASSWORD 'rootpw'").unwrap();
        db.shutdown().unwrap();
        drop(db);

        // 重开同一个库：账号、口令摘要都必须从内部表恢复
        let mut db = Database::open_at(TEST_DB, TEST_LOG).expect("reopen");
        let mut session = Session::new();

        // 重启后 bootstrap 不会重来：匿名仍然被拒
        assert_err_contains(exec_sql(&mut db, &mut session, "SELECT * FROM acp_t"),
                            "please LOGIN first");

        exec_sql(&mut db, &mut session, "LOGIN root2 PASSWORD 'rootpw'").unwrap();
        assert!(matches!(
            exec_sql(&mut db, &mut session, "SELECT * FROM acp_t").unwrap(),
            ResultSet::Rows { .. }
        ));

        let _ = exec_sql(&mut db, &mut session, "DROP TABLE acp_t");
        let _ = db.shutdown();
    }
    #[test]
    fn test_e2e_bootstrap_requires_localhost() {
        let _guard = db_lock();
        let mut db = open_test_db();

        // 模拟一个来自非 loopback 的连接（Session::new() 默认 true，这里手动覆盖）
        let mut remote = Session::new();
        remote.peer_is_local = false;

        // 零用户期：远端连 CREATE TABLE 都不行，不只是抢不到 admin
        assert_err_contains(exec_sql(&mut db, &mut remote, "CREATE TABLE r_t (id INT)"),
                            "bootstrap must come from localhost");
        assert_err_contains(exec_sql(&mut db, &mut remote, "CREATE USER eve PASSWORD 'x'"),
                            "bootstrap must come from localhost");

        // 本地连接仍然可以 bootstrap
        let mut local = Session::new();
        exec_sql(&mut db, &mut local, "CREATE USER root3 PASSWORD 'rootpw'").unwrap();

        // 有用户之后，远端看到的是常规提示：不再暴露“库是空的”这件事
        assert_err_contains(exec_sql(&mut db, &mut remote, "CREATE TABLE r_t (id INT)"),
                            "please LOGIN first");

        let _ = db.shutdown();
    }
    // ==================== 视图端到端 ====================

    #[test]
    fn test_e2e_view_basic() {
        let _guard = db_lock();
        let mut db = open_test_db();
        let mut session = Session::new();

        exec_sql(&mut db, &mut session,
                 "CREATE TABLE v_t (id INT PRIMARY KEY, name VARCHAR(20), age INT)").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO v_t VALUES (1, '张三', 20)").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO v_t VALUES (2, '李四', 15)").unwrap();
        exec_sql(&mut db, &mut session,
                 "CREATE VIEW adults AS SELECT id, name FROM v_t WHERE age >= 18").unwrap();

        // ① * 展开成视图的列，不是基表的全部列
        match exec_sql(&mut db, &mut session, "SELECT * FROM adults").unwrap() {
            ResultSet::Rows { columns, rows } => {
                assert_eq!(columns, vec!["id".to_string(), "name".to_string()]);
                assert_eq!(cells_all(&rows),
                           vec![vec!["1".to_string(), "张三".to_string()]]);
            }
            other => panic!("expected Rows, got {:?}", other),
        }

        // ② 定义里的 WHERE 必须保留，否则视图退化成「基表的别名」
        match exec_sql(&mut db, &mut session, "SELECT * FROM adults WHERE id = 2").unwrap() {
            ResultSet::Rows { rows, .. } => assert_eq!(rows.len(), 0),
            other => panic!("expected Rows, got {:?}", other),
        }

        // ③ 视图名可以当限定符用：adults.id → id
        match exec_sql(&mut db, &mut session, "SELECT adults.id FROM adults").unwrap() {
            ResultSet::Rows { rows, .. } => assert_eq!(rows.len(), 1),
            other => panic!("expected Rows, got {:?}", other),
        }

        // ④ 视图外的列不许透视
        assert_err_contains(exec_sql(&mut db, &mut session, "SELECT age FROM adults"),
                            "not found in view");

        // ⑤ 基表不受影响
        match exec_sql(&mut db, &mut session, "SELECT * FROM v_t").unwrap() {
            ResultSet::Rows { rows, .. } => assert_eq!(rows.len(), 2),
            other => panic!("expected Rows, got {:?}", other),
        }

        let _ = db.shutdown();
    }

    #[test]
    fn test_e2e_view_rejects_invalid_definitions() {
        let _guard = db_lock();
        let mut db = open_test_db();
        let mut session = Session::new();

        exec_sql(&mut db, &mut session, "CREATE TABLE vr_t (id INT PRIMARY KEY, name VARCHAR(20))").unwrap();
        exec_sql(&mut db, &mut session, "CREATE VIEW vr_v AS SELECT id FROM vr_t").unwrap();

        // 合并展开只对「单表 + 投影 + WHERE」语义等价，其余入口拒绝
        assert_err_contains(exec_sql(&mut db, &mut session, "CREATE VIEW vr_a AS SELECT * FROM vr_t LIMIT 1"),
                            "does not support LIMIT");
        assert_err_contains(exec_sql(&mut db, &mut session, "CREATE VIEW vr_b AS SELECT * FROM vr_t GROUP BY id"),
                            "GROUP BY");
        assert_err_contains(exec_sql(&mut db, &mut session, "CREATE VIEW vr_c AS SELECT * FROM vr_t ORDER BY id"),
                            "does not support ORDER BY");
        assert_err_contains(exec_sql(&mut db, &mut session, "CREATE VIEW vr_d AS SELECT DISTINCT id FROM vr_t"),
                            "does not support DISTINCT");

        // 禁止视图套视图
        assert_err_contains(exec_sql(&mut db, &mut session, "CREATE VIEW vr_e AS SELECT * FROM vr_v"),
                            "based on another view");
        // 基表必须存在、列必须存在
        assert_err_contains(exec_sql(&mut db, &mut session, "CREATE VIEW vr_f AS SELECT * FROM nosuch_t"),
                            "does not exist");
        assert_err_contains(exec_sql(&mut db, &mut session, "CREATE VIEW vr_g AS SELECT ghost FROM vr_t"),
                            "not found");
        // 重名：既不能撞表，也不能撞视图
        assert_err_contains(exec_sql(&mut db, &mut session, "CREATE VIEW vr_t AS SELECT id FROM vr_t"),
                            "already exists");
        assert_err_contains(exec_sql(&mut db, &mut session, "CREATE VIEW vr_v AS SELECT id FROM vr_t"),
                            "already exists");

        let _ = db.shutdown();
    }

    #[test]
    fn test_e2e_view_acl_uses_base_table() {
        let _guard = db_lock();
        let mut db = open_test_db();
        let mut session = Session::new();

        exec_sql(&mut db, &mut session,
                 "CREATE TABLE aclv_t (id INT PRIMARY KEY, secret VARCHAR(20))").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO aclv_t VALUES (1, 's3cret')").unwrap();
        exec_sql(&mut db, &mut session, "CREATE VIEW aclv_v AS SELECT * FROM aclv_t").unwrap();

        exec_sql(&mut db, &mut session, "CREATE USER alice PASSWORD 'pw123'").unwrap();
        exec_sql(&mut db, &mut session, "LOGIN alice PASSWORD 'pw123'").unwrap();
        exec_sql(&mut db, &mut session, "CREATE USER bob PASSWORD 'pw456'").unwrap();
        exec_sql(&mut db, &mut session, "LOGIN bob PASSWORD 'pw456'").unwrap();

        // ① bob 没有基表权限 → 视图也查不了（视图不是越权通道）
        assert_err_contains(exec_sql(&mut db, &mut session, "SELECT * FROM aclv_v"),
                            "permission denied: SELECT");

        // ② 视图只读
        assert_err_contains(exec_sql(&mut db, &mut session, "INSERT INTO aclv_v VALUES (2, 'x')"),
                            "read-only");

        // ③ 授权的对象是基表：授了基表，视图自然可查
        exec_sql(&mut db, &mut session, "LOGIN alice PASSWORD 'pw123'").unwrap();
        exec_sql(&mut db, &mut session, "GRANT SELECT ON aclv_t TO bob").unwrap();
        exec_sql(&mut db, &mut session, "LOGIN bob PASSWORD 'pw456'").unwrap();
        match exec_sql(&mut db, &mut session, "SELECT * FROM aclv_v").unwrap() {
            ResultSet::Rows { rows, .. } => assert_eq!(rows.len(), 1),
            other => panic!("expected Rows, got {:?}", other),
        }

        // ④ 撤权立刻生效，视图跟着失效
        exec_sql(&mut db, &mut session, "LOGIN alice PASSWORD 'pw123'").unwrap();
        exec_sql(&mut db, &mut session, "REVOKE SELECT ON aclv_t FROM bob").unwrap();
        exec_sql(&mut db, &mut session, "LOGIN bob PASSWORD 'pw456'").unwrap();
        assert_err_contains(exec_sql(&mut db, &mut session, "SELECT * FROM aclv_v"),
                            "permission denied: SELECT");

        // ⑤ 视图没有独立权限：GRANT ON view 不成立
        exec_sql(&mut db, &mut session, "LOGIN alice PASSWORD 'pw123'").unwrap();
        assert_err_contains(exec_sql(&mut db, &mut session, "GRANT SELECT ON aclv_v TO bob"),
                            "does not exist");

        let _ = db.shutdown();
    }

    #[test]
    fn test_e2e_view_persists_after_reopen() {
        let _guard = db_lock();
        let mut db = open_test_db();
        let mut session = Session::new();

        exec_sql(&mut db, &mut session, "CREATE TABLE vp_t (id INT PRIMARY KEY, name VARCHAR(20))").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO vp_t VALUES (1, 'Alice')").unwrap();
        exec_sql(&mut db, &mut session, "CREATE VIEW vp_v AS SELECT name FROM vp_t").unwrap();
        db.shutdown().unwrap();
        drop(db);

        // 重开：定义从 __minidb_views 里恢复，重新解析后照样能查
        let mut db = Database::open_at(TEST_DB, TEST_LOG).expect("reopen");
        let mut session = Session::new();
        match exec_sql(&mut db, &mut session, "SELECT * FROM vp_v").unwrap() {
            ResultSet::Rows { columns, rows } => {
                assert_eq!(columns, vec!["name".to_string()]);
                assert_eq!(cells_all(&rows), vec![vec!["Alice".to_string()]]);
            }
            other => panic!("expected Rows, got {:?}", other),
        }

        let _ = db.shutdown();
    }
    #[test]
    fn test_e2e_view_invalid_after_base_table_dropped() {
        let _guard = db_lock();
        let mut db = open_test_db();
        let mut session = Session::new();

        exec_sql(&mut db, &mut session, "CREATE TABLE vd_t (id INT PRIMARY KEY)").unwrap();
        exec_sql(&mut db, &mut session, "CREATE VIEW vd_v AS SELECT id FROM vd_t").unwrap();
        exec_sql(&mut db, &mut session, "DROP TABLE vd_t").unwrap();

        // 延迟失效：视图定义还在，但查的时候明确告诉你基表没了
        assert_err_contains(exec_sql(&mut db, &mut session, "SELECT * FROM vd_v"),
                            "is invalid");

        let _ = db.shutdown();
    }
    #[test]
    fn test_e2e_update_in_place() {
        let _guard = db_lock();
        let mut db = open_test_db();
        let mut session = Session::new();

        exec_sql(&mut db, &mut session,
                 "CREATE TABLE up_t (id INT PRIMARY KEY, name VARCHAR(20), age INT)").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO up_t VALUES (1, 'Alice', 20)").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO up_t VALUES (2, 'Bob', 30)").unwrap();

        // 定长字段改值 → 长度不变 → 原地覆盖，行的物理位置必须不动
        let before_ids: Vec<RowId> = db.open_table("up_t").unwrap()
            .scan_all().unwrap().iter().map(|(id, _)| *id).collect();
        assert!(matches!(
            exec_sql(&mut db, &mut session, "UPDATE up_t SET age = 21 WHERE id = 1").unwrap(),
            ResultSet::Affected(1)
        ));
        let after_ids: Vec<RowId> = db.open_table("up_t").unwrap()
            .scan_all().unwrap().iter().map(|(id, _)| *id).collect();
        assert_eq!(before_ids, after_ids, "原地修改不该改变行的物理位置");

        match exec_sql(&mut db, &mut session, "SELECT age FROM up_t WHERE id = 1").unwrap() {
            ResultSet::Rows { rows, .. } => assert_eq!(cells_all(&rows), vec![vec!["21".to_string()]]),
            other => panic!("expected Rows, got {:?}", other),
        }

        // 原地修改不产生墓碑：直接数页里的活记录。
        // 不能用 record_count()——catalog 的统计字段只在新页分配时写回，
        // 单页表的 meta 永远是 0（既有缺陷，不是本次改动引入）
        assert_eq!(db.open_table("up_t").unwrap().scan_all().unwrap().len(), 2);

        let _ = db.shutdown();
    }

    #[test]
    fn test_e2e_update_grow_falls_back() {
        let _guard = db_lock();
        let mut db = open_test_db();
        let mut session = Session::new();

        exec_sql(&mut db, &mut session,
                 "CREATE TABLE ug_t (id INT PRIMARY KEY, name VARCHAR(20))").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO ug_t VALUES (1, 'Bob')").unwrap();

        // 'Bob'(3) → 'Bobby'(5)：变长放不下 → 退化成删+插，结果必须仍正确
        assert!(matches!(
            exec_sql(&mut db, &mut session, "UPDATE ug_t SET name = 'Bobby' WHERE id = 1").unwrap(),
            ResultSet::Affected(1)
        ));
        // 再改短 → 原地覆盖
        assert!(matches!(
            exec_sql(&mut db, &mut session, "UPDATE ug_t SET name = 'Bo' WHERE id = 1").unwrap(),
            ResultSet::Affected(1)
        ));

        match exec_sql(&mut db, &mut session, "SELECT * FROM ug_t").unwrap() {
            ResultSet::Rows { columns, rows } => {
                assert_eq!(columns, vec!["id".to_string(), "name".to_string()]);
                assert_eq!(cells_all(&rows), vec![vec!["1".to_string(), "Bo".to_string()]]);
            }
            other => panic!("expected Rows, got {:?}", other),
        }

        let _ = db.shutdown();
    }

    #[test]
    fn test_e2e_update_rollback() {
        let _guard = db_lock();
        let mut db = open_test_db();
        let mut session = Session::new();

        exec_sql(&mut db, &mut session,
                 "CREATE TABLE ur_t (id INT PRIMARY KEY, age INT)").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO ur_t VALUES (1, 20)").unwrap();

        // 原地修改写的是物理 Update 日志，Undo 按 before image 写回
        exec_sql(&mut db, &mut session, "BEGIN").unwrap();
        exec_sql(&mut db, &mut session, "UPDATE ur_t SET age = 99 WHERE id = 1").unwrap();
        match exec_sql(&mut db, &mut session, "SELECT age FROM ur_t").unwrap() {
            ResultSet::Rows { rows, .. } => assert_eq!(cells_all(&rows), vec![vec!["99".to_string()]]),
            other => panic!("got {:?}", other),
        }
        exec_sql(&mut db, &mut session, "ROLLBACK").unwrap();

        match exec_sql(&mut db, &mut session, "SELECT age FROM ur_t").unwrap() {
            ResultSet::Rows { rows, .. } => {
                assert_eq!(cells_all(&rows), vec![vec!["20".to_string()]], "ROLLBACK 必须还原");
            }
            other => panic!("got {:?}", other),
        }

        let _ = db.shutdown();
    }

    // ==================== 多库端到端 ====================
    // ==================== 多库端到端 ====================

    const MULTI_DIR: &str = "./data/test_multi";
    const MULTI_DB: &str = "./data/test_multi/main.db";
    const MULTI_LOG: &str = "./data/test_multi/main.log";
    const SHOP_DB: &str = "./data/test_multi/shop.db";
    const SHOP_LOG: &str = "./data/test_multi/shop.log";

    /// 显式传入账号库的版本：模拟 server 层 DbRegistry::resolve 解出的 (当前库, 账号库)。
    /// 两个库必须是不同对象——同一个对象无法同时给出两个 &mut。
    fn exec_sql_with(
        db: &mut Database,
        accounts: &mut Database,
        session: &mut Session,
        sql: &str,
    ) -> Result<ResultSet, String> {
        let tokens = Lexer::new(sql).tokenize()?;
        let stmt = Parser::new(tokens).parse()?;
        execute(stmt, db, Some(accounts), session)
    }

    #[test]
    fn test_e2e_multi_database() {
        let _guard = db_lock();
        // 整个目录一起清：只删 main.db 的话，残留的 shop.db 会让「建库成功」变成假绿
        let _ = std::fs::remove_dir_all(MULTI_DIR);

        let mut main_db = Database::open_at(MULTI_DB, MULTI_LOG).expect("open main");
        let mut session = Session::new();

        // ① 默认库同时是账号库：bootstrap + admin
        exec_sql(&mut main_db, &mut session, "CREATE USER alice PASSWORD 'pw123'").unwrap();
        exec_sql(&mut main_db, &mut session, "LOGIN alice PASSWORD 'pw123'").unwrap();

        // ② 库名规则：内部前缀保留
        assert_err_contains(
            exec_sql(&mut main_db, &mut session, "CREATE DATABASE __minidb_x"),
            "reserved",
        );

        // ③ 建库 = 建两个文件；重复建库被拒
        exec_sql(&mut main_db, &mut session, "CREATE DATABASE shop").unwrap();
        assert!(std::path::Path::new(SHOP_DB).exists(), "shop.db 应该被创建");
        assert!(std::path::Path::new(SHOP_LOG).exists(), "shop.log 应该被创建");
        assert_err_contains(
            exec_sql(&mut main_db, &mut session, "CREATE DATABASE shop"),
            "already exists",
        );

        // ④ USE 只认已存在的库，而且不会顺手把库建出来
        exec_sql(&mut main_db, &mut session, "USE shop").unwrap();
        assert_err_contains(
            exec_sql(&mut main_db, &mut session, "USE nosuch"),
            "does not exist",
        );
        assert!(!std::path::Path::new("./data/test_multi/nosuch.db").exists());
        session.current_db = None;   // execute() 不消费它，清掉避免干扰后文

        // ⑤ 事务中禁止切库（事务号在各库的 txn_mgr 里不通用）
        exec_sql(&mut main_db, &mut session, "BEGIN").unwrap();
        assert_err_contains(
            exec_sql(&mut main_db, &mut session, "USE shop"),
            "not allowed inside a transaction",
        );
        exec_sql(&mut main_db, &mut session, "ROLLBACK").unwrap();

        // ===== 以下模拟 server 层：当前库 = shop，账号库 = main =====
        let mut shop = Database::open_at(SHOP_DB, SHOP_LOG).expect("open shop");
        let mut shop_session = Session::new();
        shop_session.current_db = Some("shop".to_string());

        // ⑥ 账号表只有一份（在主库）：shop 里匿名照样被拒
        assert_err_contains(
            exec_sql_with(&mut shop, &mut main_db, &mut shop_session, "SELECT * FROM t"),
            "please LOGIN first",
        );

        // ⑦ 主库的 admin 登录后能在 shop 建表 —— 身份是实例级的
        exec_sql_with(&mut shop, &mut main_db, &mut shop_session, "LOGIN alice PASSWORD 'pw123'").unwrap();
        exec_sql_with(&mut shop, &mut main_db, &mut shop_session, "CREATE TABLE t (id INT)").unwrap();
        exec_sql_with(&mut shop, &mut main_db, &mut shop_session, "INSERT INTO t VALUES (7)").unwrap();

        // ⑧ 库间隔离：主库里没有 t
        assert_err_contains(
            exec_sql(&mut main_db, &mut session, "SELECT * FROM t"),
            "not found",
        );

        // ⑨ 在 shop 里建的用户，落在主库的 __minidb_users
        exec_sql_with(&mut shop, &mut main_db, &mut shop_session, "CREATE USER bob PASSWORD 'pw456'").unwrap();
        match exec_sql(&mut main_db, &mut session, "SELECT username FROM __minidb_users").unwrap() {
            ResultSet::Rows { rows, .. } => assert_eq!(rows.len(), 2, "alice + bob"),
            other => panic!("got {:?}", other),
        }

        // ⑩ 普通用户在 shop 里没授权 → 拒
        exec_sql_with(&mut shop, &mut main_db, &mut shop_session, "LOGIN bob PASSWORD 'pw456'").unwrap();
        assert_err_contains(
            exec_sql_with(&mut shop, &mut main_db, &mut shop_session, "SELECT * FROM t"),
            "permission denied: SELECT",
        );

        // ⑪ 在 shop 里授权 → 只对 shop 生效
        exec_sql_with(&mut shop, &mut main_db, &mut shop_session, "LOGIN alice PASSWORD 'pw123'").unwrap();
        assert!(matches!(
            exec_sql_with(&mut shop, &mut main_db, &mut shop_session, "GRANT SELECT ON t TO bob").unwrap(),
            ResultSet::Affected(1)
        ));
        exec_sql_with(&mut shop, &mut main_db, &mut shop_session, "LOGIN bob PASSWORD 'pw456'").unwrap();
        assert!(matches!(
            exec_sql_with(&mut shop, &mut main_db, &mut shop_session, "SELECT * FROM t").unwrap(),
            ResultSet::Rows { .. }
        ));

        // ⑫ 权限不跨库：主库有同名表 t，但 bob 的授权记录只存在于 shop 的 __minidb_privs
        exec_sql(&mut main_db, &mut session, "CREATE TABLE t (id INT)").unwrap();
        exec_sql(&mut main_db, &mut session, "INSERT INTO t VALUES (9)").unwrap();
        let mut main_bob = Session::new();
        exec_sql(&mut main_db, &mut main_bob, "LOGIN bob PASSWORD 'pw456'").unwrap();
        assert_err_contains(
            exec_sql(&mut main_db, &mut main_bob, "SELECT * FROM t"),
            "permission denied: SELECT",
        );

        // ⑬ 非 admin 不能 DDL，也不能建库
        assert_err_contains(
            exec_sql_with(&mut shop, &mut main_db, &mut shop_session, "CREATE TABLE u2 (id INT)"),
            "admin required",
        );
        assert_err_contains(
            exec_sql_with(&mut shop, &mut main_db, &mut shop_session, "CREATE DATABASE other"),
            "admin required",
        );
        assert!(!std::path::Path::new("./data/test_multi/other.db").exists());

        // ⑭ 关库重开：shop 自己的数据、自己的 privs 都还在
        shop.shutdown().unwrap();
        drop(shop);

        let mut shop2 = Database::open_at(SHOP_DB, SHOP_LOG).expect("reopen shop");
        let mut s2 = Session::new();
        exec_sql_with(&mut shop2, &mut main_db, &mut s2, "LOGIN alice PASSWORD 'pw123'").unwrap();
        match exec_sql_with(&mut shop2, &mut main_db, &mut s2, "SELECT * FROM t").unwrap() {
            ResultSet::Rows { rows, .. } => {
                assert_eq!(rows.len(), 1);
                assert_eq!(cell(&rows[0][0]), "7");
            }
            other => panic!("got {:?}", other),
        }

        let _ = shop2.shutdown();
        drop(shop2);
        let _ = main_db.shutdown();
        drop(main_db);
        let _ = std::fs::remove_dir_all(MULTI_DIR);
    }
    #[test]
    fn test_e2e_drop_user() {
        let _guard = db_lock();
        let mut db = open_test_db();
        let mut session = Session::new();

        // 第一个用户自动成为 admin
        exec_sql(&mut db, &mut session, "CREATE USER alice PASSWORD 'pw'").unwrap();
        exec_sql(&mut db, &mut session, "LOGIN alice PASSWORD 'pw'").unwrap();
        exec_sql(&mut db, &mut session, "CREATE TABLE du_t (id INT)").unwrap();
        exec_sql(&mut db, &mut session, "CREATE USER bob PASSWORD 'pw2'").unwrap();
        exec_sql(&mut db, &mut session, "GRANT SELECT ON du_t TO bob").unwrap();

        // ① 不能删自己
        assert_err_contains(exec_sql(&mut db, &mut session, "DROP USER alice"), "logged in as");
        // ② 不存在的用户
        assert_err_contains(exec_sql(&mut db, &mut session, "DROP USER nosuch"), "does not exist");

        // ③ 删除成功，并且 bob 的授权被级联清空
        exec_sql(&mut db, &mut session, "DROP USER bob").unwrap();
        match exec_sql(&mut db, &mut session, "SELECT username FROM __minidb_privs").unwrap() {
            ResultSet::Rows { rows, .. } => assert!(rows.is_empty(), "bob 的授权应被清空"),
            other => panic!("got {:?}", other),
        }

        // ④ 重建同名用户 → 第二层防御：不能继承旧权限
        exec_sql(&mut db, &mut session, "CREATE USER bob PASSWORD 'pw3'").unwrap();
        exec_sql(&mut db, &mut session, "LOGIN bob PASSWORD 'pw3'").unwrap();
        assert_err_contains(exec_sql(&mut db, &mut session, "SELECT * FROM du_t"),
                            "permission denied");

        let _ = db.shutdown();
    }
    #[test]
    fn test_e2e_drop_view() {
        let _guard = db_lock();
        let mut db = open_test_db();
        let mut session = Session::new();

        exec_sql(&mut db, &mut session,
                 "CREATE TABLE dv_t (id INT PRIMARY KEY, age INT)").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO dv_t VALUES (1, 20)").unwrap();
        exec_sql(&mut db, &mut session, "CREATE VIEW dv_v AS SELECT id FROM dv_t").unwrap();

        // 删之前能查
        match exec_sql(&mut db, &mut session, "SELECT * FROM dv_v").unwrap() {
            ResultSet::Rows { rows, .. } => assert_eq!(cells_all(&rows), vec![vec!["1".to_string()]]),
            other => panic!("got {:?}", other),
        }

        // 删视图只动元数据，基表数据必须还在
        exec_sql(&mut db, &mut session, "DROP VIEW dv_v").unwrap();
        match exec_sql(&mut db, &mut session, "SELECT id FROM dv_t").unwrap() {
            ResultSet::Rows { rows, .. } => {
                assert_eq!(cells_all(&rows), vec![vec!["1".to_string()]], "基表不该被牵连")
            }
            other => panic!("got {:?}", other),
        }

        // 视图没了：查询退化成「表不存在」（走的是 open_table 的原始文案）
        assert_err_contains(exec_sql(&mut db, &mut session, "SELECT * FROM dv_v"),
                            "not found");
        // 重复删 → 明确说视图不存在
        assert_err_contains(exec_sql(&mut db, &mut session, "DROP VIEW dv_v"),
                            "view 'dv_v' does not exist");

        // 用 DROP TABLE 删视图 → 提示正确命令
        exec_sql(&mut db, &mut session, "CREATE VIEW dv_v2 AS SELECT id FROM dv_t").unwrap();
        assert_err_contains(exec_sql(&mut db, &mut session, "DROP TABLE dv_v2"),
                            "use DROP VIEW");

        let _ = db.shutdown();
    }
}
