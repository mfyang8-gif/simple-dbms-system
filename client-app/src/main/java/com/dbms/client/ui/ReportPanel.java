package com.dbms.client.ui;

import com.dbms.client.report.Exporters;
import com.dbms.client.service.ReportService;
import org.jfree.chart.ChartFactory;
import org.jfree.chart.ChartPanel;
import org.jfree.chart.JFreeChart;
import org.jfree.chart.StandardChartTheme;
import org.jfree.data.category.DefaultCategoryDataset;
import org.jfree.data.general.DefaultPieDataset;

import javax.swing.*;
import javax.swing.table.DefaultTableModel;
import java.awt.*;
import java.io.File;
import java.time.LocalDate;
import java.time.LocalDateTime;
import java.time.format.DateTimeFormatter;
import java.util.ArrayList;
import java.util.List;

/**
 * 统计与报表（子系统五）：图形展示 + 月度单据 + 账本 + PDF/Excel 导出。
 *
 * 【第 1 课】工具栏新增日期区间输入框，按时间段统计。
 * 【第 2 课】新增"图表类型"下拉框，可切换柱状图 / 折线图 / 饼图。
 *           知识点：同一份数据喂给不同的 ChartFactory 方法就得到不同图形，
 *           因此每个报表同时准备 CategoryDataset（柱状/折线）与 PieDataset（饼图）。
 * 【第 8 课】新增"按操作人统计"报表：流水表按 operator_id 聚合，再关联人员表取姓名；
 *           同时把月份/图表两个下拉框的可用状态跟统计类型绑起来（明细单据不画图）。
 * 【第 9 课】新增"库存预警"报表：库存低于界面阈值的物料单列，缺货与偏低分级；
 *           阈值放界面而不入库，因为字典规定 materials 字段不可改。
 */
public class ReportPanel extends JPanel {

    private final ReportService service = new ReportService();

    private final JComboBox<String> typeBox = new JComboBox<>(new String[]{
            "按物料流量统计", "月度进出仓趋势", "月度进出仓单", "物料仓库账本",
            "按操作人统计", "库存预警"});

    /**
     * 库存预警阈值。
     * materials 表没有安全库存字段（字典规定字段不可改），
     * 所以阈值放在界面上，由用户现场调整。
     */
    private final JSpinner thresholdSpinner =
            new JSpinner(new SpinnerNumberModel(50, 1, 99999, 10));

    /** 第 2 课新增：图表类型。 */
    private final JComboBox<String> chartBox = new JComboBox<>(new String[]{
            "柱状图", "折线图", "饼图"});

    private final JSpinner yearSpinner =
            new JSpinner(new SpinnerNumberModel(LocalDate.now().getYear(), 2000, 2100, 1));

    private final JComboBox<String> monthBox = new JComboBox<>(new String[]{
            "01", "02", "03", "04", "05", "06", "07", "08", "09", "10", "11", "12"});

    // 第 1 课新增：日期区间输入
    private final JTextField fromField = new JTextField("2025-01-01", 9);
    private final JTextField toField = new JTextField("2025-12-31", 9);

    private final DefaultTableModel model = new DefaultTableModel() {
        @Override
        public boolean isCellEditable(int row, int column) {
            return false;
        }
    };
    private final JTable table = new JTable(model);
    private final JPanel chartHolder = new JPanel(new BorderLayout());

    private String[] currentHeaders = new String[0];
    private List<Object[]> currentRows = new ArrayList<>();
    private JFreeChart currentChart;
    private String currentTitle = "报表";

    public ReportPanel() {
        setupChartFont();

        setLayout(new BorderLayout(8, 8));
        setBorder(BorderFactory.createEmptyBorder(10, 10, 10, 10));

        add(buildToolbar(), BorderLayout.NORTH);

        chartHolder.setPreferredSize(new Dimension(600, 300));
        chartHolder.setBorder(BorderFactory.createTitledBorder("图形展示"));
        JSplitPane split = new JSplitPane(JSplitPane.VERTICAL_SPLIT, chartHolder, new JScrollPane(table));
        split.setResizeWeight(0.55);
        add(split, BorderLayout.CENTER);

        updateControlEnabled();
        doQuery();
    }

