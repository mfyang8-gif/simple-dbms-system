# client-app —— 桌面 UI 与报表统计（Java + Swing）

> 负责人：桌面 UI 与报表统计。
> 职责：**所有交互界面 + 正则校验 + 月度报表 / 图形展示 / PDF·Excel 导出**。
>
> 本目录已是一套**可运行的最小可跑骨架**，按下面步骤跑通后，再按第 6 步逐个把功能做厚。

---

## 1. 环境准备

| 需要 | 版本 |
|---|---|
| JDK | 17+ |
| MySQL | 8.0 |
| IDEA | 任意（自带 Maven，不用单独装） |
| Git | for Windows |

> ⚠️ **不要用 IDEA 打开仓库根目录**（根目录混着 Rust 和 Java，依赖分析会乱）。
> 只 `File → Open` 选中 **`client-app`** 这一个文件夹。

---

## 2. 建库建表（第一次必做）

```bash
cd client-app
mysql -u root -p < sql/init.sql
```

或直接把 `sql/init.sql` 拖进 Navicat / DataGrip 执行。会建 3 张表 + 少量演示数据。

---

## 3. 改数据库连接

打开 `config/db.properties`，把 `db.user` / `db.password` 改成你本机 MySQL 的账号密码。

---

## 4. 跑起来

IDEA 里打开 `src/main/java/com/dbms/client/App.java` → 点绿色三角运行。

命令行（装了 Maven 的话）：

```bash
mvn compile exec:java
```

**测试账号**：`1001 / 123456`（管理员）、`1002 / 123456`（普通用户）。
用 1001 登录能看到「权限管理」菜单，用 1002 登录看不到 —— 这就是子系统四要求的"动态控制菜单可见性"。

打包成可双击运行的 jar：

```bash
mvn clean package
# 产物：target/dbms-client.jar（记得按"班级+座号+程序名"改 pom.xml 里的 finalName）
```

---

## 5. 目录结构（谁的地盘是谁的）

```
client-app/
├── pom.xml                      Maven 依赖与打包配置
├── config/db.properties         数据库连接（切换引擎只改这里）
├── sql/init.sql                 建表 + 演示数据
└── src/main/java/com/dbms/client/
    ├── App.java                 程序入口
    ├── entity/                  ① 数据实体（业务逻辑同学负责，与你共用）
    ├── dao/                     ② SQL 访问层（业务逻辑同学负责）
    ├── service/                 ③ 业务逻辑 + 事务（业务逻辑同学负责）
    ├── util/                    ④ 你：DbUtil / SqlUtil / RegexValidator / PasswordUtil
    ├── report/                  ⑤ 你：Exporters（Excel / PDF / 图表 PNG）
    └── ui/                      ⑥ 你：全部界面
```

**铁律：`ui/` 只调用 `service/`，绝不自己拼 SQL、不 import `dao/`。**
业务逻辑同学重写 dao/service 时，你的界面一行都不用改。

---

## 6. 开发顺序（对应规范文档第七、十一章）

| 顺序 | 要做的事 | 涉及文件 | 里程碑 |
|---|---|---|---|
| ① | 跑通登录 → 主菜单 | `ui/LoginFrame` `ui/MainFrame` `service/AuthService` | M1 / M4 |
| ② | 人员档案增删改查 + 正则校验 | `ui/PersonnelPanel` `service/PersonnelService` | M2 |
| ③ | 物料档案增删改查 | `ui/MaterialPanel` `dao/MaterialDao` | M2 |
| ④ | 单物料 / 多物料进出仓 + 流水查询 | `ui/InventoryPanel` `service/InventoryService` | M3 |
| ⑤ | 权限授予/回收 + 菜单动态显示 | `ui/PersonnelPanel` `ui/MainFrame` | M4 |
| ⑥ | 统计图表 + 月度单据 + 账本 + 导出 | `ui/ReportPanel` `service/ReportService` `report/Exporters` | M4 |

骨架里 ①~⑥ 都已有可运行实现，你要做的是**照着当前写法把细节做厚**（更多字段校验、更多查询条件、表格排序、界面美化等）。

---

## 7. 报表统计的实现策略（重点，也是答辩亮点）

底层 Rust 引擎的限制：**无子查询、GROUP BY 只能单列、无 HAVING、`op_date` 是 VARCHAR 做不了日期函数**。

所以本项目统一采用：

```
明细一次拉回内存 → 用 Java 做分组 / 聚合 / 关联 → 出图表和表格
```

`ReportService` 里三张报表都是这么算的：
- **按物料流量**：`flowByMaterial()`，Map 累加进仓/出仓
- **月度趋势**：`monthlyFlow(year)`，用 `op_date.substring(0,7)` 分月
- **仓库账本**：`ledger()`，期初 = 当前库存 − 入库 + 出库 倒推

数据量是课设级别，性能毫无压力，但**100% 不会踩引擎的坑**。等引擎联调稳定后想炫技，再改成 SQL 的 `GROUP BY` + `SUM`。

---

## 8. 两个必须提前和业务逻辑同学确认的坑

1. **JDBC 驱动**：规范文档说引擎"模拟 PostgreSQL 协议"、监听 5432，但又让前期用 MySQL(3306)。这俩驱动不一样，**不是"只改 url/port"**。必须问清楚：是标准 PG 协议（换成 `org.postgresql` 驱动）还是自定义协议（要他给驱动 jar）。
2. **PreparedStatement**：本项目所有 SQL 都用 `Statement` + `SqlUtil.lit()` 手拼，**故意不用 PreparedStatement**——手搓引擎很可能不支持预编译语句的参数绑定与元数据交互。若引擎实现了标准协议，再换成 PreparedStatement 更安全。

---

## 9. 提交代码（别踩冲突）

```bash
git pull
git add client-app/ui/ client-app/util/ client-app/report/ client-app/sql/ client-app/config/ client-app/pom.xml
git commit -m "新增：客户端界面与报表骨架"
git push
```

🚨 **绝对不要 `git add .`** —— 会把同学的 `dao/`、`service/` 一起提交，天天冲突。
