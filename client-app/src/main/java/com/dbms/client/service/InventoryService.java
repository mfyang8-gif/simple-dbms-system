package com.dbms.client.service;

import com.dbms.client.dao.InventoryLogDao;
import com.dbms.client.dao.MaterialDao;
import com.dbms.client.dao.PersonnelDao;
import com.dbms.client.entity.InventoryLog;
import com.dbms.client.entity.Material;
import com.dbms.client.entity.Personnel;
import com.dbms.client.util.DbUtil;

import java.sql.Connection;
import java.sql.ResultSet;
import java.sql.SQLException;
import java.sql.Statement;
import java.time.LocalDate;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/**
 * 进出仓业务逻辑 —— 课设核心考点：事务控制。
 *
 * 关键点：整个方法必须共用同一条 Connection，才能保证
 *        BEGIN / COMMIT / ROLLBACK 作用在同一批操作上。
 */
public class InventoryService {

    public static final String IN = "I";
    public static final String OUT = "O";

    public static final String[] LOG_HEADERS =
            {"流水单号", "物料代码", "物料名称", "类型", "数量", "日期", "操作人", "备注"};

    private final MaterialDao materialDao = new MaterialDao();
    private final InventoryLogDao logDao = new InventoryLogDao();
    private final PersonnelDao personnelDao = new PersonnelDao();

    /** 单物料进 / 出仓。 */
    public void singleMove(int materialCode, String opType, int qty, int operatorId, String remarks) {
        List<Item> items = new ArrayList<>();
        items.add(new Item(materialCode, qty));
        multiMove(items, opType, operatorId, remarks);
    }

    /**
     * 多物料进 / 出仓 —— 同一单号必须全进或全出。
     * 任何一条失败，整批 ROLLBACK。
     */
    public void multiMove(List<Item> items, String opType, int operatorId, String remarks) {
        if (items == null || items.isEmpty()) {
            throw new IllegalArgumentException("请至少填写一种物料");
        }
        boolean inbound = IN.equalsIgnoreCase(opType);

        // 阶段①：事务外先做预校验，库存不足直接拒绝，不浪费一次数据库往返
        Map<Integer, Integer> needMap = new LinkedHashMap<>();
        for (Item it : items) {
            if (it.qty <= 0) {
                throw new IllegalArgumentException("数量必须大于 0");
            }
            needMap.merge(it.materialCode, it.qty, Integer::sum);
        }
        for (Map.Entry<Integer, Integer> e : needMap.entrySet()) {
            Material m = materialDao.findById(e.getKey());
            if (m == null) {
                throw new IllegalArgumentException("物料不存在：" + e.getKey());
            }
            if (!inbound && m.getStockQty() < e.getValue()) {
                throw new IllegalStateException("库存不足：" + m.getName()
                        + "（现有 " + m.getStockQty() + "，本单需要 " + e.getValue() + "）");
            }
        }

        // 阶段②：真正的强事务
        int nextLogId = logDao.nextLogId();
        String today = LocalDate.now().toString();
        Connection conn = null;
        try {
            conn = DbUtil.getConnection();
            conn.setAutoCommit(false);      // 等价于发送 BEGIN

            for (Item it : items) {
                int stock = queryStock(conn, it.materialCode);
                int newStock = inbound ? stock + it.qty : stock - it.qty;

                exec(conn, "UPDATE materials SET stock_qty = " + newStock
                        + " WHERE material_code = " + it.materialCode);

                InventoryLog log = new InventoryLog();
                log.setLogId(nextLogId++);
                log.setMaterialCode(it.materialCode);
                log.setOpType(opType);
                log.setQty(it.qty);
                log.setOpDate(today);
                log.setOperatorId(operatorId);
                log.setRemarks(remarks);
                InventoryLogDao.insert(conn, log);
            }

            conn.commit();                  // 等价于发送 COMMIT
        } catch (Exception e) {
            if (conn != null) {
                try {
                    conn.rollback();        // 等价于发送 ROLLBACK
                } catch (SQLException ignored) {
                }
            }
            throw new RuntimeException("操作失败，已全部回滚：" + e.getMessage(), e);
        } finally {
            DbUtil.closeQuietly(conn);
        }
    }

    /**
     * 按日期 / 物料代码 / 操作人查询流水（支持子串模糊）。
     * 日期在 SQL 里用 LIKE 过滤，代码和操作人在内存里过滤，
     * 这样完全绕开引擎"不能对 INT 列做模糊匹配"的限制。
     */
    public List<Object[]> queryLogs(String dateKeyword, String codeKeyword, String operatorKeyword) {
        Map<Integer, Material> materialMap = new HashMap<>();
        for (Material m : materialDao.search("")) {
            materialMap.put(m.getMaterialCode(), m);
        }
        Map<Integer, String> personMap = new HashMap<>();
        for (Personnel p : personnelDao.search("")) {
            personMap.put(p.getId(), p.getName());
        }

        List<Object[]> rows = new ArrayList<>();
        for (InventoryLog log : logDao.searchByDate(dateKeyword)) {
            if (codeKeyword != null && !codeKeyword.isBlank()
                    && !String.valueOf(log.getMaterialCode()).contains(codeKeyword.trim())) {
                continue;
            }
            if (operatorKeyword != null && !operatorKeyword.isBlank()
                    && !String.valueOf(log.getOperatorId()).contains(operatorKeyword.trim())) {
                continue;
            }
            Material m = materialMap.get(log.getMaterialCode());
            rows.add(new Object[]{
                    log.getLogId(),
                    log.getMaterialCode(),
                    m == null ? "(已删除)" : m.getName(),
                    log.opTypeLabel(),
                    log.getQty(),
                    log.getOpDate(),
                    personMap.getOrDefault(log.getOperatorId(), String.valueOf(log.getOperatorId())),
                    log.getRemarks() == null ? "" : log.getRemarks()
            });
        }
        return rows;
    }

    // ---------- 事务内使用的小工具 ----------

    private int queryStock(Connection conn, int code) throws SQLException {
        try (Statement st = conn.createStatement();
             ResultSet rs = st.executeQuery("SELECT stock_qty FROM materials WHERE material_code = " + code)) {
            if (!rs.next()) {
                throw new SQLException("物料不存在：" + code);
            }
            return rs.getInt(1);
        }
    }

    private void exec(Connection conn, String sql) throws SQLException {
        try (Statement st = conn.createStatement()) {
            st.executeUpdate(sql);
        }
    }

    /** 一张明细行：物料代码 + 数量。 */
    public static class Item {
        public final int materialCode;
        public final int qty;

        public Item(int materialCode, int qty) {
            this.materialCode = materialCode;
            this.qty = qty;
        }
    }
}
