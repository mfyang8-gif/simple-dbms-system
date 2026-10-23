package com.dbms.client.ui;

import com.dbms.client.dao.MaterialDao;
import com.dbms.client.entity.Material;
import com.dbms.client.entity.Personnel;
import com.dbms.client.service.InventoryService;
import com.dbms.client.util.RegexValidator;

import javax.swing.*;
import javax.swing.border.Border;
import javax.swing.event.DocumentEvent;
import javax.swing.event.DocumentListener;
import javax.swing.table.DefaultTableCellRenderer;
import javax.swing.table.DefaultTableModel;
import java.awt.*;
import java.util.ArrayList;
import java.util.List;

/**
 * 单物料 / 多物料进出仓 + 流水查询（子系统二、三）。
 *
 * 【第 5 课】单物料栏从"手输物料代码"改为"下拉选择物料"，
 *           并在输入数量时实时提示库存变化（出仓超量会标红）。
 */
public class InventoryPanel extends JPanel {

    private final InventoryService service = new InventoryService();
    private final MaterialDao materialDao = new MaterialDao();
    private final Personnel currentUser;

    /** 第 5 课：物料下拉框（显示 代码 · 名称（库存 单位））。 */
    private final JComboBox<Material> materialBox = new JComboBox<>();
    /** 第 5 课：库存实时提示。 */
    private final JLabel stockLabel = new JLabel(" ");

    private final JTextField qtyField = new JTextField(6);
    private final JComboBox<String> typeBox = new JComboBox<>(new String[]{"进仓", "出仓"});
    private final JTextField remarksField = new JTextField(16);

    private final JTextField dateKeyword = new JTextField(10);
    private final JTextField codeKeyword = new JTextField(8);
    private final JTextField operatorKeyword = new JTextField(8);

    private final DefaultTableModel model = new DefaultTableModel(InventoryService.LOG_HEADERS, 0) {
        @Override
        public boolean isCellEditable(int row, int column) {
            return false;
        }
    };
    private final JTable table = new JTable(model);
    private List<Object[]> dataRows = new ArrayList<>();

    private String[] exportHeaders = InventoryService.LOG_HEADERS;

    public InventoryPanel(Personnel currentUser) {
        this.currentUser = currentUser;
        setLayout(new BorderLayout(8, 8));
        setBorder(UiTheme.panelPadding());
        UiTheme.styleTable(table, true);

        JPanel top = new JPanel(new BorderLayout(8, 8));
        top.add(buildSingleMoveBar(), BorderLayout.NORTH);
        top.add(buildQueryBar(), BorderLayout.SOUTH);
        add(top, BorderLayout.NORTH);

        add(new JScrollPane(table), BorderLayout.CENTER);

        loadMaterials();
        reload();
    }

    private JComponent buildSingleMoveBar() {
        // 两行：第一行是输入控件，第二行是库存提示（一行放不下，会被挤出去）
        JPanel rows = new JPanel();
        rows.setLayout(new BoxLayout(rows, BoxLayout.Y_AXIS));
        rows.setBorder(BorderFactory.createTitledBorder("单物料进出仓"));

        JPanel inputs = new JPanel(new FlowLayout(FlowLayout.LEFT, 8, 2));

        inputs.add(new JLabel("物料："));
        materialBox.setPreferredSize(new Dimension(260, 26));
        materialBox.setRenderer(new MaterialRenderer());
        inputs.add(materialBox);

        inputs.add(new JLabel("数量："));
        inputs.add(qtyField);

        inputs.add(new JLabel("类型："));
        inputs.add(typeBox);

        inputs.add(new JLabel("备注："));
        inputs.add(remarksField);

        JButton submitButton = new JButton("提交");
        submitButton.addActionListener(e -> doSingleMove());
        inputs.add(submitButton);

        // 监听：换物料 / 改数量 / 换类型，都刷新提示
        materialBox.addActionListener(e -> updateStockPreview());
        typeBox.addActionListener(e -> updateStockPreview());
        qtyField.getDocument().addDocumentListener(new DocumentListener() {
            @Override
            public void insertUpdate(DocumentEvent e) {
                updateStockPreview();
            }

            @Override
            public void removeUpdate(DocumentEvent e) {
                updateStockPreview();
            }

            @Override
            public void changedUpdate(DocumentEvent e) {
                updateStockPreview();
            }
        });

        JPanel preview = new JPanel(new FlowLayout(FlowLayout.LEFT, 8, 0));
        stockLabel.setFont(UiTheme.BODY);
        preview.add(stockLabel);

        // 「多物料进出仓…」放第二行：第一行放不下会被 FlowLayout 换行并裁掉（同工具栏挤爆的坑）
        JButton multiButton = new JButton("多物料进出仓…");
        multiButton.addActionListener(e -> openMultiDialog());
        preview.add(multiButton);

        rows.add(inputs);
        rows.add(preview);
        return rows;
    }

