package com.dbms.client.service;

import com.dbms.client.dao.InventoryLogDao;
import com.dbms.client.dao.MaterialDao;
import com.dbms.client.entity.InventoryLog;
import com.dbms.client.entity.Material;

import java.util.ArrayList;
import java.util.Comparator;
import java.util.HashMap;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/**
 * 报表统计。
 *
 * 设计要点：底层引擎不支持子查询、不支持多列 GROUP BY、不支持 HAVING，
 * op_date 还是 VARCHAR 做不了日期函数。所以这里统一采取
 * "明细一次拉回内存 → 用 Java 做分组/聚合/关联" 的策略，
 * 数据量是课设级别，性能完全不是问题，但换来的是 100% 不会踩引擎的坑。
 *
 * 【第 1 课改动】所有查询方法都增加了日期区间参数（fromDate / toDate），
 * null 或空串表示该端不限制。为了不破坏原有调用，每个方法都保留了无参重载。
 */
public class ReportService {

    private final MaterialDao materialDao = new MaterialDao();
    private final InventoryLogDao logDao = new InventoryLogDao();

    /** 按物料统计进出仓流量。 */
    public static class FlowRow {
        public int code;
        public String name;
        public String unit;
        public int inQty;
        public int outQty;
        public int stock;
    }

    /** 物料仓库账本行。 */
    public static class LedgerRow {
        public int code;
        public String name;
        public String unit;
        public int opening;
        public int inQty;
        public int outQty;
        public int closing;
    }

    public static final String[] FLOW_HEADERS = {"物料代码", "物料名称", "单位", "进仓合计", "出仓合计", "当前库存"};
    public static final String[] LEDGER_HEADERS = {"物料代码", "物料名称", "单位", "期初", "入库", "出库", "结存"};
    public static final String[] MONTH_BILL_HEADERS =
            {"日期", "流水单号", "物料代码", "物料名称", "类型", "数量", "单位", "操作人ID", "备注"};

    // ============================================================
    // 日期区间筛选工具
    // ============================================================

    /**
     * 判断日期是否落在 [from, to] 区间内（null 或空串表示该端不限制）。
     *
     * 为什么直接用字符串比较？
     * op_date 存的是 '2025-09-03' 这种"零填充定长"格式，
     * 这种格式的字典序大小与日期先后完全一致，
     * 所以 compareTo 就等价于日期比较 —— 而且完全不依赖数据库的日期函数。
     */
    private static boolean inRange(String date, String from, String to) {
        if (date == null || date.isEmpty()) {
            return false;
        }
        if (from != null && !from.isEmpty() && date.compareTo(from) < 0) {
            return false;
        }
        if (to != null && !to.isEmpty() && date.compareTo(to) > 0) {
            return false;
        }
        return true;
    }

    // ============================================================
    // ① 按物料流量统计
    // ============================================================

    /** 按物料聚合进出仓流量 + 当前库存（不限日期）。 */
    public List<FlowRow> flowByMaterial() {
        return flowByMaterial(null, null);
    }

    /** 按物料聚合进出仓流量，只统计 [fromDate, toDate] 内的流水。 */
    public List<FlowRow> flowByMaterial(String fromDate, String toDate) {
        Map<Integer, Material> materials = new LinkedHashMap<>();
        for (Material m : materialDao.search("")) {
            materials.put(m.getMaterialCode(), m);
        }

        Map<Integer, int[]> agg = new HashMap<>();      // code -> [进仓合计, 出仓合计]
        for (InventoryLog log : logDao.listAll()) {
            if (!inRange(log.getOpDate(), fromDate, toDate)) {
                continue;                               // 区间外的流水直接跳过
            }
            int[] v = agg.computeIfAbsent(log.getMaterialCode(), k -> new int[2]);
            if (InventoryService.IN.equalsIgnoreCase(log.getOpType())) {
                v[0] += log.getQty();
            } else {
                v[1] += log.getQty();
            }
        }

        List<FlowRow> rows = new ArrayList<>();
        for (Material m : materials.values()) {
            FlowRow r = new FlowRow();
            r.code = m.getMaterialCode();
            r.name = m.getName();
            r.unit = m.getUnit();
            r.stock = m.getStockQty();
            int[] v = agg.getOrDefault(m.getMaterialCode(), new int[2]);
            r.inQty = v[0];
            r.outQty = v[1];
            rows.add(r);
        }
        return rows;
    }

