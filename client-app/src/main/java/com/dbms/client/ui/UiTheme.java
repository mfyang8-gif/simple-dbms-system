package com.dbms.client.ui;

import javax.swing.*;
import javax.swing.border.Border;
import javax.swing.table.DefaultTableCellRenderer;
import javax.swing.table.JTableHeader;
import javax.swing.table.TableColumn;
import java.awt.*;
import java.util.Arrays;
import java.util.HashSet;
import java.util.Set;

/**
 * 全局界面规范：字体、行高、边距、配色统一在这里定义（第 10 课）。
 *
 * 为什么改 UIManager 的默认值，而不是给每个组件 setFont？
 * Swing 组件的字体来自 LookAndFeel 的默认值。FlatLaf 在 Windows 上默认取
 * "Segoe UI"，中文会回退到别的字体，于是 JLabel 用一套、JTable 用另一套，
 * 同一个界面里字形和粗细不统一。
 * 直接改 UIManager 的默认绑定点，等于给所有组件（包括以后新加的）统一换字体，
 * 不用在每个面板里重复写 setFont —— 这也是 Swing 里做主题的标准做法。
 *
 * 注意调用时机：必须在 setLookAndFeel 之后、创建任何窗口之前。
 * 顺序反了会被 LAF 的默认值覆盖掉。
 */
public final class UiTheme {

    /** 优先使用的中文字体，按顺序取第一个系统里存在的。 */
    public static final String FONT_FAMILY = pickFontFamily();

    public static final Font BODY = new Font(FONT_FAMILY, Font.PLAIN, 13);
    public static final Font BODY_BOLD = new Font(FONT_FAMILY, Font.BOLD, 13);
    public static final Font SMALL = new Font(FONT_FAMILY, Font.PLAIN, 12);
    public static final Font HEADING = new Font(FONT_FAMILY, Font.BOLD, 16);
    public static final Font APP_TITLE = new Font(FONT_FAMILY, Font.BOLD, 22);

    /** 表格行高：13px 中文字号下 26 比较舒服，默认 16 太挤。 */
    public static final int ROW_HEIGHT = 26;
    public static final int HEADER_HEIGHT = 28;

    /** 面板外边距，全项目统一。 */
    public static final int PAD = 10;

    /** 次要文字：提示语、占位符、单位说明。 */
    public static final Color MUTED = new Color(0x8A, 0x8A, 0x85);
    /** 校验通过。 */
    public static final Color OK = new Color(0x0F, 0x6E, 0x56);
    /** 校验失败 / 警示。 */
    public static final Color ERROR = new Color(0xA3, 0x2D, 0x2D);
    /** 表格网格线：浅灰，别用默认的深灰。 */
    public static final Color GRID = new Color(0xE2, 0xE2, 0xE2);

    private UiTheme() {
    }

    /**
     * 统一全局字体与表格外观。在 App 的 setupLookAndFeel() 之后调用一次即可。
     */
    public static void install() {
        String[] fontKeys = {
                "Label.font", "Button.font", "ToggleButton.font", "CheckBox.font",
                "RadioButton.font", "TextField.font", "PasswordField.font",
                "FormattedTextField.font", "TextArea.font", "EditorPane.font",
                "ComboBox.font", "Spinner.font", "List.font", "Table.font",
                "TableHeader.font", "MenuBar.font", "Menu.font", "MenuItem.font",
                "CheckBoxMenuItem.font", "RadioButtonMenuItem.font",
                "PopupMenu.font", "OptionPane.messageFont", "OptionPane.buttonFont",
                "TabbedPane.font", "ToolTip.font", "TitledBorder.font", "ProgressBar.font"
        };
        for (String key : fontKeys) {
            UIManager.put(key, BODY);
        }
        UIManager.put("TitledBorder.font", BODY_BOLD);
        UIManager.put("ToolTip.font", SMALL);
        UIManager.put("Table.rowHeight", ROW_HEIGHT);
        UIManager.put("Table.gridColor", GRID);
        UIManager.put("Table.selectionBackground", new Color(0xD6, 0xE4, 0xF7));
        UIManager.put("Table.selectionForeground", Color.BLACK);
    }

    /** 统一的面板内边距（四周 10）。 */
    public static Border panelPadding() {
        return BorderFactory.createEmptyBorder(PAD, PAD, PAD, PAD);
    }