    private JComponent buildQueryBar() {
        JPanel panel = new JPanel(new FlowLayout(FlowLayout.LEFT, 8, 4));
        panel.setBorder(BorderFactory.createTitledBorder("流水查询（支持子串模糊）"));

        panel.add(new JLabel("日期："));
        panel.add(dateKeyword);
        panel.add(new JLabel("物料代码："));
        panel.add(codeKeyword);
        panel.add(new JLabel("操作人ID："));
        panel.add(operatorKeyword);

        JButton searchButton = new JButton("查询");
        searchButton.addActionListener(e -> reload());
        panel.add(searchButton);

        JButton clearButton = new JButton("重置");
        clearButton.addActionListener(e -> {
            dateKeyword.setText("");
            codeKeyword.setText("");
            operatorKeyword.setText("");
            reload();
        });
        panel.add(clearButton);

        return panel;
    }

    // ---------------- 第 5 课：物料下拉 + 库存提示 ----------------

    /** 重新载入物料列表（进出仓后库存会变，必须刷新）。 */
    private void loadMaterials() {
        Integer keepCode = null;
        Material selected = (Material) materialBox.getSelectedItem();
        if (selected != null) {
            keepCode = selected.getMaterialCode();
        }
        try {
            materialBox.removeAllItems();
            for (Material m : materialDao.search("")) {
                materialBox.addItem(m);
            }
            // 尽量保持原来选中项，避免刷新后跳到第一行
            if (keepCode != null) {
                for (int i = 0; i < materialBox.getItemCount(); i++) {
                    if (materialBox.getItemAt(i).getMaterialCode() == keepCode) {
                        materialBox.setSelectedIndex(i);
                        break;
                    }
                }
            }
        } catch (Exception ex) {
            showError(ex);
        }
        updateStockPreview();
    }

    /** 根据所选物料 / 类型 / 数量，实时显示"当前库存 → 操作后库存"。 */
    private void updateStockPreview() {
        Material m = (Material) materialBox.getSelectedItem();
        if (m == null) {
            stockLabel.setText("（物料列表为空，请先在「物料档案」中新增物料）");
            stockLabel.setForeground(Color.GRAY);
            return;
        }
        boolean inbound = typeBox.getSelectedIndex() == 0;
        int stock = m.getStockQty();
        String unit = m.getUnit() == null ? "" : m.getUnit();

        int qty = 0;
        String text = qtyField.getText().trim();
        if (RegexValidator.isPositiveInt(text)) {
            qty = Integer.parseInt(text);
        }

        if (qty <= 0) {
            stockLabel.setText("当前库存 " + stock + " " + unit);
            stockLabel.setForeground(new Color(0x44, 0x44, 0x41));
            return;
        }
        if (inbound) {
            stockLabel.setText("当前库存 " + stock + " " + unit + " → 进仓后 " + (stock + qty) + " " + unit);
            stockLabel.setForeground(new Color(0x0F, 0x6E, 0x56));
        } else {
            int after = stock - qty;
            if (after < 0) {
                stockLabel.setText("⚠ 库存不足：当前只有 " + stock + " " + unit + "，最多可出 " + stock + " " + unit);
                stockLabel.setForeground(new Color(0xA3, 0x2D, 0x2D));
            } else {
                stockLabel.setText("当前库存 " + stock + " " + unit + " → 出仓后剩余 " + after + " " + unit);
                stockLabel.setForeground(new Color(0x0F, 0x6E, 0x56));
            }
        }
    }

