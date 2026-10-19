package com.dbms.client.ui;

import com.dbms.client.dao.MaterialDao;
import com.dbms.client.entity.Material;
import com.dbms.client.entity.Personnel;
import com.dbms.client.service.InventoryService;
import com.dbms.client.util.RegexValidator;

import javax.swing.*;
import javax.swing.event.DocumentEvent;
import javax.swing.event.DocumentListener;
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
        setBorder(BorderFactory.createEmptyBorder(10, 10, 10, 10));

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

        JButton multiButton = new JButton("多物料进出仓…");
        multiButton.addActionListener(e -> openMultiDialog());
        inputs.add(multiButton);

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
        stockLabel.setFont(new Font("微软雅黑", Font.PLAIN, 13));
        preview.add(stockLabel);

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

    private void openMultiDialog() {
        DefaultTableModel dialogModel = new DefaultTableModel(new String[]{"物料代码", "数量"}, 0);
        for (int i = 0; i < 5; i++) {
            dialogModel.addRow(new Object[]{"", ""});
        }
        JTable dialogTable = new JTable(dialogModel);

        JComboBox<String> dialogType = new JComboBox<>(new String[]{"进仓", "出仓"});
        JTextField dialogRemarks = new JTextField(16);

        JDialog dialog = new JDialog(SwingUtilities.getWindowAncestor(this), "多物料进出仓（同单号全进/全出）",
                Dialog.ModalityType.APPLICATION_MODAL);

        JPanel head = new JPanel(new FlowLayout(FlowLayout.LEFT, 8, 4));
        head.add(new JLabel("类型："));
        head.add(dialogType);
        head.add(new JLabel("备注："));
        head.add(dialogRemarks);

        JButton submit = new JButton("提交本单");
        JButton addRow = new JButton("加一行");
        addRow.addActionListener(e -> dialogModel.addRow(new Object[]{"", ""}));

        JPanel buttons = new JPanel(new FlowLayout(FlowLayout.RIGHT, 8, 4));
        buttons.add(addRow);
        buttons.add(submit);

        submit.addActionListener(e -> {
            List<InventoryService.Item> items = new ArrayList<>();
            for (int r = 0; r < dialogModel.getRowCount(); r++) {
                Object codeObj = dialogModel.getValueAt(r, 0);
                Object qtyObj = dialogModel.getValueAt(r, 1);
                String code = codeObj == null ? "" : codeObj.toString().trim();
                String qty = qtyObj == null ? "" : qtyObj.toString().trim();
                if (code.isEmpty() && qty.isEmpty()) {
                    continue;
                }
                if (!RegexValidator.isPositiveInt(code) || !RegexValidator.isPositiveInt(qty)) {
                    warn("第 " + (r + 1) + " 行：物料代码和数量都必须是正整数");
                    return;
                }
                items.add(new InventoryService.Item(Integer.parseInt(code), Integer.parseInt(qty)));
            }
            if (items.isEmpty()) {
                warn("请至少填写一种物料");
                return;
            }
            String opType = dialogType.getSelectedIndex() == 0 ? InventoryService.IN : InventoryService.OUT;
            try {
                service.multiMove(items, opType, currentUser.getId(), dialogRemarks.getText().trim());
                JOptionPane.showMessageDialog(dialog, "本单已全部提交成功");
                dialog.dispose();
                loadMaterials();
                reload();
            } catch (Exception ex) {
                JOptionPane.showMessageDialog(dialog, ex.getMessage(), "提交失败（已回滚）", JOptionPane.ERROR_MESSAGE);
            }
        });

        dialog.setLayout(new BorderLayout(8, 8));
        dialog.add(head, BorderLayout.NORTH);
        dialog.add(new JScrollPane(dialogTable), BorderLayout.CENTER);
        dialog.add(buttons, BorderLayout.SOUTH);
        dialog.setSize(460, 380);
        dialog.setLocationRelativeTo(this);
        dialog.setVisible(true);
    }

    // ---------------- 查询 ----------------

    private void reload() {
        try {
            dataRows = service.queryLogs(dateKeyword.getText(), codeKeyword.getText(), operatorKeyword.getText());
            model.setRowCount(0);
            for (Object[] row : dataRows) {
                model.addRow(row);
            }
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
