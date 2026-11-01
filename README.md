# MiniDB —— 教学型关系数据库管理系统

从零实现的关系数据库管理系统：Rust 引擎（约 12,000 行，**不依赖任何外部 crate**）+ Java Swing 图形客户端。
支持页式存储、WAL/ARIES 恢复、事务、多库、表级访问控制、视图、多表连接与聚合查询，通过自定义 TCP 文本协议对外服务。

---

## 1. 目录结构

```
simple-dbms-project/
├── db-engine/                  # Rust 引擎（crate: db-engine）
│   ├── src/
│   │   ├── main.rs             # 入口：--repl 交互 / 指定地址起服务
│   │   ├── database.rs         # Database（单库）+ DbRegistry（多库懒加载、切换、删除）
│   │   ├── config.rs           # 页大小、默认数据/日志路径
│   │   ├── storage/            # DiskManager / BufferPool / Page / FileHeader / FreePageManager
│   │   ├── recovery/           # LogManager / LogRecord / LSN / Transaction / Restore(ARIES)
│   │   ├── catalog/            # 系统目录：表元数据、列定义、持久化与多页扩展
│   │   ├── table/              # 堆表：插入/扫描/删除、记录格式、列定义与默认值
│   │   ├── record/ row/        # 记录编解码、行视图
│   │   ├── types/              # Value（Int / Str / Null）与比较
│   │   ├── sql/                # token / lexer / ast / parser / executor（约 6,300 行）
│   │   └── server/             # TCP 服务、每连接会话、REPL、协议编解码
│   └── Cargo.toml              # [dependencies] 为空
└── docs/                       # 课程报告与自评表（见第 9 节）
```

Java 客户端在**同一仓库的 `dev-client` 分支**（worktree 为 `../simple-dbms-backend`），包名 `com.dbms.client`，通过本协议直连引擎，不使用 JDBC。注意：`dev-engine` 分支上的 `client-app/` 只有一个 0 字节占位 `README.MD`，客户端源码不在本分支的工作区里。

---

## 2. 功能清单

### 2.1 必选功能

| 功能 | 状态 | 说明 |
|---|---|---|
| 创建数据库 | ✅ | `CREATE DATABASE` / `DROP DATABASE` / `USE`；每库独立 `.db` + `.log`，按需懒加载 |
| 创建表 | ✅ | `CREATE TABLE`，支持 `INT` / `VARCHAR(n)` 两类型，列约束 `PRIMARY KEY` / `UNIQUE` / `NOT NULL` / `DEFAULT` |
| 表数据基本操作 | ✅ | `INSERT` / `SELECT` / `UPDATE` / `DELETE` / `TRUNCATE TABLE` |
| 单表查询 | ✅ | `WHERE`（比较、`AND`/`OR`/`NOT`、`LIKE`、`IS [NOT] NULL`）、`ORDER BY`、`LIMIT`、`DISTINCT`、聚合（`COUNT`/`SUM`/`AVG`/`MIN`/`MAX`）、`GROUP BY`、`HAVING` |
| 连接查询 | ✅ | 多表内连接，`JOIN ... ON`，递归拼接带 ON 剪枝 |
| 创建视图并查询 | ✅ | `CREATE VIEW v AS SELECT ...` / `DROP VIEW`；查询时展开为底层 `SELECT` 复用执行器 |
| 访问控制 | ✅ | `CREATE USER` / `LOGIN` / `GRANT` / `REVOKE` / `DROP USER`，表级 `SELECT`/`INSERT`/`UPDATE`/`DELETE` |
| SQL 规范性 | ⚠️ 部分 | 无子查询、无 `UNION`、无 `ALTER TABLE ADD/DROP COLUMN`（见第 8 节） |

### 2.2 可选功能

| 功能 | 状态 | 说明 |
|---|---|---|
| 日志 | ✅ | WAL + ARIES 三阶段恢复（分析 / 重做 / 撤销） |
| 表空间管理 | ✅ | 页式存储、空闲页管理、文件头 + 数据页链 |
| 任意多表连接 | ✅ | 连接层不限表数 |
| 远程访问 | ✅ | TCP 服务，每连接独立会话 |
| 索引 | ❌ 未实现 | 无索引结构，查询一律全表扫描（见第 8 节） |

### 2.3 附加功能

| 功能 | 状态 | 说明 |
|---|---|---|
| 网络访问 + 类 JDBC 接口 | ✅ | TCP 协议 + Java 侧 `MiniDbConnection` / `Statement` / `ResultSet` |
| 图形化控制台 | ✅ | Java Swing 客户端 |
| 更新时原地修改记录 | ❌ | `UPDATE` 实现为「删除旧行 + 插入新行」（删除标记、空位复用、文件预分配均已实现，仅原地覆写未做） |

---

## 3. 架构