    /**
     * 修复 JFreeChart 中文乱码：给它换一套含中文的字体主题。
     * 必须在创建任何图表之前调用（全局生效，调用一次即可）。
     */
    private static void setupChartFont() {
        Font font = new Font("微软雅黑", Font.PLAIN, 13);
        StandardChartTheme theme = new StandardChartTheme("CN");
        theme.setExtraLargeFont(font.deriveFont(Font.BOLD, 16f));   // 图表标题
        theme.setLargeFont(font.deriveFont(13f));                   // 坐标轴标题
        theme.setRegularFont(font);                                 // 刻度、图例
        theme.setSmallFont(font.deriveFont(11f));
        ChartFactory.setChartTheme(theme);
    }

    private JComponent buildToolbar() {
        // 两行布局：第一行 = 查询条件；第二行 = 图表类型 + 生成 + 导出
        // （一行放不下这么多控件，会被挤出屏幕右边）
        JPanel rows = new JPanel();
        rows.setLayout(new BoxLayout(rows, BoxLayout.Y_AXIS));
        rows.setBorder(BorderFactory.createTitledBorder("报表参数"));

        // ---- 第一行：日期 + 统计类型 + 年月 ----
        JPanel conditions = new JPanel(new FlowLayout(FlowLayout.LEFT, 8, 2));

        conditions.add(new JLabel("起始日期："));
        conditions.add(fromField);
        conditions.add(new JLabel("结束日期："));
        conditions.add(toField);
        fromField.setToolTipText("格式 2025-09-01，留空表示不限");
        toField.setToolTipText("格式 2025-12-31，留空表示不限");
        // 文本框不会因为"打字"触发事件；绑上回车键 = 在框里按 Enter 立即查询
        fromField.addActionListener(e -> doQuery());
        toField.addActionListener(e -> doQuery());

        conditions.add(new JLabel("统计类型："));
        // 切换统计类型时：① 更新月份/图表的可用状态 ② 立即重新查询并重绘
        typeBox.addActionListener(e -> {
            updateControlEnabled();
            doQuery();
        });
        conditions.add(typeBox);

        conditions.add(new JLabel("年份："));
        yearSpinner.addChangeListener(e -> doQuery());
        conditions.add(yearSpinner);

        conditions.add(new JLabel("月份："));
        monthBox.addActionListener(e -> doQuery());
        conditions.add(monthBox);

        // ---- 第二行：预警阈值 + 图表类型 + 按钮 ----
        JPanel actions = new JPanel(new FlowLayout(FlowLayout.LEFT, 8, 2));

        actions.add(new JLabel("预警阈值："));
        thresholdSpinner.setToolTipText("当前库存低于该值的物料会列入预警（仅库存预警报表使用）");
        // 只有当前正在看库存预警时才需要重查，切到别的报表时改阈值不该触发查询
        thresholdSpinner.addChangeListener(e -> {
            if ("库存预警".equals(typeBox.getSelectedItem())) {
                doQuery();
            }
        });
        actions.add(thresholdSpinner);

        actions.add(new JLabel("图表："));
        chartBox.addActionListener(e -> doQuery());
        actions.add(chartBox);

        JButton queryButton = new JButton("生成报表");
        queryButton.addActionListener(e -> doQuery());
        actions.add(queryButton);

        actions.add(new JLabel("   "));

        JButton excelButton = new JButton("导出 Excel");
        excelButton.addActionListener(e -> exportExcel());
        actions.add(excelButton);

        JButton pdfButton = new JButton("导出 PDF");
        pdfButton.addActionListener(e -> exportPdf());
        actions.add(pdfButton);

        JButton pngButton = new JButton("导出图表 PNG");
        pngButton.addActionListener(e -> exportChartPng());
        actions.add(pngButton);

        // 第 3 课：打印单据（Swing 自带，不需要第三方库）
        JButton printButton = new JButton("打印单据");
        printButton.addActionListener(e -> printBill());
        actions.add(printButton);

        rows.add(conditions);
        rows.add(actions);
        return rows;
    }

