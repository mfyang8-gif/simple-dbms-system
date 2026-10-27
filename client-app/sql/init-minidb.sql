-- ============================================================
-- MiniDB（Rust 引擎）建表 + 初始数据
-- 重要：MiniDB 是"一行 = 一条 SQL 语句"，因此每条语句必须写成单行，
--       不能像 MySQL 那样把列定义换行铺开，否则会被当成多条残句而解析失败。
-- 执行方式：用 client-app 的 MiniDB 客户端逐行发送（见 MiniDbSmoke / 初始化工具）。
-- ============================================================

CREATE TABLE personnel (id INT PRIMARY KEY, name VARCHAR(50), gender VARCHAR(10), birth_date VARCHAR(20), id_card VARCHAR(20), native_place VARCHAR(50), address VARCHAR(100), phone VARCHAR(20), password VARCHAR(64), role_id INT);
CREATE TABLE materials (material_code INT PRIMARY KEY, name VARCHAR(50), spec VARCHAR(50), unit VARCHAR(20), stock_qty INT);
CREATE TABLE inventory_logs (log_id INT PRIMARY KEY, material_code INT, op_type VARCHAR(10), qty INT, op_date VARCHAR(20), operator_id INT, remarks VARCHAR(100));

-- 初始数据：password 为 SHA-256("123456")，登录输入明文 123456 即可
INSERT INTO personnel VALUES (1001,'张三','男','1990-05-12','110101199005121234','北京','北京市朝阳区建国路1号','13800000001','8d969eef6ecad3c29a3a629280e686cf0c3f5d5a86aff3ca12020c923adc6c92',1);
INSERT INTO personnel VALUES (1002,'李四','女','1995-08-20','310101199508201234','上海','上海市浦东新区世纪大道2号','13800000002','8d969eef6ecad3c29a3a629280e686cf0c3f5d5a86aff3ca12020c923adc6c92',0);
INSERT INTO materials VALUES (1001,'螺丝','M4x10','个',150);
INSERT INTO materials VALUES (1002,'螺母','M4','个',200);
INSERT INTO materials VALUES (1003,'垫片','M4','个',50);
INSERT INTO inventory_logs VALUES (1,1001,'I',200,'2025-09-03',1001,'期初入库');
INSERT INTO inventory_logs VALUES (2,1002,'I',300,'2025-09-05',1001,'期初入库');
INSERT INTO inventory_logs VALUES (3,1001,'O',100,'2025-09-10',1002,'生产领用');
INSERT INTO inventory_logs VALUES (4,1003,'I',80,'2025-09-12',1001,'采购入库');
INSERT INTO inventory_logs VALUES (5,1002,'O',100,'2025-09-20',1002,'生产领用');
INSERT INTO inventory_logs VALUES (6,1003,'O',30,'2025-10-02',1002,'生产领用');
INSERT INTO inventory_logs VALUES (7,1001,'I',50,'2025-10-06',1001,'补充采购');