```
             ┌──────────────────────────────────────────────┐
  客户端 ──► │ server        每连接一个 Session             │
  (TCP)      │   txn_id / user / current_db / peer_is_local │
             └───────────────┬──────────────────────────────┘
                             ▼
             ┌──────────────────────────────────────────────┐
             │ sql     lexer → parser → AST → executor      │
             │         authorize(need, touched) 权限校验    │
             └───────┬───────────────────────┬──────────────┘
                     ▼                       ▼
        ┌─────────────────────┐   ┌──────────────────────────┐
        │ catalog  表元数据   │   │ table   堆表 / 记录编解码 │
        │ (多页链表持久化)    │   │ scan_all / insert / delete│
        └─────────┬───────────┘   └───────────┬──────────────┘
                  └───────────┬───────────────┘
                              ▼
             ┌──────────────────────────────────────────────┐
             │ storage   BufferPool ← DiskManager           │
             │           Page / FileHeader / FreePageManager │
             └───────────────┬──────────────────────────────┘
                             ▼
             ┌──────────────────────────────────────────────┐
             │ recovery  LogManager(WAL) / ARIES Restore     │
             └──────────────────────────────────────────────┘
```

关键设计取舍：

- **一切经 WAL**：行级删除打标记、插入走事务号，崩溃后按 LSN 重做 / 撤销。`TRUNCATE` 因此实现为逐行 `delete` 而非直接释放页——直接改 `free_mgr` 不参与日志，崩溃后会与 catalog 不一致。
- **权限在执行前统一校验**：`authorize` 返回（所需权限，触及的表名）二元组，内部表（`__minidb_` 前缀）另有单独拦截；「表不存在」与「无权限」返回同一文案，避免用报错文案探测库结构。
- **多库按文件隔离**：`data/<db>.db` + `data/<db>.log`，`DbRegistry` 懒加载与关闭；`DROP DATABASE` 先关句柄，再删 `.log`，最后删 `.db`。

---

## 4. 构建与运行

```bash
cd db-engine

cargo build --release          # 构建
cargo test                     # 全部测试（当前 164 passed / 0 failed）
```

三种启动方式：

```bash
cargo run --release                     # 监听 127.0.0.1:9000
cargo run --release -- 0.0.0.0:9000     # 指定监听地址
cargo run --release -- --repl           # 本地交互式 REPL
```

数据文件默认落在 `./data/`（`db.db` / `db.log`），库名取文件名去后缀，故默认库名是 `db`。

---

## 5. SQL 支持

```sql
-- 库
CREATE DATABASE shop;   USE shop;   DROP DATABASE shop;

-- 表
CREATE TABLE users (
  id   INT PRIMARY KEY,
  name VARCHAR(32) NOT NULL UNIQUE,
  age  INT DEFAULT 18
);
DROP TABLE users;
TRUNCATE TABLE users;                        -- 可回滚：BEGIN; TRUNCATE; ROLLBACK 数据复原
ALTER TABLE users RENAME COLUMN name TO nickname;
ALTER TABLE users SET DEFAULT age = 20;
ALTER TABLE users DROP DEFAULT age;
-- 其余形态（ADD COLUMN / DROP COLUMN / RENAME TO）统一报 not supported yet

-- 数据
INSERT INTO users (name, age) VALUES ('张三', 20);
SELECT * FROM users WHERE age > 18 ORDER BY age DESC LIMIT 10;
UPDATE users SET age = 21 WHERE id = 1;
DELETE FROM users WHERE id = 1;

-- 聚合与分组
SELECT age, COUNT(*) FROM users GROUP BY age HAVING COUNT(*) > 1;

-- 连接（不支持表别名，用限定列名）
SELECT users.name, orders.amount FROM users JOIN orders ON users.id = orders.uid;

-- 视图
CREATE VIEW adult AS SELECT * FROM users WHERE age >= 18;
SELECT * FROM adult;
DROP VIEW adult;

-- 元数据
SHOW TABLES;
DESCRIBE users;                              -- column/type/nullable/default/primary_key/unique

-- 访问控制（表级）
CREATE USER alice PASSWORD 'p@ss';
LOGIN alice PASSWORD 'p@ss';
GRANT SELECT, INSERT ON users TO alice;
REVOKE INSERT ON users FROM alice;
DROP USER alice;

-- 事务
BEGIN;  COMMIT;  ROLLBACK;
```

---

## 6. 网络协议

一行一条 SQL，服务端按语句返回一段文本：

| 结果 | 应答 |
|---|---|
| 无返回集 | `OK 0` |
| 影响 n 行 | `OK n` |
| 结果集 | `ROWS <列数> <行数>\n<表头行>\n<数据行…>\nEND` |
| 出错 | `ERR <消息>` |

- 字段以 **TAB** 分隔，字段内的 `\t` / `\n` / `\\` 转义；
- NULL 写作 **`\N`**（与 MySQL `LOAD DATA` 习惯一致），不是空串；
- 表头行在前，列数与行数可让客户端预分配；
- **空行（含纯空格、纯 TAB）回 `OK 0`**，`quit` 同样回 `OK 0` 后断开——每个请求都保证有且只有一条应答，客户端可以严格「发一条、收一条」。