    /**
     * 控件联动：月份只对"月度进出仓单"有意义；图表类型对明细单据无意义；
     * 预警阈值只对"库存预警"有意义——用不到就禁掉，别留"点了没反应"的控件。
     */
    private void updateControlEnabled() {
        String type = (String) typeBox.getSelectedItem();
        boolean isBill = "月度进出仓单".equals(type);
        monthBox.setEnabled(isBill);
        chartBox.setEnabled(!isBill);
        thresholdSpinner.setEnabled("库存预警".equals(type));
    }

    /** 空串放行（表示不限），否则必须是 yyyy-MM-dd。 */
    private boolean checkDate(String s) {
        return s.isEmpty() || s.matches("^\\d{4}-\\d{2}-\\d{2}$");
    }

    // ---------------- 生成报表 ----------------

    private void doQuery() {
        String from = fromField.getText().trim();
        String to = toField.getText().trim();
        if (!checkDate(from) || !checkDate(to)) {
            JOptionPane.showMessageDialog(this,
                    "日期格式应为 yyyy-MM-dd，例如 2025-09-01（留空表示不限）",
                    "格式错误", JOptionPane.WARNING_MESSAGE);
            return;
        }
        if (!from.isEmpty() && !to.isEmpty() && from.compareTo(to) > 0) {
            JOptionPane.showMessageDialog(this,
                    "起始日期不能晚于结束日期", "格式错误", JOptionPane.WARNING_MESSAGE);
            return;
        }

        int type = typeBox.getSelectedIndex();
        int year = (Integer) yearSpinner.getValue();
        int month = monthBox.getSelectedIndex() + 1;
        int threshold = (Integer) thresholdSpinner.getValue();
        try {
            switch (type) {
                case 0 -> loadFlow(from, to);
                case 1 -> loadMonthly(year, from, to);
                case 2 -> loadMonthBill(year, month, from, to);
                case 3 -> loadLedger(from, to);
                case 4 -> loadPerson(from, to);
                default -> loadStock(threshold);
            }
        } catch (Exception ex) {
            showError(ex);
        }
    }

    private void loadFlow(String from, String to) {
        List<ReportService.FlowRow> list = service.flowByMaterial(from, to);
        List<Object[]> rows = new ArrayList<>();
        DefaultCategoryDataset bar = new DefaultCategoryDataset();
        DefaultPieDataset<String> pie = new DefaultPieDataset<>();
        for (ReportService.FlowRow f : list) {
            rows.add(new Object[]{f.code, f.name, f.unit, f.inQty, f.outQty, f.stock});
            bar.addValue(f.inQty, "进仓", f.name);
            bar.addValue(f.outQty, "出仓", f.name);
            if (f.inQty > 0) {
                pie.setValue(f.name + " 进仓", f.inQty);
            }
        }
        currentTitle = "按物料进出仓流量对比";
        apply(ReportService.FLOW_HEADERS, rows,
                makeChart(currentTitle, "物料", "数量", bar, pie));
    }

    private void loadMonthly(int year, String from, String to) {
        int[][] monthly = service.monthlyFlow(year, from, to);
        List<Object[]> rows = new ArrayList<>();
        DefaultCategoryDataset bar = new DefaultCategoryDataset();
        DefaultPieDataset<String> pie = new DefaultPieDataset<>();
        for (int i = 0; i < 12; i++) {
            rows.add(new Object[]{(i + 1) + "月", monthly[i][0], monthly[i][1]});
            bar.addValue(monthly[i][0], "进仓", (i + 1) + "月");
            bar.addValue(monthly[i][1], "出仓", (i + 1) + "月");
            if (monthly[i][0] > 0) {
                pie.setValue((i + 1) + "月进仓", monthly[i][0]);
            }
        }
        currentTitle = year + " 年月度进出仓趋势";
        apply(new String[]{"月份", "进仓合计", "出仓合计"}, rows,
                makeChart(currentTitle, "月份", "数量", bar, pie));
    }

