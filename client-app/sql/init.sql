-- ============================================================
-- 建库建表脚本（严格执行《核心数据表结构字典》，字段不要改）
-- 执行方式：MySQL 命令行 或 Navicat / DataGrip 里直接跑
--   mysql -u root -p < init.sql
-- ============================================================

DROP DATABASE IF EXISTS dbms_course;
CREATE DATABASE dbms_course DEFAULT CHARACTER SET utf8mb4;
USE dbms_course;

CREATE TABLE personnel (
    id           INT PRIMARY KEY,
    name         VARCHAR(50),
    gender       VARCHAR(10),
    birth_date   VARCHAR(20),
    id_card      VARCHAR(20),
    native_place VARCHAR(50),
    address      VARCHAR(100),
    phone        VARCHAR(20),
    password     VARCHAR(64),
    role_id      INT
);

CREATE TABLE materials (
    material_code INT PRIMARY KEY,
    name          VARCHAR(50),
    spec          VARCHAR(50),
    unit          VARCHAR(20),
    stock_qty     INT
);

CREATE TABLE inventory_logs (
    log_id        INT PRIMARY KEY,
    material_code INT,
    op_type       VARCHAR(10),
    qty           INT,
    op_date       VARCHAR(20),
    operator_id   INT,
    remarks       VARCHAR(100)
);

-- ============================================================
-- 初始数据
-- password 存的是 SHA-256("123456")，登录时输入明文 123456 即可
-- ============================================================
INSERT INTO personnel VALUES
 (1001,'张三','男','1990-05-12','110101199005121234','北京','北京市朝阳区建国路1号','13800000001',
  '8d969eef6ecad3c29a3a629280e686cf0c3f5d5a86aff3ca12020c923adc6c92', 1),
 (1002,'李四','女','1995-08-20','310101199508201234','上海','上海市浦东新区世纪大道2号','13800000002',
  '8d969eef6ecad3c29a3a629280e686cf0c3f5d5a86aff3ca12020c923adc6c92', 0);

INSERT INTO materials VALUES
 (1001,'螺丝','M4x10','个',150),
 (1002,'螺母','M4',   '个',200),
 (1003,'垫片','M4',   '个',50);

INSERT INTO inventory_logs VALUES
 (1,1001,'I',200,'2025-09-03',1001,'期初入库'),
 (2,1002,'I',300,'2025-09-05',1001,'期初入库'),
 (3,1001,'O',100,'2025-09-10',1002,'生产领用'),
 (4,1003,'I', 80,'2025-09-12',1001,'采购入库'),
 (5,1002,'O',100,'2025-09-20',1002,'生产领用'),
 (6,1003,'O', 30,'2025-10-02',1002,'生产领用'),
 (7,1001,'I', 50,'2025-10-06',1001,'补充采购');

SELECT '初始化完成' AS msg;