示例：

```
> SELECT id, name FROM users;
< ROWS 2 2
< id	name
< 1	张三
< 2	李四
< END
```

---

## 7. 客户端集成

Java 侧（`dev-client` 分支的 `client-app`）不使用 JDBC，直接按上述协议收发：

- `MiniDbConnection`：host + port，建立 TCP 长连接；
- `MiniDbStatement`：`executeQuery` 发 SQL，`ResultSet` 解析 `ROWS/END`；
- `MiniDbResultSet`：游标遍历，`getString` 把 `\N` 还原为 `null`。

**连接后若库中已存在任何用户，必须先发 `LOGIN <user> PASSWORD '<pwd>'`，否则所有语句都会返回 `please LOGIN first`。** 新库在没有任何用户时是开放状态（首次部署免登录即可建库建表）。

实测注意（已联调验证）：`MiniDbConnection` 只带 host/port，**本身不发 `LOGIN`**；图形控制台可以手工输入 `LOGIN` 语句，但仓库管理 GUI（`com.dbms.client.App`）没有登录入口。因此**一旦库里创建了用户，业务 GUI 的所有操作都会失败**（实测返回 `please LOGIN first`）。

演示编排上的规避办法：**GUI 段排在权限段之前**——先用零用户的库演示增删改查 / JOIN / 视图 / 聚合（零用户时允许本地匿名，GUI 直接可用），再用 REPL 演示 `CREATE USER` / `GRANT` / 越权被拒 / 事务回滚 / 崩溃恢复（这一段本来就要看错误原文）。顺序不能颠倒。

本节涉及的客户端行为，是在 `dev-client` 工作区的源码上编译验证的（含该工作区当时未提交的改动），不是 `dev-engine` 分支的内容，也不是 10-27 构建的旧 jar。

---

## 8. 已知限制

按影响程度排序，均为已知且经过权衡，不是未发现的缺陷：

1. **无索引**。查询一律全表扫描，多表连接是嵌套循环（三表近似 O(n³)）。任务书第 6 页将索引列为可选，第 17 页将其写入要求清单，两处口径不一致；本项目按「可选」处理，未实现。
2. **并发是串行的**。`server` 用一把全局互斥锁保护 `DbRegistry`，所有连接、所有库串行执行。正确性有保证，吞吐上限为单核顺序执行，无可并行的读。
3. **协议无加密无压缩**。明文 TAB 文本，可读性优先；未做 TLS，未做压缩。
4. **`UPDATE` 非原地修改**。实现为「删除旧行 + 插入新行」，页内会留下可复用空洞；附加功能 1 中「原地修改」一条未满足。
5. **WAL 无检查点、无日志截断**。日志只增不减，长期运行会持续增长，需手动清理或重启归档。
6. **`DEFAULT` 值受 `u32` 限制**。列默认值以 `DefaultVal::Int(u32)` 存储，且解析处是 `n as u32`：`DEFAULT -1` 会变成 `4294967295`，`DEFAULT 5000000000` 同样回绕。DDL 阶段既不校验范围，也不校验默认值与列类型是否匹配（`INT` 列 `DEFAULT 'abc'` 会被接受，错误延迟到写入该列时才暴露）。
7. **视图列名是创建时刻的快照**。基表列被 `RENAME COLUMN` 后，视图会在下次查询时报 `column 'x' not found`，与基表被 `DROP` 时的延迟失效行为一致；未做级联更新。
8. **SQL 子集**。无子查询、无 `UNION`、无 `ALTER TABLE ADD/DROP COLUMN`、无外键、无 `CHECK` 约束；类型仅 `INT` 与 `VARCHAR(n)`。**不支持表别名**（`FROM users u`、`FROM users AS u` 均报语法错误）与逗号连接（`FROM a, b`），多表连接必须写成 `FROM a JOIN b ON a.x = b.x` 并用全限定列名。
9. **口令哈希非 KDF**。FNV-1a 64 位、无盐无迭代，仅满足教学演示，不可用于生产；客户端侧算法需与引擎保持一致才能登录。

---

## 9. 交付物与文档

- [x] 可运行系统（引擎 + 客户端）
- [x] 单元 / 端到端测试：`cargo test` 164 passed / 0 failed
- [ ] 课程报告（架构、WAL/ARIES、事务、连接算法、功能取舍论证）
- [ ] 团队工作质量自评表、团队工作量自评表
- [ ] 第 6 周个人开发小结（补交）

`docs/` 目录用于存放上述文档。

---

## 10. 测试

```bash
cd db-engine && cargo test
```

- 当前：**164 passed / 0 failed**（默认并行执行）；
- 覆盖：存储页管理、WAL 与 ARIES 恢复、事务提交 / 回滚、catalog 持久化与多页扩展、SQL 各语句端到端、权限校验、多库切换与删除、协议编解码；
- 每个测试使用独立的数据文件名（如 `data/test_catalog_create_list.db`），因此可并行运行、互不干扰。