    /**
     * 表格通用外观。
     *
     * @param sortable 是否允许点表头排序。
     *                 报表里带"合计"行的表格不能开——排序会把合计行一起排进去。
     */
    public static void styleTable(JTable table, boolean sortable) {
        table.setRowHeight(ROW_HEIGHT);
        table.setFillsViewportHeight(true);              // 数据少时表格仍铺满，不留白框
        table.setShowGrid(true);
        table.setGridColor(GRID);
        table.setIntercellSpacing(new Dimension(1, 1));
        table.getTableHeader().setReorderingAllowed(false);   // 防止误拖列
        table.getTableHeader().setPreferredSize(
                new Dimension(0, HEADER_HEIGHT));
        if (sortable) {
            table.setAutoCreateRowSorter(true);
        }
    }

    /**
     * 按内容估算列宽：取表头和该列数据的最大字符宽度，再留一点余量。
     * 不这样做的话 JTable 默认把总宽度平分给各列，
     * 结果"物料代码"和"家庭住址"一样宽，地址被截断、代码留一大片空白。
     */
    public static void fitColumns(JTable table, int minWidth, int maxWidth) {
        FontMetrics headerMetrics = table.getTableHeader().getFontMetrics(BODY_BOLD);
        for (int col = 0; col < table.getColumnCount(); col++) {
            TableColumn column = table.getColumnModel().getColumn(col);
            int width = headerMetrics.stringWidth(String.valueOf(table.getColumnName(col)));
            for (int row = 0; row < table.getRowCount(); row++) {
                Object value = table.getValueAt(row, col);
                if (value != null) {
                    width = Math.max(width,
                            table.getFontMetrics(BODY).stringWidth(value.toString()));
                }
            }
            column.setPreferredWidth(Math.max(minWidth, Math.min(maxWidth, width + 24)));
        }
    }

    /**
     * 让"整列都是数字"的列右对齐（代码、数量、金额这类列左对齐时位数参差，很难纵向比较）。
     *
     * 做法：逐列扫描，只有当该列所有非空值都是 Number 时才右对齐；
     * 这样"合计""缺货"之类的文字列自动不受影响，新增报表也不用逐列手工指定。
     *
     * 注意：必须在 setDataVector / fitColumns 之后调用——
     * setDataVector 会重建所有 TableColumn，之前设置的渲染器会一并丢失。
     */
    public static void alignNumericColumns(JTable table) {
        for (int col = 0; col < table.getColumnCount(); col++) {
            boolean allNumeric = table.getRowCount() > 0;
            for (int row = 0; row < table.getRowCount(); row++) {
                Object value = table.getValueAt(row, col);
                if (value != null && !(value instanceof Number)) {
                    allNumeric = false;
                    break;
                }
            }
            if (allNumeric) {
                DefaultTableCellRenderer renderer = new DefaultTableCellRenderer();
                renderer.setHorizontalAlignment(SwingConstants.RIGHT);
                table.getColumnModel().getColumn(col).setCellRenderer(renderer);
            }
        }
    }

    /** 次要提示文字（灰）。 */
    public static JLabel muted(String text) {
        JLabel label = new JLabel(text);
        label.setForeground(MUTED);
        label.setFont(SMALL);
        return label;
    }

    /**
     * 用代码画一个应用图标（蓝底白字"仓"），省得依赖外部 png 文件。
     * 登录窗和主窗共用，任务栏里能一眼认出是哪个程序。
     */
    public static Image appIcon() {
        int size = 32;
        java.awt.image.BufferedImage img =
                new java.awt.image.BufferedImage(size, size, java.awt.image.BufferedImage.TYPE_INT_ARGB);
        Graphics2D g = img.createGraphics();
        g.setRenderingHint(RenderingHints.KEY_ANTIALIASING, RenderingHints.VALUE_ANTIALIAS_ON);
        g.setRenderingHint(RenderingHints.KEY_TEXT_ANTIALIASING,
                RenderingHints.VALUE_TEXT_ANTIALIAS_ON);
        g.setColor(new Color(0x2F, 0x6F, 0xED));
        g.fillRoundRect(0, 0, size, size, 8, 8);
        g.setColor(Color.WHITE);
        g.setFont(new Font(FONT_FAMILY, Font.BOLD, 20));
        FontMetrics fm = g.getFontMetrics();
        String text = "仓";
        g.drawString(text, (size - fm.stringWidth(text)) / 2,
                (size - fm.getHeight()) / 2 + fm.getAscent());
        g.dispose();
        return img;
    }

    private static String pickFontFamily() {
        String[] prefer = {"Microsoft YaHei UI", "微软雅黑", "Microsoft YaHei", "SimHei"};
        Set<String> available = new HashSet<>(Arrays.asList(
                GraphicsEnvironment.getLocalGraphicsEnvironment().getAvailableFontFamilyNames()));
        for (String name : prefer) {
            if (available.contains(name)) {
                return name;
            }
        }
        return Font.SANS_SERIF;
    }
}
