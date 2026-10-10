package com.dbms.client.ui;

import com.dbms.client.dao.MaterialDao;
import com.dbms.client.entity.Material;
import com.dbms.client.util.RegexValidator;

import javax.swing.*;
import javax.swing.table.DefaultTableModel;
import java.awt.*;
import java.util.ArrayList;
import java.util.List;

/** 物料档案管理（子系统一）。 */
public class MaterialPanel extends JPanel {

    private static final String[] COLUMNS = {"物料代码", "物料名称", "规格", "计量单位", "当前库存"};

    private final MaterialDao dao = new MaterialDao();

    private final DefaultTableModel model = new DefaultTableModel(COLUMNS, 0) {
        @Override
        public boolean isCellEditable(int row, int column) {
            return false;
        }
    };
    private final JTable table = new JTable(model);
    private List<Material> dataRows = new ArrayList<>();

    private final JTextField keywordField = new JTextField(12);
    private final JTextField codeField = new JTextField(6);
    private final JTextField nameField = new JTextField(10);
    private final JTextField specField = new JTextField(10);
    private final JTextField unitField = new JTextField(6);
    private final JTextField stockField = new JTextField(8);

    private boolean creating = false;

    public MaterialPanel() {
        setLayout(new BorderLayout(8, 8));
        setBorder(BorderFactory.createEmptyBorder(10, 10, 10, 10));

        add(buildToolbar(), BorderLayout.NORTH);
        add(new JScrollPane(table), BorderLayout.CENTER);
        add(buildForm(), BorderLayout.SOUTH);

        table.getSelectionModel().addListSelectionListener(e -> {
            if (!e.getValueIsAdjusting()) {
                fillFormFromSelection();
            }
        });

        reload();
    }

    private JComponent buildToolbar() {
        JPanel panel = new JPanel(new FlowLayout(FlowLayout.LEFT, 8, 4));
        panel.add(new JLabel("关键字："));
        panel.add(keywordField);
        panel.add(new JLabel("（名称 / 规格 / 单位 子串模糊）"));

        JButton searchButton = new JButton("查询");
        searchButton.addActionListener(e -> reload());
        panel.add(searchButton);

        JButton newButton = new JButton("新增");
        newButton.addActionListener(e -> startCreate());
        panel.add(newButton);

        JButton deleteButton = new JButton("删除");
        deleteButton.addActionListener(e -> doDelete());
        panel.add(deleteButton);

        keywordField.addActionListener(e -> reload());
        return panel;
    }

    private JComponent buildForm() {
        JPanel form = new JPanel(new GridBagLayout());
        form.setBorder(BorderFactory.createTitledBorder("物料明细"));
        GridBagConstraints g = new GridBagConstraints();
        g.insets = new Insets(4, 6, 4, 6);
        g.anchor = GridBagConstraints.WEST;

        field(form, g, 0, "物料代码：", codeField);
        field(form, g, 1, "物料名称：", nameField);
        field(form, g, 2, "规格：", specField);
        field(form, g, 3, "计量单位：", unitField);
        field(form, g, 4, "当前库存：", stockField);

        JButton saveButton = new JButton("保存");
        saveButton.addActionListener(e -> doSave());
        g.gridx = 1;
        g.gridy = 5;
        form.add(saveButton, g);

        return form;
    }

    private void field(JPanel panel, GridBagConstraints g, int row, String label, JComponent comp) {
        g.gridx = 0;
        g.gridy = row;
        panel.add(new JLabel(label), g);
        g.gridx = 1;
        panel.add(comp, g);
    }

    private void reload() {
        try {
            dataRows = dao.search(keywordField.getText());
            model.setRowCount(0);
            for (Material m : dataRows) {
                model.addRow(new Object[]{m.getMaterialCode(), m.getName(), m.getSpec(), m.getUnit(), m.getStockQty()});
            }
        } catch (Exception ex) {
            showError(ex);
        }
    }

    private void startCreate() {
        creating = true;
        table.clearSelection();
        try {
            codeField.setText(String.valueOf(dao.nextCode()));
        } catch (Exception ex) {
            codeField.setText("");
        }
        nameField.setText("");
        specField.setText("");
        unitField.setText("");
        stockField.setText("0");
        nameField.requestFocusInWindow();
    }

    private void fillFormFromSelection() {
        int row = table.getSelectedRow();
        if (row < 0 || row >= dataRows.size()) {
            return;
        }
        Material m = dataRows.get(row);
        creating = false;
        codeField.setText(String.valueOf(m.getMaterialCode()));
        nameField.setText(m.getName());
        specField.setText(m.getSpec());
        unitField.setText(m.getUnit());
        stockField.setText(String.valueOf(m.getStockQty()));
    }

    private void doSave() {
        Material m = new Material();
        try {
            m.setMaterialCode(Integer.parseInt(codeField.getText().trim()));
        } catch (Exception ex) {
            JOptionPane.showMessageDialog(this, "物料代码必须是数字", "校验失败", JOptionPane.WARNING_MESSAGE);
            return;
        }
        m.setName(nameField.getText().trim());
        m.setSpec(specField.getText().trim());
        m.setUnit(unitField.getText().trim());
        if (!RegexValidator.isNonNegativeInt(stockField.getText())) {
            JOptionPane.showMessageDialog(this, "库存必须是非负整数", "校验失败", JOptionPane.WARNING_MESSAGE);
            return;
        }
        m.setStockQty(Integer.parseInt(stockField.getText().trim()));

        if (m.getName().isEmpty() || m.getUnit().isEmpty()) {
            JOptionPane.showMessageDialog(this, "物料名称和计量单位不能为空", "校验失败", JOptionPane.WARNING_MESSAGE);
            return;
        }

        try {
            if (creating) {
                if (dao.findById(m.getMaterialCode()) != null) {
                    throw new IllegalArgumentException("物料代码已存在：" + m.getMaterialCode());
                }
                dao.insert(m);
                JOptionPane.showMessageDialog(this, "新增成功");
            } else {
                dao.update(m);
                JOptionPane.showMessageDialog(this, "修改成功");
            }
            creating = false;
            reload();
        } catch (Exception ex) {
            showError(ex);
        }
    }

    private void doDelete() {
        int row = table.getSelectedRow();
        if (row < 0 || row >= dataRows.size()) {
            JOptionPane.showMessageDialog(this, "请先在表格中选择一条记录", "提示", JOptionPane.INFORMATION_MESSAGE);
            return;
        }
        Material m = dataRows.get(row);
        int ok = JOptionPane.showConfirmDialog(this, "确定删除物料【" + m.getName() + "】吗？",
                "确认删除", JOptionPane.YES_NO_OPTION);
        if (ok != JOptionPane.YES_OPTION) {
            return;
        }
        try {
            dao.delete(m.getMaterialCode());
            reload();
        } catch (Exception ex) {
            showError(ex);
        }
    }

    private void showError(Exception ex) {
        JOptionPane.showMessageDialog(this, ex.getMessage(), "操作失败", JOptionPane.ERROR_MESSAGE);
    }
}
