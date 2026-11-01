package com.dbms.client.console;

import javax.swing.*;
import javax.swing.table.AbstractTableModel;
import javax.swing.table.DefaultTableCellRenderer;
import javax.swing.table.TableColumn;
import java.awt.*;
import java.awt.datatransfer.StringSelection;
import java.io.BufferedWriter;
import java.io.File;
import java.io.FileOutputStream;
import java.io.OutputStreamWriter;
import java.io.Writer;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.List;

/**
 * 结果集展示与导出。
 *
 * 两个刻意的设计取舍：
 *  1. SQL NULL 与空字符串在表格里必须区分——NULL 渲染成灰色斜体 "NULL"，
 *     空串渲染成空白。否则演示时无法说明协议里 {@code \N} 的处理是否正确。
 *  2. 表格最多渲染 {@link #DISPLAY_LIMIT} 行。引擎没有 LIMIT 语法，
 *     SELECT * 可能一次拉回整表；界面卡死的锅不该由引擎背。
 *     超出部分在页脚明确提示，并可导出 CSV 全量查看。
 */
public class ResultTablePanel extends JPanel {

    /** 界面渲染上限（导出不受限）。 */
    static final int DISPLAY_LIMIT = 5000;

    private final ResultModel model = new ResultModel();
    private final JTable table = new JTable(model);
    private final JLabel footer = new JLabel(" ");
    private ConsoleSession.Exec lastExec;

    public ResultTablePanel() {
        super(new BorderLayout());
        table.setAutoResizeMode(JTable.AUTO_RESIZE_OFF);
        table.setRowHeight(24);
        table.setFillsViewportHeight(true);
        table.setCellSelectionEnabled(true);
        table.getTableHeader().setReorderingAllowed(false);
        table.setDefaultRenderer(Object.class, new CellRenderer());

        footer.setBorder(BorderFactory.createEmptyBorder(4, 8, 4, 8));
        footer.setFont(footer.getFont().deriveFont(Font.PLAIN, 12f));
        footer.setText("尚未执行任何语句");

        add(new JScrollPane(table), BorderLayout.CENTER);
        add(footer, BorderLayout.SOUTH);
    }

    /** 展示一次执行结果；非结果集语句显示一行提示。 */
    public void showResult(ConsoleSession.Exec exec) {
        this.lastExec = exec;
        if (exec == null) {
            clear();
            return;
        }
        if (exec.isError()) {
            model.setData(List.of("错误"), List.of(List.of(exec.error)));
            footer.setText("执行失败 · " + exec.elapsedMs + " ms · SQL: " + abbreviate(exec.sql));
            return;
        }
        if (exec.kind == ConsoleSession.Exec.Kind.OK) {
            model.setData(List.of("结果"), List.of(List.of("OK，" + exec.affected + " 行受影响")));
            footer.setText("执行成功 · 影响 " + exec.affected + " 行 · " + exec.elapsedMs + " ms");
            return;
        }
        if (exec.kind == ConsoleSession.Exec.Kind.RAW) {
            model.setData(List.of("未识别的响应"), List.of(List.of(exec.raw)));
            footer.setText("响应格式未识别 · " + exec.elapsedMs + " ms（原文见上）");
            return;
        }

        int total = exec.rows.size();
        List<List<String>> view = total > DISPLAY_LIMIT ? exec.rows.subList(0, DISPLAY_LIMIT) : exec.rows;
        model.setData(exec.columns, new ArrayList<>(view));
        applyColumnWidths(exec.columns, view);

        String note = total > DISPLAY_LIMIT
                ? " · 仅显示前 " + DISPLAY_LIMIT + " 行，共 " + total + " 行（可导出 CSV 查看全部）"
                : "";
        footer.setText(total + " 行 · " + exec.columns.size() + " 列 · " + exec.elapsedMs + " ms" + note);
    }

    public void clear() {
        lastExec = null;
        model.setData(List.of(), List.of());
        footer.setText(" ");
    }

    /** 当前结果是否可导出（有列即认为可导出）。 */
    public boolean hasExportableResult() {
        return lastExec != null && lastExec.kind == ConsoleSession.Exec.Kind.ROWS && !lastExec.columns.isEmpty();
    }

    public void copySelectionToClipboard(Component parent) {
        int[] rows = table.getSelectedRows();
        int[] cols = table.getSelectedColumns();
        if (rows.length == 0 || cols.length == 0) {
            JOptionPane.showMessageDialog(parent, "请先在结果表格中选择单元格区域。", "提示",
                    JOptionPane.INFORMATION_MESSAGE);
            return;
        }
        StringBuilder sb = new StringBuilder();
        for (int r : rows) {
            for (int i = 0; i < cols.length; i++) {
                if (i > 0) {
                    sb.append('\t');
                }
                Object v = table.getValueAt(r, cols[i]);
                sb.append(v == null ? "" : v.toString());
            }
            sb.append('\n');
        }
        Toolkit.getDefaultToolkit().getSystemClipboard().setContents(new StringSelection(sb.toString()), null);
        footer.setText("已复制 " + rows.length + " 行 × " + cols.length + " 列到剪贴板");
    }

