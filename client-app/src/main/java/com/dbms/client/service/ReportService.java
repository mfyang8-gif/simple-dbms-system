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

    /** 按物料聚合进出仓流量 + 当前库存。 */
    public List<FlowRow> flowByMaterial() {
        Map<Integer, Material> materials = new LinkedHashMap<>();
        for (Material m : materialDao.search("")) {
            materials.put(m.getMaterialCode(), m);
        }

        Map<Integer, int[]> agg = new HashMap<>();      // code -> [进仓合计, 出仓合计]
        for (InventoryLog log : logDao.listAll()) {
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

    /** 指定年份 12 个月的进出仓合计：[i][0]=进仓，[i][1]=出仓。 */
    public int[][] monthlyFlow(int year) {
        int[][] result = new int[12][2];
        String prefix = year + "-";
        for (InventoryLog log : logDao.listAll()) {
            String d = log.getOpDate();
            if (d == null || !d.startsWith(prefix) || d.length() < 7) {
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

    /** 指定年月的进出仓单明细，按日期 + 单号排序。 */
    public List<Object[]> monthBill(int year, int month) {
        String prefix = String.format("%04d-%02d", year, month);
        Map<Integer, Material> materialMap = new HashMap<>();
        for (Material m : materialDao.search("")) {
            materialMap.put(m.getMaterialCode(), m);
        }

        List<InventoryLog> logs = new ArrayList<>();
        for (InventoryLog log : logDao.listAll()) {
            if (log.getOpDate() != null && log.getOpDate().startsWith(prefix)) {
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

    /** 物料仓库账本：期初 + 入库 - 出库 = 结存（期初由当前库存倒推）。 */
    public List<LedgerRow> ledger() {
        List<LedgerRow> rows = new ArrayList<>();
        for (FlowRow f : flowByMaterial()) {
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