    /** 下拉框渲染：显示 "1001 · 螺丝  M4x10（库存 150 个）"。 */
    private static class MaterialRenderer extends DefaultListCellRenderer {
        @Override
        public Component getListCellRendererComponent(JList<?> list, Object value, int index,
                                                      boolean isSelected, boolean cellHasFocus) {
            super.getListCellRendererComponent(list, value, index, isSelected, cellHasFocus);
            if (value instanceof Material m) {
                String spec = m.getSpec() == null || m.getSpec().isBlank() ? "" : " " + m.getSpec();
                setText(m.getMaterialCode() + " · " + m.getName() + spec
                        + "（库存 " + m.getStockQty() + " " + (m.getUnit() == null ? "" : m.getUnit()) + "）");
            }
            return this;
        }
    }

    // ---------------- 单物料 ----------------

    private void doSingleMove() {
        Material m = (Material) materialBox.getSelectedItem();
        if (m == null) {
            warn("请先选择物料");
            return;
        }
        if (!RegexValidator.isPositiveInt(qtyField.getText())) {
            warn("数量必须是正整数");
            return;
        }
        int qty = Integer.parseInt(qtyField.getText().trim());
        String opType = typeBox.getSelectedIndex() == 0 ? InventoryService.IN : InventoryService.OUT;
        try {
            service.singleMove(m.getMaterialCode(), opType, qty, currentUser.getId(),
                    remarksField.getText().trim());
            JOptionPane.showMessageDialog(this,
                    "操作成功：" + m.getName() + " " + (InventoryService.IN.equals(opType) ? "进仓 " : "出仓 ") + qty);
            qtyField.setText("");
            remarksField.setText("");
            loadMaterials();     // 库存变了，刷新下拉框
            reload();
        } catch (Exception ex) {
            showError(ex);
        }
    }

    // ---------------- 多物料（强事务） ----------------

