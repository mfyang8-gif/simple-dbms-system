package com.dbms.client.dao;

import com.dbms.client.entity.InventoryLog;
import com.dbms.client.util.DbUtil;
import com.dbms.client.util.SqlUtil;

import java.sql.Connection;
import java.sql.ResultSet;
import java.sql.SQLException;
import java.sql.Statement;
import java.util.ArrayList;
import java.util.List;

/** 进出仓记录访问层。 */
public class InventoryLogDao {

    /** 把全部流水一次拉回内存，报表统计一律在内存里聚合（避开引擎的 SQL 限制）。 */
    public List<InventoryLog> listAll() {
        List<InventoryLog> list = new ArrayList<>();
        try (Connection c = DbUtil.getConnection();
             Statement st = c.createStatement();
             ResultSet rs = st.executeQuery("SELECT * FROM inventory_logs ORDER BY log_id")) {
            while (rs.next()) {
                list.add(map(rs));
            }
        } catch (SQLException e) {
            throw new RuntimeException("查询进出仓记录失败：" + e.getMessage(), e);
        }
        return list;
    }

    /**
     * 按日期做 SQL 层面的模糊查询（op_date 是 VARCHAR，用 LIKE 最稳）；
     * 物料代码和操作人这两个条件的子串匹配交给上层在内存里过滤。
     */
    public List<InventoryLog> searchByDate(String dateKeyword) {
        StringBuilder sql = new StringBuilder("SELECT * FROM inventory_logs");
        if (dateKeyword != null && !dateKeyword.isBlank()) {
            sql.append(" WHERE op_date LIKE ").append(SqlUtil.like(dateKeyword));
        }
        sql.append(" ORDER BY log_id");

        List<InventoryLog> list = new ArrayList<>();
        try (Connection c = DbUtil.getConnection();
             Statement st = c.createStatement();
             ResultSet rs = st.executeQuery(sql.toString())) {
            while (rs.next()) {
                list.add(map(rs));
            }
        } catch (SQLException e) {
            throw new RuntimeException("查询进出仓记录失败：" + e.getMessage(), e);
        }
        return list;
    }

    /** 在已有连接上插入（事务中调用，不能自己开关连接）。 */
    public static void insert(Connection conn, InventoryLog log) throws SQLException {
        String sql = "INSERT INTO inventory_logs (log_id, material_code, op_type, qty, op_date, operator_id, remarks) VALUES ("
                + log.getLogId() + ", "
                + log.getMaterialCode() + ", "
                + SqlUtil.lit(log.getOpType()) + ", "
                + log.getQty() + ", "
                + SqlUtil.lit(log.getOpDate()) + ", "
                + log.getOperatorId() + ", "
                + SqlUtil.lit(log.getRemarks()) + ")";
        try (Statement st = conn.createStatement()) {
            st.executeUpdate(sql);
        }
    }

    public void insert(InventoryLog log) {
        try (Connection c = DbUtil.getConnection()) {
            insert(c, log);
        } catch (SQLException e) {
            throw new RuntimeException("写入进出仓记录失败：" + e.getMessage(), e);
        }
    }

    public int nextLogId() {
        try (Connection c = DbUtil.getConnection();
             Statement st = c.createStatement();
             ResultSet rs = st.executeQuery("SELECT MAX(log_id) FROM inventory_logs")) {
            int max = 0;
            if (rs.next()) {
                max = rs.getInt(1);
            }
            return max + 1;
        } catch (SQLException e) {
            throw new RuntimeException("生成流水单号失败：" + e.getMessage(), e);
        }
    }

    private InventoryLog map(ResultSet rs) throws SQLException {
        InventoryLog log = new InventoryLog();
        log.setLogId(rs.getInt("log_id"));
        log.setMaterialCode(rs.getInt("material_code"));
        log.setOpType(rs.getString("op_type"));
        log.setQty(rs.getInt("qty"));
        log.setOpDate(rs.getString("op_date"));
        log.setOperatorId(rs.getInt("operator_id"));
        log.setRemarks(rs.getString("remarks"));
        return log;
    }
}