    /** 导出当前结果为 CSV（UTF-8 带 BOM，Excel 直接双击不乱码）。 */
    public void exportCsv(Component parent) {
        if (lastExec == null || lastExec.columns.isEmpty()) {
            JOptionPane.showMessageDialog(parent, "当前没有可导出的结果集。", "提示",
                    JOptionPane.INFORMATION_MESSAGE);
            return;
        }
        JFileChooser chooser = new JFileChooser();
        chooser.setDialogTitle("导出结果集为 CSV");
        chooser.setSelectedFile(new File("result-" + System.currentTimeMillis() + ".csv"));
        if (chooser.showSaveDialog(parent) != JFileChooser.APPROVE_OPTION) {
            return;
        }
        File file = chooser.getSelectedFile();
        try (Writer w = new BufferedWriter(new OutputStreamWriter(
                new FileOutputStream(file), StandardCharsets.UTF_8))) {
            w.write('\uFEFF');
            writeCsvRow(w, lastExec.columns);
            for (List<String> row : lastExec.rows) {
                writeCsvRow(w, row);
            }
        } catch (Exception e) {
            JOptionPane.showMessageDialog(parent, "导出失败：" + e.getMessage(), "错误",
                    JOptionPane.ERROR_MESSAGE);
            return;
        }
        footer.setText("已导出 " + lastExec.rows.size() + " 行到 " + file.getAbsolutePath());
    }

    private void writeCsvRow(Writer w, List<String> cells) throws Exception {
        for (int i = 0; i < cells.size(); i++) {
            if (i > 0) {
                w.write(',');
            }
            String v = cells.get(i);
            if (v == null) {
                w.write("");
                continue;
            }
            boolean needQuote = v.contains(",") || v.contains("\"") || v.contains("\n") || v.contains("\r");
            if (needQuote) {
                w.write('"');
                w.write(v.replace("\"", "\"\""));
                w.write('"');
            } else {
                w.write(v);
            }
        }
        w.write("\r\n");
    }

    private void applyColumnWidths(List<String> columns, List<List<String>> rows) {
        for (int c = 0; c < columns.size() && c < table.getColumnCount(); c++) {
            TableColumn column = table.getColumnModel().getColumn(c);
            int width = Math.max(60, columns.get(c).length() * 12 + 24);
            int sample = Math.min(rows.size(), 50);
            for (int r = 0; r < sample; r++) {
                List<String> row = rows.get(r);
                if (c < row.size()) {
                    String v = row.get(c);
                    width = Math.max(width, Math.min(320, (v == null ? 4 : v.length()) * 9 + 20));
                }
            }
            column.setPreferredWidth(Math.min(width, 320));
        }
    }

    private static String abbreviate(String sql) {
        String s = sql.replace('\n', ' ');
        return s.length() <= 80 ? s : s.substring(0, 80) + "...";
    }

    // ---------------- 内部模型与渲染器 ----------------

    private static final class ResultModel extends AbstractTableModel {

        private List<String> columns = new ArrayList<>();
        private List<List<String>> rows = new ArrayList<>();

        void setData(List<String> columns, List<List<String>> rows) {
            this.columns = new ArrayList<>(columns);
            this.rows = new ArrayList<>(rows);
            fireTableStructureChanged();
        }

        @Override
        public int getRowCount() {
            return rows.size();
        }

        @Override
        public int getColumnCount() {
            return columns.size();
        }

        @Override
        public String getColumnName(int column) {
            return column < columns.size() ? columns.get(column) : "";
        }

        @Override
        public Object getValueAt(int rowIndex, int columnIndex) {
            if (rowIndex >= rows.size()) {
                return null;
            }
            List<String> row = rows.get(rowIndex);
            return columnIndex < row.size() ? row.get(columnIndex) : null;
        }
    }

    /** NULL 与空串必须可区分；数字右对齐便于核对。 */
    private static final class CellRenderer extends DefaultTableCellRenderer {

        @Override
        public Component getTableCellRendererComponent(JTable table, Object value, boolean isSelected,
                                                       boolean hasFocus, int row, int column) {
            JLabel label = (JLabel) super.getTableCellRendererComponent(table, value, isSelected, hasFocus, row, column);
            if (value == null) {
                label.setText("NULL");
                label.setFont(label.getFont().deriveFont(Font.ITALIC));
                if (!isSelected) {
                    label.setForeground(Color.GRAY);
                }
                label.setHorizontalAlignment(SwingConstants.CENTER);
            } else {
                label.setFont(label.getFont().deriveFont(Font.PLAIN));
                if (!isSelected) {
                    label.setForeground(table.getForeground());
                }
                label.setHorizontalAlignment(isNumeric(value.toString())
                        ? SwingConstants.RIGHT : SwingConstants.LEFT);
            }
            return label;
        }

        private boolean isNumeric(String s) {
            if (s.isEmpty()) {
                return false;
            }
            int start = (s.charAt(0) == '-' || s.charAt(0) == '+') ? 1 : 0;
            if (start >= s.length()) {
                return false;
            }
            for (int i = start; i < s.length(); i++) {
                if (!Character.isDigit(s.charAt(i))) {
                    return false;
                }
            }
            return true;
        }
    }
}