    /**
     * 第 6 课：多物料对话框改造。
     * 以前是"手输物料代码"的空白表格，现在改为：
     *   ① 每行一个物料下拉框（自动显示库存）
     *   ② 第三列实时显示"库存 → 操作后"，出仓超量标红
     *   ③ 底部实时合计（共几种物料、总共多少）
     *   ④ 可以删掉选错的行
     */
    private void openMultiDialog() {
        // ---- 先把物料列表读出来，下拉框和单元格编辑器共用同一份 ----
        List<Material> materials;
        try {
            materials = materialDao.search("");
        } catch (Exception ex) {
            showError(ex);
            return;
        }
        if (materials.isEmpty()) {
            warn("物料档案里还没有物料，请先在「物料档案」中新增物料");
            return;
        }

        // ---- 表格模型：三列，物料列存 Material 对象本身 ----
        DefaultTableModel dialogModel = new DefaultTableModel(new String[]{"物料", "数量", "库存提示"}, 0) {
            @Override
            public boolean isCellEditable(int row, int column) {
                return column != 2;          // 第三列只做展示，不允许编辑
            }
        };
        for (int i = 0; i < 3; i++) {
            dialogModel.addRow(new Object[]{materials.get(0), "", ""});
        }

        JTable dialogTable = new JTable(dialogModel);
        dialogTable.setRowHeight(30);
        dialogTable.getColumnModel().getColumn(0).setPreferredWidth(230);
        dialogTable.getColumnModel().getColumn(1).setPreferredWidth(120);   // 数量列加宽，否则输入框被挤成 0 宽
        dialogTable.getColumnModel().getColumn(2).setPreferredWidth(150);
        dialogTable.putClientProperty("terminateEditOnFocusLost", Boolean.TRUE);  // 点别处也保留编辑值
        dialogTable.setSurrendersFocusOnKeystroke(true);                          // 选中单元格直接打字即可
        // 选中色调成与普通一致 → 任何时候都不会出现蓝色高亮整行
        dialogTable.setSelectionBackground(dialogTable.getBackground());
        dialogTable.setSelectionForeground(dialogTable.getForeground());

        // ---- 数量列：渲染器画成"输入框"的样子 + 编辑器带边框，单击即可输入，回车确认 ----
        JTextField qtyEditor = new JTextField();
        qtyEditor.setHorizontalAlignment(JTextField.RIGHT);
        qtyEditor.setBorder(BorderFactory.createCompoundBorder(
                BorderFactory.createLineBorder(new Color(0x2F, 0x6F, 0xED), 1),
                BorderFactory.createEmptyBorder(0, 6, 0, 6)));
        qtyEditor.setToolTipText("输入数量后按回车确认");
        JTextFieldCellEditor qtyCellEditor = new JTextFieldCellEditor(qtyEditor);
        qtyCellEditor.setClickCountToStart(1);          // 单击就进编辑（默认双击）
        dialogTable.getColumnModel().getColumn(1).setCellEditor(qtyCellEditor);
        dialogTable.getColumnModel().getColumn(1).setCellRenderer(new QtyRenderer());

        // ---- 第 1 列：把下拉框塞进单元格（本课关键代码） ----
        JComboBox<Material> cellBox = new JComboBox<>(materials.toArray(new Material[0]));
        cellBox.setRenderer(new MaterialRenderer());
        dialogTable.getColumnModel().getColumn(0).setCellEditor(new DefaultCellEditor(cellBox));

        // 编辑器只管"点进去编辑时"的样子；静止显示走的是渲染器，
        // 默认渲染器只会 toString()（打出 Material@4eb780a0），所以要单独配一个
        dialogTable.getColumnModel().getColumn(0).setCellRenderer(new DefaultTableCellRenderer() {
            @Override
            protected void setValue(Object value) {
                if (value instanceof Material m) {
                    String spec = m.getSpec() == null || m.getSpec().isBlank() ? "" : " " + m.getSpec();
                    setText(m.getMaterialCode() + " · " + m.getName() + spec
                            + "（库存 " + m.getStockQty() + " " + (m.getUnit() == null ? "" : m.getUnit()) + "）");
                } else {
                    super.setValue(value);
                }
            }
        });

        JComboBox<String> dialogType = new JComboBox<>(new String[]{"进仓", "出仓"});
        JTextField dialogRemarks = new JTextField(14);
        JLabel totalLabel = new JLabel(" ");
        totalLabel.setFont(UiTheme.BODY_BOLD);

        JLabel hintLabel = new JLabel("提示：点数量格子直接输入，按回车确认并跳到下一行；也可点「✓ 确认本行」");
        hintLabel.setFont(UiTheme.SMALL);
        hintLabel.setForeground(new Color(0x7A, 0x7A, 0x76));

        JDialog dialog = new JDialog(SwingUtilities.getWindowAncestor(this),
                "多物料进出仓（同单号全进/全出）", Dialog.ModalityType.APPLICATION_MODAL);

        // ---- 顶部：类型 + 备注 + 操作提示 ----
        JPanel headArea = new JPanel();
        headArea.setLayout(new BoxLayout(headArea, BoxLayout.Y_AXIS));

        JPanel head = new JPanel(new FlowLayout(FlowLayout.LEFT, 8, 4));
        head.add(new JLabel("类型："));
        head.add(dialogType);
        head.add(new JLabel("备注："));
        head.add(dialogRemarks);

        JPanel hintRow = new JPanel(new FlowLayout(FlowLayout.LEFT, 8, 0));
        hintRow.add(hintLabel);

        headArea.add(head);
        headArea.add(hintRow);

        // ---- 底部：合计 + 按钮 ----
        JButton addRow = new JButton("加一行");
        JButton delRow = new JButton("删选中行");
        JButton confirmRow = new JButton("✓ 确认本行");
        JButton submit = new JButton("提交本单");
        submit.setFont(UiTheme.BODY_BOLD);

        JPanel bottom = new JPanel(new BorderLayout(8, 4));
        JPanel btns = new JPanel(new FlowLayout(FlowLayout.RIGHT, 8, 4));
        btns.add(addRow);
        btns.add(delRow);
        btns.add(confirmRow);
        btns.add(submit);
        bottom.add(totalLabel, BorderLayout.WEST);
        bottom.add(btns, BorderLayout.EAST);

        // 确认后把光标静默移到下一行的数量格（不选中行 → 不出现蓝色高亮）
        java.util.function.BiConsumer<Integer, Integer> jumpTo = (row, col) -> {
            dialogTable.clearSelection();                     // 清掉蓝色选中
            dialogTable.editCellAt(row, col);
            dialogTable.requestFocus();
        };

        // 「✓ 确认本行」：把当前正在编辑的格子提交掉，并跳到下一行
        confirmRow.addActionListener(e -> {
            if (dialogTable.isEditing()) {
                dialogTable.getCellEditor().stopCellEditing();   // 回车等价操作
            }
            int row = dialogTable.getSelectedRow();
            if (row < 0) {
                return;
            }
            int next = Math.min(row + 1, dialogTable.getRowCount() - 1);
            jumpTo.accept(next, 1);
        });

        // ---- 填完数量的"确认"方式：回车确认并跳下一行（无高亮）；点别处也保留值 ----
        dialogTable.getInputMap(JComponent.WHEN_ANCESTOR_OF_FOCUSED_COMPONENT)
                .put(KeyStroke.getKeyStroke(java.awt.event.KeyEvent.VK_ENTER, 0), "confirmAndNext");
        dialogTable.getActionMap().put("confirmAndNext", new AbstractAction() {
            @Override
            public void actionPerformed(java.awt.event.ActionEvent e) {
                int row = dialogTable.getSelectedRow();
                int col = dialogTable.getSelectedColumn();
                if (dialogTable.isEditing()) {
                    dialogTable.getCellEditor().stopCellEditing();   // 先把当前值写进表格
                }
                int next = row + 1;
                if (next >= dialogTable.getRowCount()) {
                    next = 0;
                }
                jumpTo.accept(next, col);
            }
        });

        // ---- 刷新合计 + 每行库存提示（换类型 / 改数量 / 换物料 都要调它）----
        Runnable refresh = () -> {
            boolean inbound = dialogType.getSelectedIndex() == 0;
            int kinds = 0;
            long total = 0;                 // 用 long 防溢（数量列可能被人瞎填大数）
            boolean anyShort = false;

            for (int r = 0; r < dialogModel.getRowCount(); r++) {
                Object mObj = dialogModel.getValueAt(r, 0);
                Material m = mObj instanceof Material ? (Material) mObj : null;
                Object qObj = dialogModel.getValueAt(r, 1);
                String qtyText = qObj == null ? "" : qObj.toString().trim();
                int qty = RegexValidator.isPositiveInt(qtyText) ? Integer.parseInt(qtyText) : 0;

                String tip = "";
                if (m != null && qty > 0) {
                    int after = inbound ? m.getStockQty() + qty : m.getStockQty() - qty;
                    if (inbound) {
                        tip = "库存 " + m.getStockQty() + " → " + after;
                    } else if (after < 0) {
                        tip = "⚠ 不足（现有 " + m.getStockQty() + "）";
                        anyShort = true;
                    } else {
                        tip = "库存 " + m.getStockQty() + " → " + after;
                    }
                    kinds++;
                    total += qty;
                } else if (m != null) {
                    tip = "现有 " + m.getStockQty();
                }
                // ⚠️ 关键：值没变就不写回！setValueAt 会再次触发 TableModelListener → refresh，
                //    如果无条件写回就变成 refresh → setValueAt → refresh → … 的死循环（StackOverflowError）。
                //    比较旧值、相同则跳过，循环就能收敛。
                Object oldTip = dialogModel.getValueAt(r, 2);
                if (!java.util.Objects.equals(oldTip, tip)) {
                    dialogModel.setValueAt(tip, r, 2);
                }
            }

            totalLabel.setText("共 " + kinds + " 种物料，合计 " + total
                    + (inbound ? "（进仓）" : "（出仓）"));
            totalLabel.setForeground(anyShort ? new Color(0xA3, 0x2D, 0x2D) : new Color(0x0F, 0x6E, 0x56));
            submit.setEnabled(kinds > 0);
        };

        // ---- 监听：这三处一变就重算 ----
        dialogType.addActionListener(e -> refresh.run());
        dialogModel.addTableModelListener(e -> refresh.run());

        addRow.addActionListener(e -> dialogModel.addRow(new Object[]{materials.get(0), "", ""}));

        delRow.addActionListener(e -> {
            int row = dialogTable.getSelectedRow();
            if (row < 0) {
                warn("先点一下要删除的那一行");
                return;
            }
            if (dialogModel.getRowCount() <= 1) {
                warn("至少要保留一行");
                return;
            }
            dialogModel.removeRow(row);
        });

        submit.addActionListener(e -> {
            List<InventoryService.Item> items = new ArrayList<>();
            for (int r = 0; r < dialogModel.getRowCount(); r++) {
                Object mObj = dialogModel.getValueAt(r, 0);
                Object qObj = dialogModel.getValueAt(r, 1);
                String qtyText = qObj == null ? "" : qObj.toString().trim();
                if (mObj == null || qtyText.isEmpty()) {
                    continue;                       // 空行直接跳过
                }
                if (!(mObj instanceof Material m)) {
                    warn("第 " + (r + 1) + " 行：请选择物料");
                    return;
                }
                if (!RegexValidator.isPositiveInt(qtyText)) {
                    warn("第 " + (r + 1) + " 行：数量必须是正整数");
                    return;
                }
                items.add(new InventoryService.Item(m.getMaterialCode(), Integer.parseInt(qtyText)));
            }
            if (items.isEmpty()) {
                warn("请至少填写一种物料和数量");
                return;
            }
            String opType = dialogType.getSelectedIndex() == 0 ? InventoryService.IN : InventoryService.OUT;
            try {
                service.multiMove(items, opType, currentUser.getId(), dialogRemarks.getText().trim());
                JOptionPane.showMessageDialog(dialog, "本单已全部提交成功（" + items.size() + " 种物料）");
                dialog.dispose();
                loadMaterials();
                reload();
            } catch (Exception ex) {
                JOptionPane.showMessageDialog(dialog, ex.getMessage(), "提交失败（已回滚）",
                        JOptionPane.ERROR_MESSAGE);
            }
        });

        dialog.setLayout(new BorderLayout(8, 8));
        dialog.add(headArea, BorderLayout.NORTH);
        dialog.add(new JScrollPane(dialogTable), BorderLayout.CENTER);
        dialog.add(bottom, BorderLayout.SOUTH);
        dialog.setSize(620, 400);
        dialog.setLocationRelativeTo(this);

        refresh.run();          // 打开时先算一次，避免合计空白
        dialog.setVisible(true);
    }

