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
    pub fn new() -> Self { Self { txn_id: None } }
}

/// 执行一条 AST，返回结果集
pub fn execute(stmt: Statement, db: &mut Database, session: &mut Session) -> Result<ResultSet, String> {
    match stmt {
        Statement::CreateTable { name, columns } => {
            execute_create_table(name, columns, db)
        }
        Statement::DropTable { name } => execute_drop_table(name, db),
        Statement::Insert { table, columns, values } => execute_insert(table, columns, values, db, session),
        Statement::Select { columns, table, joins, filter, group_by, having, order_by, limit, .. } => {
            execute_select(columns, table, joins, filter, group_by, having, order_by, limit, db)
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
        Statement::Update { table, assignments, filter } => execute_update(table, assignments, filter, db, session),
    }
}


fn execute_drop_table(name: String, db: &mut Database) -> Result<ResultSet, String> {
    db.drop_table(&name).map_err(|e| format!("drop_table failed: {}", e))?;
    Ok(ResultSet::Ok)
}
fn execute_insert(table_name: String, columns: Vec<String>, values: Vec<Expr>, db: &mut Database, session: &mut Session) -> Result<ResultSet, String> {
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

    // 3. 严格校验：值数量必须等于目标列数量；重复列名不能默默后者覆盖前者
    if values.len() != targets.len() {
        return Err(format!("expected {} values, got {}", targets.len(), values.len()));
    }
    let mut slot_of: Vec<Option<usize>> = vec![None; table.columns.len()];
    for (v_idx, &col_idx) in targets.iter().enumerate() {
        if slot_of[col_idx].is_some() {
            return Err(format!("duplicate column '{}' in INSERT", table.columns[col_idx].name));
        }
        slot_of[col_idx] = Some(v_idx);
    }

    // 4. 逐列构造 Row（按 schema 顺序走完每一列）：
    //    没被点名的列：有 DEFAULT 就用默认值，没 DEFAULT 才写 NULL（能不能空交给 Table 层判）；
    //    类型/长度/NOT NULL 校验全部下沉到 Table 层的 TypedRowBuilder
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
    let row = builder.build().map_err(|e| format!("row build failed: {}", e))?;

    // 5. 调存储层（session 有事务 → 加入该事务；否则自动提交）
    db.insert_in_txn(&mut table, row, session.txn_id)
        .map_err(|e| format!("insert failed: {}", e))?;
    Ok(ResultSet::Affected(1))
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

/// LIKE 匹配（支持 % 通配符，双指针回溯法）
fn like_match(text: &str, pattern: &str) -> bool {
    let t: Vec<char> = text.chars().collect();
    let p: Vec<char> = pattern.chars().collect();
    let (mut ti, mut pi) = (0usize, 0usize);
    let (mut last_star_p, mut last_star_t) = (usize::MAX, 0usize);

    while ti < t.len() {
        if pi < p.len() && p[pi] == t[ti] {
            ti += 1; pi += 1;                      // 字符相等，双双前进
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
fn execute_select(
    columns: Vec<SelectItem>,
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

    // ④ 真正落盘：删旧 + 插新。两步都挂在同一条事务链上，ROLLBACK 能整体还原
    let count = rows.len();
    for (row_id, row) in rows {
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
        execute(stmt, db, session)
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
    fn test_e2e_update_rollback() {
        let _guard = db_lock();
        let mut db = open_test_db();
        let mut session = Session::new();

        exec_sql(&mut db, &mut session, "CREATE TABLE ur_emp (id INT PRIMARY KEY, name VARCHAR(50))").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO ur_emp VALUES (1, 'Alice')").unwrap();
        exec_sql(&mut db, &mut session, "INSERT INTO ur_emp VALUES (2, 'Bob')").unwrap();

        exec_sql(&mut db, &mut session, "BEGIN").unwrap();
        exec_sql(&mut db, &mut session, "UPDATE ur_emp SET name = 'Zed' WHERE id = 1").unwrap();

        // 事务里看得到新值
        match exec_sql(&mut db, &mut session, "SELECT name FROM ur_emp WHERE id = 1").unwrap() {
            ResultSet::Rows { rows, .. } => assert_eq!(cell(&rows[0][0]), "Zed"),
            other => panic!("got {:?}", other),
        }

        exec_sql(&mut db, &mut session, "ROLLBACK").unwrap();
        assert!(session.txn_id.is_none());

        // 回滚后：旧值回来，行数不变
        match exec_sql(&mut db, &mut session, "SELECT * FROM ur_emp ORDER BY id").unwrap() {
            ResultSet::Rows { rows, .. } => {
                assert_eq!(rows.len(), 2, "UPDATE 回滚后行数不能变");
                assert_eq!(cells(&rows[0]), vec!["1".to_string(), "Alice".to_string()]);
                assert_eq!(cells(&rows[1]), vec!["2".to_string(), "Bob".to_string()]);
            }
            other => panic!("got {:?}", other),
        }
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
}