    /**
     * 第 3 课：月度进出仓单 —— 明细 + 合计行 + 单据抬头。
     * 这块不是"图表"，所以要把 chartHolder 里的内容换成抬头面板。
     */
    private void loadMonthBill(int year, int month, String from, String to) {
        List<Object[]> detail = (from.isEmpty() && to.isEmpty())
                ? service.monthBill(year, month)
                : service.monthBillByRange(from, to);

        // ① 算合计：把明细里的数量按"进仓/出仓"分别累加
        int inTotal = 0;
        int outTotal = 0;
        for (Object[] r : detail) {
            String kind = String.valueOf(r[4]);            // 第 5 列是"类型"
            int qty = ((Number) r[5]).intValue();          // 第 6 列是"数量"
            if (kind.startsWith("进")) {
                inTotal += qty;
            } else {
                outTotal += qty;
            }
        }

        // ② 明细后面追加合计行（就是往表格模型里多塞两行数据）
        List<Object[]> rows = new ArrayList<>(detail);
        if (!detail.isEmpty()) {
            rows.add(new Object[]{"合计", "", "", "", "进仓", inTotal, "", "", detail.size() + " 笔"});
            rows.add(new Object[]{"", "", "", "", "出仓", outTotal, "", "", ""});
        }

        String period = (!from.isEmpty() || !to.isEmpty())
                ? (from.isEmpty() ? "最早" : from) + " ~ " + (to.isEmpty() ? "最新" : to)
                : String.format("%04d-%02d", year, month);
        currentTitle = period + " 进出仓单";

        currentHeaders = ReportService.MONTH_BILL_HEADERS;
        currentRows = rows;
        currentChart = null;
        model.setDataVector(rows.toArray(new Object[0][]), ReportService.MONTH_BILL_HEADERS);

        // ③ 把 chartHolder 换成单据抬头
        showBillHeader(period, detail.size(), inTotal, outTotal);
    }

    /** 在原本放图表的位置显示"单据抬头"。 */
    private void showBillHeader(String period, int count, int inTotal, int outTotal) {
        JPanel header = new JPanel();
        header.setLayout(new BoxLayout(header, BoxLayout.Y_AXIS));
        header.setBorder(BorderFactory.createEmptyBorder(14, 20, 14, 20));

        JLabel title = new JLabel("简易仓库管理系统 —— 物料进出仓单");
        title.setFont(new Font("微软雅黑", Font.BOLD, 20));
        title.setAlignmentX(Component.CENTER_ALIGNMENT);

        JLabel periodLabel = new JLabel("统计期间：" + period);
        periodLabel.setFont(new Font("微软雅黑", Font.PLAIN, 13));
        periodLabel.setAlignmentX(Component.CENTER_ALIGNMENT);

        JLabel timeLabel = new JLabel("制表时间："
                + LocalDateTime.now().format(DateTimeFormatter.ofPattern("yyyy-MM-dd HH:mm")));
        timeLabel.setFont(new Font("微软雅黑", Font.PLAIN, 13));
        timeLabel.setAlignmentX(Component.CENTER_ALIGNMENT);

        JLabel sumLabel = new JLabel(String.format(
                "共 %d 笔　进仓合计：%d　出仓合计：%d", count, inTotal, outTotal));
        sumLabel.setFont(new Font("微软雅黑", Font.BOLD, 14));
        sumLabel.setAlignmentX(Component.CENTER_ALIGNMENT);

        header.add(title);
        header.add(Box.createVerticalStrut(8));
        header.add(periodLabel);
        header.add(Box.createVerticalStrut(4));
        header.add(timeLabel);
        header.add(Box.createVerticalStrut(8));
        header.add(sumLabel);

        chartHolder.removeAll();
        chartHolder.add(header, BorderLayout.CENTER);
        chartHolder.revalidate();
        chartHolder.repaint();
    }

    /** 第 3 课：打印当前表格（弹出系统打印对话框，可预览、可选打印机）。 */
    private void printBill() {
        if (currentRows.isEmpty()) {
            warn("当前没有数据可打印");
            return;
        }
        try {
            boolean done = table.print(JTable.PrintMode.FIT_WIDTH,
                    new java.text.MessageFormat(currentTitle),
                    new java.text.MessageFormat("第 {0} 页 / 共 {1} 页"));
            if (done) {
                warn("已提交打印任务");
            }
        } catch (Exception ex) {
            showError(ex);
        }
    }

