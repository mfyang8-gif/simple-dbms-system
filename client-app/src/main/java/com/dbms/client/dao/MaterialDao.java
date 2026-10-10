package com.dbms.client.dao;

import com.dbms.client.entity.Material;
import com.dbms.client.util.DbUtil;
import com.dbms.client.util.SqlUtil;

import java.sql.Connection;
import java.sql.ResultSet;
import java.sql.SQLException;
import java.sql.Statement;
import java.util.ArrayList;
import java.util.List;

/** 物料档案访问层。 */
public class MaterialDao {

    public List<Material> search(String keyword) {
        StringBuilder sql = new StringBuilder("SELECT * FROM materials");
        if (keyword != null && !keyword.isBlank()) {
            String k = SqlUtil.like(keyword);
            sql.append(" WHERE name LIKE ").append(k)
               .append(" OR spec LIKE ").append(k)
               .append(" OR unit LIKE ").append(k);
        }
        sql.append(" ORDER BY material_code");

        List<Material> list = new ArrayList<>();
        try (Connection c = DbUtil.getConnection();
             Statement st = c.createStatement();
             ResultSet rs = st.executeQuery(sql.toString())) {
            while (rs.next()) {
                list.add(map(rs));
            }
        } catch (SQLException e) {
            throw new RuntimeException("查询物料档案失败：" + e.getMessage(), e);
        }
        return list;
    }

    public Material findById(int code) {
        String sql = "SELECT * FROM materials WHERE material_code = " + code;
        try (Connection c = DbUtil.getConnection();
             Statement st = c.createStatement();
             ResultSet rs = st.executeQuery(sql)) {
            return rs.next() ? map(rs) : null;
        } catch (SQLException e) {
            throw new RuntimeException("查询物料失败：" + e.getMessage(), e);
        }
    }

    public void insert(Material m) {
        String sql = "INSERT INTO materials (material_code, name, spec, unit, stock_qty) VALUES ("
                + m.getMaterialCode() + ", "
                + SqlUtil.lit(m.getName()) + ", "
                + SqlUtil.lit(m.getSpec()) + ", "
                + SqlUtil.lit(m.getUnit()) + ", "
                + m.getStockQty() + ")";
        exec(sql, "新增物料失败");
    }

    public void update(Material m) {
        String sql = "UPDATE materials SET "
                + "name = " + SqlUtil.lit(m.getName())
                + ", spec = " + SqlUtil.lit(m.getSpec())
                + ", unit = " + SqlUtil.lit(m.getUnit())
                + ", stock_qty = " + m.getStockQty()
                + " WHERE material_code = " + m.getMaterialCode();
        exec(sql, "修改物料失败");
    }

    public void delete(int code) {
        exec("DELETE FROM materials WHERE material_code = " + code, "删除物料失败");
    }

    /** 直接覆盖库存（事务里用）。 */
    public void updateStock(int code, int newQty) {
        exec("UPDATE materials SET stock_qty = " + newQty + " WHERE material_code = " + code, "更新库存失败");
    }

    public int nextCode() {
        try (Connection c = DbUtil.getConnection();
             Statement st = c.createStatement();
             ResultSet rs = st.executeQuery("SELECT MAX(material_code) FROM materials")) {
            int max = 1000;
            if (rs.next()) {
                max = Math.max(max, rs.getInt(1));
            }
            return max + 1;
        } catch (SQLException e) {
            throw new RuntimeException("生成物料代码失败：" + e.getMessage(), e);
        }
    }

    private void exec(String sql, String errorMsg) {
        try (Connection c = DbUtil.getConnection();
             Statement st = c.createStatement()) {
            st.executeUpdate(sql);
        } catch (SQLException e) {
            throw new RuntimeException(errorMsg + "：" + e.getMessage(), e);
        }
    }

    private Material map(ResultSet rs) throws SQLException {
        Material m = new Material();
        m.setMaterialCode(rs.getInt("material_code"));
        m.setName(rs.getString("name"));
        m.setSpec(rs.getString("spec"));
        m.setUnit(rs.getString("unit"));
        m.setStockQty(rs.getInt("stock_qty"));
        return m;
    }
}