    /**
     * 数量格的"静止外观"：样式朴素——
     *   空      → 灰色居中占位"点这里输入"
     *   已填    → 正常显示数字（黑字，无底色变化）
     *   非法    → 仅文字标红提醒，不改背景
     */
    private static class QtyRenderer extends DefaultTableCellRenderer {
        @Override
        public Component getTableCellRendererComponent(JTable table, Object value, boolean isSelected,
                                                       boolean hasFocus, int row, int column) {
            super.getTableCellRendererComponent(table, value, isSelected, hasFocus, row, column);
            String text = value == null ? "" : value.toString().trim();
            setHorizontalAlignment(SwingConstants.RIGHT);
            setFont(UiTheme.BODY);

            if (text.isEmpty()) {
                setText("点这里输入");
                setHorizontalAlignment(SwingConstants.CENTER);
                setForeground(new Color(0xA8, 0xA8, 0xA2));
            } else if (RegexValidator.isPositiveInt(text)) {
                setText(text);
                // ⚠️ 渲染器是复用的同一组件，上一个格子设的灰色会残留到这里，必须显式设回黑色
                setForeground(table.getForeground());
            } else {
                setText(text + "（须为正整数）");
                setForeground(new Color(0xA3, 0x2D, 0x2D));
            }
            return this;
        }
    }

