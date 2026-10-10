package com.dbms.client.ui;

import com.dbms.client.entity.Personnel;
import com.dbms.client.service.InventoryService;
import com.dbms.client.util.RegexValidator;

import javax.swing.*;
import javax.swing.table.DefaultTableModel;
import java.awt.*;
import java.util.ArrayList;
import java.util.List;

/** 单物料 / 多物料进出仓 + 流水查询（子系统二、三）。 */
public class InventoryPanel extends JPanel {

    private final InventoryService service = new InventoryService();
    private final Personnel currentUser;

    private final JTextField codeField = new JTextField(6);
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

        reload();
    }

    private JComponent buildSingleMoveBar() {
        JPanel panel = new JPanel(new FlowLayout(FlowLayout.LEFT, 8, 4));
        panel.setBorder(BorderFactory.createTitledBorder("单物料进出仓"));

        panel.add(new JLabel("物料代码："));
        panel.add(codeField);
        panel.add(new JLabel("数量："));
        panel.add(qtyField);
        panel.add(new JLabel("类型："));
        panel.add(typeBox);
        panel.add(new JLabel("备注："));
        panel.add(remarksField);

        JButton submitButton = new JButton("提交");
        submitButton.addActionListener(e -> doSingleMove());
        panel.add(submitButton);

        JButton multiButton = new JButton("多物料进出仓…");
        multiButton.addActionListener(e -> openMultiDialog());
        panel.add(multiButton);

        return panel;
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

    // ---------------- 单物料 ----------------

    private void doSingleMove() {
        if (!RegexValidator.isPositiveInt(codeField.getText())) {
            warn("物料代码必须是正整数");
            return;
        }
        if (!RegexValidator.isPositiveInt(qtyField.getText())) {
            warn("数量必须是正整数");
            return;
        }
        String opType = typeBox.getSelectedIndex() == 0 ? InventoryService.IN : InventoryService.OUT;
        try {
            service.singleMove(Integer.parseInt(codeField.getText().trim()), opType,
                    Integer.parseInt(qtyField.getText().trim()), currentUser.getId(), remarksField.getText().trim());
            JOptionPane.showMessageDialog(this, "操作成功");
            codeField.setText("");
            qtyField.setText("");
            remarksField.setText("");
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