    // ============================================================
    // ② 月度趋势
    // ============================================================

    /** 指定年份 12 个月的进出仓合计（不限月份区间）。 */
    public int[][] monthlyFlow(int year) {
        return monthlyFlow(year, null, null);
    }

    /**
     * 指定年份 12 个月的进出仓合计，可再叠加月份区间筛选。
     * 返回：[i][0]=第 i+1 月进仓，[i][1]=第 i+1 月出仓。
     */
    public int[][] monthlyFlow(int year, String fromDate, String toDate) {
        int[][] result = new int[12][2];
        String prefix = year + "-";
        for (InventoryLog log : logDao.listAll()) {
            String d = log.getOpDate();
            if (d == null || !d.startsWith(prefix) || d.length() < 7) {
                continue;
            }
            if (!inRange(d, fromDate, toDate)) {
                continue;
            }
            int month;
            try {
                month = Integer.parseInt(d.substring(5, 7));
            } catch (NumberFormatException e) {
                continue;
            }
            if (month < 1 || month > 12) {
                continue;
            }
            if (InventoryService.IN.equalsIgnoreCase(log.getOpType())) {
                result[month - 1][0] += log.getQty();
            } else {
                result[month - 1][1] += log.getQty();
            }
        }
        return result;
    }

    // ============================================================
    // ③ 月度进出仓单（明细）
    // ============================================================

    /** 指定年月的进出仓单明细。 */
    public List<Object[]> monthBill(int year, int month) {
        String prefix = String.format("%04d-%02d", year, month);
        return monthBillByRange(prefix + "-01", prefix + "-31");
    }

    /** 指定日期区间的进出仓单明细。 */
    public List<Object[]> monthBillByRange(String fromDate, String toDate) {
        Map<Integer, Material> materialMap = new HashMap<>();
        for (Material m : materialDao.search("")) {
            materialMap.put(m.getMaterialCode(), m);
        }

        List<InventoryLog> logs = new ArrayList<>();
        for (InventoryLog log : logDao.listAll()) {
            if (inRange(log.getOpDate(), fromDate, toDate)) {
                logs.add(log);
            }
        }
        logs.sort(Comparator.comparing(InventoryLog::getOpDate).thenComparingInt(InventoryLog::getLogId));

        List<Object[]> rows = new ArrayList<>();
        for (InventoryLog log : logs) {
            Material m = materialMap.get(log.getMaterialCode());
            rows.add(new Object[]{
                    log.getOpDate(),
                    log.getLogId(),
                    log.getMaterialCode(),
                    m == null ? "(已删除)" : m.getName(),
                    log.opTypeLabel(),
                    log.getQty(),
                    m == null ? "" : m.getUnit(),
                    log.getOperatorId(),
                    log.getRemarks() == null ? "" : log.getRemarks()
            });
        }
        return rows;
    }

    // ============================================================
    // ④ 物料仓库账本
    // ============================================================

    /** 物料仓库账本（不限日期）。 */
    public List<LedgerRow> ledger() {
        return ledger(null, null);
    }

    /**
     * 物料仓库账本：期初 + 入库 - 出库 = 结存。
     * 结存始终取"当前库存"（materials.stock_qty），
     * 期初则由 当前库存 - 区间入库 + 区间出库 倒推得到。
     */
    public List<LedgerRow> ledger(String fromDate, String toDate) {
        List<LedgerRow> rows = new ArrayList<>();
        for (FlowRow f : flowByMaterial(fromDate, toDate)) {
            LedgerRow r = new LedgerRow();
            r.code = f.code;
            r.name = f.name;
            r.unit = f.unit;
            r.inQty = f.inQty;
            r.outQty = f.outQty;
            r.closing = f.stock;
            r.opening = f.stock - f.inQty + f.outQty;
            rows.add(r);
        }
        return rows;
    }
}