    /**
     * 数量列的单元格编辑器：用可见边框的文本框，输入非数字时拒绝提交并提示，
     * 避免"点进去看不见输入框 / 填完不知道有没有生效"。
     */
    private static class JTextFieldCellEditor extends DefaultCellEditor {
        private static final Border NORMAL_BORDER = BorderFactory.createCompoundBorder(
                BorderFactory.createLineBorder(new Color(0x2F, 0x6F, 0xED), 1),
                BorderFactory.createEmptyBorder(0, 6, 0, 6));
        private static final Border ERROR_BORDER = BorderFactory.createCompoundBorder(
                BorderFactory.createLineBorder(new Color(0xA3, 0x2D, 0x2D), 2),
                BorderFactory.createEmptyBorder(0, 5, 0, 5));

        private final JTextField field;

        JTextFieldCellEditor(JTextField field) {
            super(field);
            this.field = field;
        }

        @Override
        public Component getTableCellEditorComponent(JTable table, Object value, boolean isSelected,
                                                     int row, int column) {
            Component c = super.getTableCellEditorComponent(table, value, isSelected, row, column);
            String text = value == null ? "" : value.toString().trim();
            field.setText(text);
            field.setBorder(NORMAL_BORDER);
            field.selectAll();          // 一进去全选，直接输入即可覆盖旧值
            return c;
        }

