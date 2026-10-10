package com.dbms.client.dao;

import com.dbms.client.entity.Personnel;
import com.dbms.client.util.DbUtil;
import com.dbms.client.util.SqlUtil;

import java.sql.Connection;
import java.sql.ResultSet;
import java.sql.SQLException;
import java.sql.Statement;
import java.util.ArrayList;
import java.util.List;

/** 人员档案访问层。只拼 SQL，不含业务判断。 */
public class PersonnelDao {

    /** 按姓名 / 籍贯 / 电话做子串模糊查询；keyword 为空则查全部。 */
    public List<Personnel> search(String keyword) {
        StringBuilder sql = new StringBuilder("SELECT * FROM personnel");
        if (keyword != null && !keyword.isBlank()) {
            String k = SqlUtil.like(keyword);
            sql.append(" WHERE name LIKE ").append(k)
               .append(" OR native_place LIKE ").append(k)
               .append(" OR phone LIKE ").append(k);
        }
        sql.append(" ORDER BY id");

        List<Personnel> list = new ArrayList<>();
        try (Connection c = DbUtil.getConnection();
             Statement st = c.createStatement();
             ResultSet rs = st.executeQuery(sql.toString())) {
            while (rs.next()) {
                list.add(map(rs));
            }
        } catch (SQLException e) {
            throw new RuntimeException("查询人员档案失败：" + e.getMessage(), e);
        }
        return list;
    }

    public Personnel findById(int id) {
        String sql = "SELECT * FROM personnel WHERE id = " + id;
        try (Connection c = DbUtil.getConnection();
             Statement st = c.createStatement();
             ResultSet rs = st.executeQuery(sql)) {
            return rs.next() ? map(rs) : null;
        } catch (SQLException e) {
            throw new RuntimeException("查询人员失败：" + e.getMessage(), e);
        }
    }

    public void insert(Personnel p) {
        String sql = "INSERT INTO personnel (id, name, gender, birth_date, id_card, native_place, address, phone, password, role_id) VALUES ("
                + p.getId() + ", "
                + SqlUtil.lit(p.getName()) + ", "
                + SqlUtil.lit(p.getGender()) + ", "
                + SqlUtil.lit(p.getBirthDate()) + ", "
                + SqlUtil.lit(p.getIdCard()) + ", "
                + SqlUtil.lit(p.getNativePlace()) + ", "
                + SqlUtil.lit(p.getAddress()) + ", "
                + SqlUtil.lit(p.getPhone()) + ", "
                + SqlUtil.lit(p.getPassword()) + ", "
                + p.getRoleId() + ")";
        update(sql, "新增人员失败");
    }

    public void update(Personnel p) {
        String sql = "UPDATE personnel SET "
                + "name = " + SqlUtil.lit(p.getName())
                + ", gender = " + SqlUtil.lit(p.getGender())
                + ", birth_date = " + SqlUtil.lit(p.getBirthDate())
                + ", id_card = " + SqlUtil.lit(p.getIdCard())
                + ", native_place = " + SqlUtil.lit(p.getNativePlace())
                + ", address = " + SqlUtil.lit(p.getAddress())
                + ", phone = " + SqlUtil.lit(p.getPhone())
                + ", role_id = " + p.getRoleId()
                + " WHERE id = " + p.getId();
        update(sql, "修改人员失败");
    }

    /** 权限授予 / 回收。 */
    public void updateRole(int id, int roleId) {
        update("UPDATE personnel SET role_id = " + roleId + " WHERE id = " + id, "修改权限失败");
    }

    public void updatePassword(int id, String encrypted) {
        update("UPDATE personnel SET password = " + SqlUtil.lit(encrypted) + " WHERE id = " + id, "修改口令失败");
    }

    public void delete(int id) {
        update("DELETE FROM personnel WHERE id = " + id, "删除人员失败");
    }

    public int nextId() {
        String sql = "SELECT MAX(id) FROM personnel";
        try (Connection c = DbUtil.getConnection();
             Statement st = c.createStatement();
             ResultSet rs = st.executeQuery(sql)) {
            int max = 1000;
            if (rs.next()) {
                max = Math.max(max, rs.getInt(1));
            }
            return max + 1;
        } catch (SQLException e) {
            throw new RuntimeException("生成人员代码失败：" + e.getMessage(), e);
        }
    }

    private void update(String sql, String errorMsg) {
        try (Connection c = DbUtil.getConnection();
             Statement st = c.createStatement()) {
            st.executeUpdate(sql);
        } catch (SQLException e) {
            throw new RuntimeException(errorMsg + "：" + e.getMessage(), e);
        }
    }

    private Personnel map(ResultSet rs) throws SQLException {
        Personnel p = new Personnel();
        p.setId(rs.getInt("id"));
        p.setName(rs.getString("name"));
        p.setGender(rs.getString("gender"));
        p.setBirthDate(rs.getString("birth_date"));
        p.setIdCard(rs.getString("id_card"));
        p.setNativePlace(rs.getString("native_place"));
        p.setAddress(rs.getString("address"));
        p.setPhone(rs.getString("phone"));
        p.setPassword(rs.getString("password"));
        p.setRoleId(rs.getInt("role_id"));
        return p;
    }
}