    private void loadLedger(String from, String to) {
        List<ReportService.LedgerRow> list = service.ledger(from, to);
        List<Object[]> rows = new ArrayList<>();
        DefaultCategoryDataset bar = new DefaultCategoryDataset();
        DefaultPieDataset<String> pie = new DefaultPieDataset<>();
        for (ReportService.LedgerRow r : list) {
            rows.add(new Object[]{r.code, r.name, r.unit, r.opening, r.inQty, r.outQty, r.closing});
            bar.addValue(r.inQty, "入库", r.name);
            bar.addValue(r.outQty, "出库", r.name);
            if (r.closing > 0) {
                pie.setValue(r.name, r.closing);
            }
        }
        currentTitle = "物料仓库账本";
        apply(ReportService.LEDGER_HEADERS, rows,
                makeChart(currentTitle, "物料", "数量", bar, pie));
    }

    /**
     * 第 8 课新增：按操作人统计工作量。
     *
     * inventory_logs.operator_id 指向 personnel.id，
     * 所以这是一张"单事实表 + 维表"的典型报表：
     * 单据数、进出仓量来自流水表，姓名来自人员表。
     */
    private void loadPerson(String from, String to) {
        List<ReportService.PersonRow> list = service.flowByOperator(from, to);
        List<Object[]> rows = new ArrayList<>();
        DefaultCategoryDataset bar = new DefaultCategoryDataset();
        DefaultPieDataset<String> pie = new DefaultPieDataset<>();
        for (ReportService.PersonRow r : list) {
            rows.add(new Object[]{r.operatorId, r.name, r.billCount,
                    r.inQty, r.outQty, r.total, r.materialKinds});
            // 图表里带上工号：万一有人重名，两个柱子/扇区不会互相覆盖
            String label = r.name + "(" + r.operatorId + ")";
            bar.addValue(r.inQty, "进仓", label);
            bar.addValue(r.outQty, "出仓", label);
            if (r.total > 0) {
                pie.setValue(label, r.total);
            }
        }
        currentTitle = "按操作人进出仓工作量";
        apply(ReportService.PERSON_HEADERS, rows,
                makeChart(currentTitle, "操作人", "数量", bar, pie));
    }

    /**
     * 第 9 课新增：库存预警。
     *
     * 数据只来自 materials 一张表（当前库存是物料档案上的存量字段），
     * 判定规则：库存 &lt; 阈值。库存为 0 单独标"缺货"，其余标"偏低"。
     * 阈值来自界面上的数字框，改一下立刻重新筛，不用动数据库。
     */
    private void loadStock(int threshold) {
        List<ReportService.StockRow> list = service.safetyStock(threshold);
        List<Object[]> rows = new ArrayList<>();
        DefaultCategoryDataset bar = new DefaultCategoryDataset();
        DefaultPieDataset<String> pie = new DefaultPieDataset<>();
        for (ReportService.StockRow r : list) {
            rows.add(new Object[]{r.code, r.name, r.spec, r.unit, r.stock, r.gap, r.level});
            // 图表标签带物料代码：不同规格可能同名，靠名字做 key 会互相覆盖
            String label = r.name + "(" + r.code + ")";
            bar.addValue(r.stock, "当前库存", label);
            if (r.stock > 0) {
                pie.setValue(label, r.stock);       // 库存为 0 在饼图上没有意义
            }
        }
        currentTitle = "库存预警（低于 " + threshold + "）";

        if (list.isEmpty()) {
            // 没有预警项：表格清空，图形位置直接说明"库存正常"，
            // 避免一片空白让人以为报表出错了
            currentHeaders = ReportService.STOCK_HEADERS;
            currentRows = rows;
            currentChart = null;
            model.setDataVector(rows.toArray(new Object[0][]), ReportService.STOCK_HEADERS);
            showMessage("当前没有库存低于 " + threshold + " 的物料，库存正常。");
            return;
        }

        apply(ReportService.STOCK_HEADERS, rows,
                makeChart(currentTitle, "物料", "当前库存", bar, pie));
    }