        @Override
        public boolean stopCellEditing() {
            String text = field.getText().trim();
            if (!text.isEmpty() && !RegexValidator.isPositiveInt(text)) {
                field.setBorder(ERROR_BORDER);
                Toolkit.getDefaultToolkit().beep();
                return false;               // 拒绝提交，光标留在格子里继续改
            }
            field.setBorder(NORMAL_BORDER);
            return super.stopCellEditing();
        }

        @Override
        public Object getCellEditorValue() {
            return field.getText().trim();  // 以字符串写回，refresh 里再判断是否正整数
        }
    }

    // ---------------- 查询 ----------------

    private void reload() {
        try {
            dataRows = service.queryLogs(dateKeyword.getText(), codeKeyword.getText(), operatorKeyword.getText());
            model.setRowCount(0);
            for (Object[] row : dataRows) {
                model.addRow(row);
            }
            UiTheme.fitColumns(table, 70, 220);
            UiTheme.alignNumericColumns(table);
        } catch (Exception ex) {
            showError(ex);
        }
    }

    public String[] getExportHeaders() {
        return exportHeaders;
    }

    public List<Object[]> getExportRows() {
        return dataRows;
    }

    private void warn(String message) {
        JOptionPane.showMessageDialog(this, message, "校验失败", JOptionPane.WARNING_MESSAGE);
    }

    private void showError(Exception ex) {
        JOptionPane.showMessageDialog(this, ex.getMessage(), "操作失败", JOptionPane.ERROR_MESSAGE);
    }
}
