package com.dbms.client.ui;

import com.dbms.client.report.Exporters;
import com.dbms.client.service.ReportService;
import org.jfree.chart.ChartFactory;
import org.jfree.chart.ChartPanel;
import org.jfree.chart.JFreeChart;
import org.jfree.data.category.DefaultCategoryDataset;
import org.jfree.data.general.DefaultPieDataset;

import javax.swing.*;
import javax.swing.table.DefaultTableModel;
import java.awt.*;
import java.io.File;
import java.time.LocalDate;
import java.util.ArrayList;
import java.util.List;

/** 统计与报表（子系统五）：图形展示 + 月度单据 + 账本 + PDF/Excel 导出。 */
public class ReportPanel extends JPanel {

    private final ReportService service = new ReportService();

    private final JComboBox<String> typeBox = new JComboBox<>(new String[]{
            "按物料流量统计", "月度进出仓趋势", "月度进出仓单", "物料仓库账本"});

    private final JSpinner yearSpinner =
            new JSpinner(new SpinnerNumberModel(LocalDate.now().getYear(), 2000, 2100, 1));

    private final JComboBox<String> monthBox = new JComboBox<>(new String[]{
            "01", "02", "03", "04", "05", "06", "07", "08", "09", "10", "11", "12"});

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
        setLayout(new BorderLayout(8, 8));
        setBorder(BorderFactory.createEmptyBorder(10, 10, 10, 10));

        add(buildToolbar(), BorderLayout.NORTH);

        chartHolder.setPreferredSize(new Dimension(600, 300));
        chartHolder.setBorder(BorderFactory.createTitledBorder("图形展示"));
        JSplitPane split = new JSplitPane(JSplitPane.VERTICAL_SPLIT, chartHolder, new JScrollPane(table));
        split.setResizeWeight(0.55);
        add(split, BorderLayout.CENTER);

        updateMonthEnabled();
        doQuery();
    }

    private JComponent buildToolbar() {
        JPanel panel = new JPanel(new FlowLayout(FlowLayout.LEFT, 8, 4));
        panel.setBorder(BorderFactory.createTitledBorder("报表参数"));

        panel.add(new JLabel("统计类型："));
        typeBox.addActionListener(e -> updateMonthEnabled());
        panel.add(typeBox);

        panel.add(new JLabel("年份："));
        panel.add(yearSpinner);

        panel.add(new JLabel("月份："));
        panel.add(monthBox);

        JButton queryButton = new JButton("生成报表");
        queryButton.addActionListener(e -> doQuery());
        panel.add(queryButton);

        panel.add(new JLabel("   "));

        JButton excelButton = new JButton("导出 Excel");
        excelButton.addActionListener(e -> exportExcel());
        panel.add(excelButton);

        JButton pdfButton = new JButton("导出 PDF");
        pdfButton.addActionListener(e -> exportPdf());
        panel.add(pdfButton);

        JButton pngButton = new JButton("导出图表 PNG");
        pngButton.addActionListener(e -> exportChartPng());
        panel.add(pngButton);

        return panel;
    }

    private void updateMonthEnabled() {
        monthBox.setEnabled(typeBox.getSelectedIndex() == 2);
    }

    // ---------------- 生成报表 ----------------

    private void doQuery() {
        int type = typeBox.getSelectedIndex();
        int year = (Integer) yearSpinner.getValue();
        int month = monthBox.getSelectedIndex() + 1;
        try {
            switch (type) {
                case 0 -> loadFlow();
                case 1 -> loadMonthly(year);
                case 2 -> loadMonthBill(year, month);
                default -> loadLedger();
            }
        } catch (Exception ex) {
            showError(ex);
        }
    }

    private void loadFlow() {
        List<ReportService.FlowRow> list = service.flowByMaterial();
        List<Object[]> rows = new ArrayList<>();
        DefaultCategoryDataset dataset = new DefaultCategoryDataset();
        for (ReportService.FlowRow f : list) {
            rows.add(new Object[]{f.code, f.name, f.unit, f.inQty, f.outQty, f.stock});
            dataset.addValue(f.inQty, "进仓", f.name);
            dataset.addValue(f.outQty, "出仓", f.name);
        }
        currentTitle = "按物料进出仓流量对比";
        apply(ReportService.FLOW_HEADERS, rows,
                ChartFactory.createBarChart(currentTitle, "物料", "数量", dataset));
    }

    private void loadMonthly(int year) {
        int[][] monthly = service.monthlyFlow(year);
        List<Object[]> rows = new ArrayList<>();
        DefaultCategoryDataset dataset = new DefaultCategoryDataset();
        for (int i = 0; i < 12; i++) {
            rows.add(new Object[]{(i + 1) + "月", monthly[i][0], monthly[i][1]});
            dataset.addValue(monthly[i][0], "进仓", (i + 1) + "月");
            dataset.addValue(monthly[i][1], "出仓", (i + 1) + "月");
        }
        currentTitle = year + " 年月度进出仓趋势";
        apply(new String[]{"月份", "进仓合计", "出仓合计"}, rows,
                ChartFactory.createBarChart(currentTitle, "月份", "数量", dataset));
    }

    private void loadMonthBill(int year, int month) {
        List<Object[]> rows = service.monthBill(year, month);
        currentTitle = String.format("%04d-%02d 月度进出仓单", year, month);
        apply(ReportService.MONTH_BILL_HEADERS, rows, null);
    }

    private void loadLedger() {
        List<ReportService.LedgerRow> list = service.ledger();
        List<Object[]> rows = new ArrayList<>();
        DefaultPieDataset<String> pie = new DefaultPieDataset<>();
        int total = 0;
        for (ReportService.LedgerRow r : list) {
            rows.add(new Object[]{r.code, r.name, r.unit, r.opening, r.inQty, r.outQty, r.closing});
            if (r.closing > 0) {
                pie.setValue(r.name, r.closing);
                total += r.closing;
            }
        }
        currentTitle = "物料仓库账本";
        apply(ReportService.LEDGER_HEADERS, rows,
                total > 0 ? ChartFactory.createPieChart("当前库存占比", pie) : null);
    }

    private void apply(String[] headers, List<Object[]> rows, JFreeChart chart) {
        currentHeaders = headers;
        currentRows = rows;
        currentChart = chart;
        model.setDataVector(rows.toArray(new Object[0][]), headers);
        showChart(chart);
    }

    private void showChart(JFreeChart chart) {
        chartHolder.removeAll();
        if (chart != null) {
            chartHolder.add(new ChartPanel(chart), BorderLayout.CENTER);
        } else {
            chartHolder.add(new JLabel("该报表以表格形式呈现，无图形。", SwingConstants.CENTER), BorderLayout.CENTER);
        }
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