    /**
     * 第 2 课核心：按"图表类型"下拉框的选择生成图形。
     *
     * @param bar 柱状图 / 折线图用的数据源（有横轴概念）
     * @param pie 饼图用的数据源（只有"名称→数值"）；null 表示该报表不支持饼图
     * @return 生成的图表；null 表示"当前类型下不画图"
     */
    private JFreeChart makeChart(String title, String catAxis, String valAxis,
                                 DefaultCategoryDataset bar, DefaultPieDataset<String> pie) {
        String kind = (String) chartBox.getSelectedItem();
        if ("饼图".equals(kind)) {
            if (pie == null || pie.getKeys().isEmpty()) {
                return null;                       // 没数据就别硬画
            }
            return ChartFactory.createPieChart(title, pie);
        }
        if ("折线图".equals(kind)) {
            return ChartFactory.createLineChart(title, catAxis, valAxis, bar);
        }
        return ChartFactory.createBarChart(title, catAxis, valAxis, bar);
    }

    private void apply(String[] headers, List<Object[]> rows, JFreeChart chart) {
        currentHeaders = headers;
        currentRows = rows;
        currentChart = chart;
        model.setDataVector(rows.toArray(new Object[0][]), headers);
        showChart(chart);
    }

    private void showChart(JFreeChart chart) {
        if (chart == null) {
            showMessage("该报表当前类型下无图形（或数据为空）。");
            return;
        }
        chartHolder.removeAll();
        chartHolder.add(new ChartPanel(chart), BorderLayout.CENTER);
        chartHolder.revalidate();
        chartHolder.repaint();
    }

    /** 在原本放图表的位置显示一句提示（无数据、无需预警等场景）。 */
    private void showMessage(String text) {
        chartHolder.removeAll();
        JLabel label = new JLabel(text, SwingConstants.CENTER);
        label.setFont(new Font("微软雅黑", Font.PLAIN, 15));
        chartHolder.add(label, BorderLayout.CENTER);
        chartHolder.revalidate();
        chartHolder.repaint();
    }

    // ---------------- 导出 ----------------

    private void exportExcel() {
        if (currentRows.isEmpty()) {
            warn("当前没有数据可导出");
            return;
        }
        File file = chooseFile(currentTitle + ".xlsx");
        if (file == null) {
            return;
        }
        try {
            Exporters.exportExcel(file, currentTitle, currentHeaders, currentRows);
            JOptionPane.showMessageDialog(this, "已导出：\n" + file.getAbsolutePath());
        } catch (Exception ex) {
            showError(ex);
        }
    }

    private void exportPdf() {
        if (currentRows.isEmpty()) {
            warn("当前没有数据可导出");
            return;
        }
        File file = chooseFile(currentTitle + ".pdf");
        if (file == null) {
            return;
        }
        try {
            Exporters.exportPdf(file, currentTitle, currentHeaders, currentRows);
            JOptionPane.showMessageDialog(this, "已导出：\n" + file.getAbsolutePath());
        } catch (Exception ex) {
            showError(ex);
        }
    }

    private void exportChartPng() {
        if (currentChart == null) {
            warn("当前报表没有图形可导出");
            return;
        }
        File file = chooseFile(currentTitle + ".png");
        if (file == null) {
            return;
        }
        try {
            Exporters.exportChartPng(file, currentChart, 1000, 600);
            JOptionPane.showMessageDialog(this, "已导出：\n" + file.getAbsolutePath());
        } catch (Exception ex) {
            showError(ex);
        }
    }

    private File chooseFile(String defaultName) {
        JFileChooser chooser = new JFileChooser();
        chooser.setSelectedFile(new File(defaultName));
        return chooser.showSaveDialog(this) == JFileChooser.APPROVE_OPTION ? chooser.getSelectedFile() : null;
    }

    private void warn(String message) {
        JOptionPane.showMessageDialog(this, message, "提示", JOptionPane.INFORMATION_MESSAGE);
    }

    private void showError(Exception ex) {
        JOptionPane.showMessageDialog(this, ex.getMessage(), "操作失败", JOptionPane.ERROR_MESSAGE);
    }
}
