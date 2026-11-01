package com.dbms.client.console;

import java.util.List;

/**
 * 演示用 SQL 片段。
 *
 * {@code supported} 标记的是「当前引擎是否已实现该能力」，用于在菜单里如实提示，
 * 避免答辩现场点一个必然报错的示例却说不清是引擎没做还是客户端写错。
 * 随着引擎补上 JOIN / 视图 / 聚合 / 授权，把这些标记改成 true 即可，不需要改执行逻辑。
 */
public final class SqlSnippets {

    public record Snippet(String name, String sql, boolean supported) {
    }

    private SqlSnippets() {
    }

    public static List<Snippet> all() {
        return List.of(
                new Snippet("建表：物料表", """
                        CREATE TABLE demo_material (
                          material_code INT PRIMARY KEY,
                          name VARCHAR(50),
                          spec VARCHAR(50),
                          stock_qty INT
                        )""", true),
                new Snippet("插入数据", """
                        INSERT INTO demo_material VALUES (2001, '螺栓', 'M8x30', 120)""", true),
                new Snippet("单表查询：条件 + 排序", """
                        SELECT * FROM demo_material WHERE stock_qty > 0 ORDER BY stock_qty DESC""", true),
                new Snippet("更新数据", """
                        UPDATE demo_material SET stock_qty = 100 WHERE material_code = 2001""", true),
                new Snippet("删除数据", """
                        DELETE FROM demo_material WHERE material_code = 2001""", true),
                new Snippet("事务：显式提交", """
                        BEGIN;
                        UPDATE demo_material SET stock_qty = 10 WHERE material_code = 2001;
                        COMMIT;""", true),
                new Snippet("事务：回滚（前一条已生效后失败）", """
                        -- 先执行 BEGIN，再改两条，最后 ROLLBACK，观察第一条是否被撤销
                        BEGIN;
                        UPDATE demo_material SET stock_qty = 1 WHERE material_code = 2001;
                        UPDATE demo_material SET stock_qty = -5 WHERE material_code = 2001;
                        ROLLBACK;""", true),
                new Snippet("连接查询（需引擎支持 JOIN）", """
                        SELECT m.name, l.qty FROM materials m, inventory_logs l WHERE m.material_code = l.material_code""", false),
                new Snippet("聚合统计（需引擎支持 COUNT/SUM）", """
                        SELECT material_code, SUM(qty) FROM inventory_logs GROUP BY material_code""", false),
                new Snippet("视图（需引擎支持 CREATE VIEW）", """
                        CREATE VIEW v_stock AS SELECT material_code, name, stock_qty FROM materials""", false),
                new Snippet("索引（需引擎支持 CREATE INDEX）", """
                        CREATE INDEX idx_material_code ON inventory_logs (material_code)""", false),
                new Snippet("授权（需引擎支持 GRANT）", """
                        GRANT SELECT ON materials TO user1""", false)
        );
    }
}
