package com.dbms.client.ui;

import com.dbms.client.dao.MaterialDao;
import com.dbms.client.entity.Material;
import com.dbms.client.util.RegexValidator;

import javax.swing.*;
import javax.swing.event.DocumentEvent;
import javax.swing.event.DocumentListener;
import javax.swing.table.DefaultTableModel;
import java.awt.*;
import java.util.ArrayList;
import java.util.List;

/**
 * 物料档案管理（子系统一）。
 *
 * 【第 10 课】界面走查改造：
 *   ① 表单从"5 行单列"改成"3 行两列"，省下将近一半高度还给表格；
 *   ② 每个字段加实时校验提示（与人员档案同一套交互：✓ / 具体原因 + 保存按钮联动）；
 *   ③ 补"取消"按钮，填了一半可以退回未选中状态；
 *   ④ 表格按内容自适应列宽 + 工具栏右侧显示记录数，空结果时明确提示而不是一片空白。
 */
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

    // ---- 实时校验提示（每个字段一个，宽度固定，避免文字长短导致布局跳动） ----
    private final JLabel codeHint = hint();
    private final JLabel nameHint = hint();
    private final JLabel specHint = hint();
    private final JLabel unitHint = hint();
    private final JLabel stockHint = hint();

    private final JLabel countLabel = UiTheme.muted("");
    private JButton saveButton;

    private boolean creating = false;

    public MaterialPanel() {
        setLayout(new BorderLayout(8, 8));
        setBorder(UiTheme.panelPadding());

        UiTheme.styleTable(table, true);
        add(buildToolbar(), BorderLayout.NORTH);
        add(new JScrollPane(table), BorderLayout.CENTER);
        add(buildForm(), BorderLayout.SOUTH);

        table.getSelectionModel().addListSelectionListener(e -> {
            if (!e.getValueIsAdjusting()) {
                fillFormFromSelection();
            }
        });

        bindLiveValidation();
        reload();
        validateLive();     // 初始状态：未填任何内容时显示"必填"、保存按钮禁用
    }

    // ---------------- 顶部工具栏 ----------------

    private JComponent buildToolbar() {
        JPanel left = new JPanel(new FlowLayout(FlowLayout.LEFT, 8, 4));
        left.add(new JLabel("关键字："));
        left.add(keywordField);
        left.add(UiTheme.muted("（名称 / 规格 / 单位 子串模糊）"));

        JButton searchButton = new JButton("查询");
        searchButton.addActionListener(e -> reload());
        left.add(searchButton);

        JButton newButton = new JButton("新增");
        newButton.addActionListener(e -> startCreate());
        left.add(newButton);

        JButton deleteButton = new JButton("删除");
        deleteButton.addActionListener(e -> doDelete());
        left.add(deleteButton);

        keywordField.addActionListener(e -> reload());

        JPanel panel = new JPanel(new BorderLayout());
        panel.add(left, BorderLayout.WEST);
        countLabel.setBorder(BorderFactory.createEmptyBorder(0, 0, 0, UiTheme.PAD));
        panel.add(countLabel, BorderLayout.EAST);
        return panel;
    }

    // ---------------- 底部表单（两列三行） ----------------

    private JComponent buildForm() {
        JPanel form = new JPanel(new GridBagLayout());
        form.setBorder(BorderFactory.createTitledBorder("物料明细"));
        GridBagConstraints g = new GridBagConstraints();
        g.insets = new Insets(4, 6, 4, 6);
        g.anchor = GridBagConstraints.WEST;

        addField(form, g, 0, 0, "物料代码：", codeField, codeHint);
        addField(form, g, 1, 0, "物料名称：", nameField, nameHint);
        addField(form, g, 0, 1, "规格：", specField, specHint);
        addField(form, g, 1, 1, "计量单位：", unitField, unitHint);
        addField(form, g, 0, 2, "当前库存：", stockField, stockHint);

        JPanel buttons = new JPanel(new FlowLayout(FlowLayout.LEFT, 8, 0));
        saveButton = new JButton("保存");
        saveButton.addActionListener(e -> doSave());
        JButton cancelButton = new JButton("取消");
        cancelButton.addActionListener(e -> doCancel());
        buttons.add(saveButton);
        buttons.add(cancelButton);

        // 放在右列第三行的标签位，正好跟左列的"当前库存"并排
        g.gridx = 3;
        g.gridy = 2;
        g.gridwidth = 2;
        form.add(buttons, g);
        g.gridwidth = 1;

        return form;
    }

    /**
     * 把一组"标签 + 输入框 + 提示"摆到第 row 行。
     *
     * @param group 0 = 左列，1 = 右列；每组占 3 个 grid 列（标签/输入/提示）
     */
    private void addField(JPanel panel, GridBagConstraints g, int group, int row,
                          String label, JComponent field, JLabel hint) {
        g.gridx = group * 3;
        g.gridy = row;
        panel.add(new JLabel(label), g);
        g.gridx = group * 3 + 1;
        panel.add(field, g);
        g.gridx = group * 3 + 2;
        panel.add(hint, g);
    }

    // ---------------- 第 10 课：实时校验 ----------------

    private static JLabel hint() {
        JLabel label = new JLabel(" ");
        label.setFont(UiTheme.SMALL);
        label.setForeground(UiTheme.MUTED);
        label.setPreferredSize(new Dimension(130, 18));
        return label;
    }

    private void bindLiveValidation() {
        for (JTextField field : new JTextField[]{codeField, nameField, specField, unitField, stockField}) {
            field.getDocument().addDocumentListener(new DocumentListener() {
                @Override
                public void insertUpdate(DocumentEvent e) {
                    validateLive();
                }

                @Override
                public void removeUpdate(DocumentEvent e) {
                    validateLive();
                }

                @Override
                public void changedUpdate(DocumentEvent e) {
                    validateLive();
                }
            });
        }
    }

    /** 逐项校验并刷新提示；返回值表示"是否可以保存"。规则全部来自 RegexValidator。 */
    private boolean validateLive() {
        boolean ok = true;

        // 物料代码：主键，正整数
        String code = codeField.getText().trim();
        if (code.isEmpty()) {
            setHint(codeHint, "必填", false);
            ok = false;
        } else if (!RegexValidator.isPositiveInt(code)) {
            setHint(codeHint, "必须是正整数", false);
            ok = false;
        } else {
            setHint(codeHint, "✓", true);
        }

        // 物料名称：必填，最长 50（表结构 VARCHAR(50)）
        String name = nameField.getText().trim();
        if (name.isEmpty()) {
            setHint(nameHint, "必填", false);
            ok = false;
        } else if (name.length() > 50) {
            setHint(nameHint, "最长 50 个字符", false);
            ok = false;
        } else {
            setHint(nameHint, "✓", true);
        }

        // 规格：可空，最长 50
        String spec = specField.getText().trim();
        if (spec.length() > 50) {
            setHint(specHint, "最长 50 个字符", false);
            ok = false;
        } else {
            setHint(specHint, spec.isEmpty() ? "可留空" : "✓", true);
        }

        // 计量单位：必填，最长 20
        String unit = unitField.getText().trim();
        if (unit.isEmpty()) {
            setHint(unitHint, "必填", false);
            ok = false;
        } else if (unit.length() > 20) {
            setHint(unitHint, "最长 20 个字符", false);
            ok = false;
        } else {
            setHint(unitHint, "✓", true);
        }

        // 当前库存：非负整数（新建物料库存可以是 0）
        String stock = stockField.getText().trim();
        if (stock.isEmpty()) {
            setHint(stockHint, "必填", false);
            ok = false;
        } else if (!RegexValidator.isNonNegativeInt(stock)) {
            setHint(stockHint, "必须是非负整数", false);
            ok = false;
        } else {
            setHint(stockHint, "✓", true);
        }

        if (saveButton != null) {
            saveButton.setEnabled(ok);
        }
        return ok;
    }

    private void setHint(JLabel label, String text, boolean ok) {
        label.setText(text);
        label.setForeground(ok ? UiTheme.OK : UiTheme.ERROR);
    }

    // ---------------- 数据加载 ----------------

    private void reload() {
        try {
            dataRows = dao.search(keywordField.getText());
            model.setRowCount(0);
            for (Material m : dataRows) {
                model.addRow(new Object[]{m.getMaterialCode(), m.getName(), m.getSpec(),
                        m.getUnit(), m.getStockQty()});
            }
            UiTheme.fitColumns(table, 80, 240);
            UiTheme.alignNumericColumns(table);
            countLabel.setText(dataRows.isEmpty()
                    ? "没有符合条件的记录"
                    : "共 " + dataRows.size() + " 条记录");
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
        validateLive();
        nameField.requestFocusInWindow();
    }

    private void doCancel() {
        creating = false;
        table.clearSelection();
        clearForm();
        validateLive();
    }

    private void clearForm() {
        codeField.setText("");
        nameField.setText("");
        specField.setText("");
        unitField.setText("");
        stockField.setText("");
    }

    private void fillFormFromSelection() {
        int viewRow = table.getSelectedRow();
        if (viewRow < 0) {
            return;
        }
        // 开了表头排序后视图行号和数据行号会不一致，必须转换后再取
        int modelRow = table.convertRowIndexToModel(viewRow);
        if (modelRow < 0 || modelRow >= dataRows.size()) {
            return;
        }
        Material m = dataRows.get(modelRow);
        creating = false;
        codeField.setText(String.valueOf(m.getMaterialCode()));
        nameField.setText(m.getName());
        specField.setText(m.getSpec());
        unitField.setText(m.getUnit());
        stockField.setText(String.valueOf(m.getStockQty()));
        validateLive();
    }

    private void doSave() {
        // 兜底再校验一次：防止有人用快捷键或其他途径绕过按钮置灰
        if (!validateLive()) {
            return;
        }
        Material m = new Material();
        m.setMaterialCode(Integer.parseInt(codeField.getText().trim()));
        m.setName(nameField.getText().trim());
        m.setSpec(specField.getText().trim());
        m.setUnit(unitField.getText().trim());
        m.setStockQty(Integer.parseInt(stockField.getText().trim()));

        try {
            if (creating) {
                if (dao.findById(m.getMaterialCode()) != null) {
                    throw new IllegalArgumentException("物料代码已存在：" + m.getMaterialCode());
                }
                dao.insert(m);
                info("新增成功");
            } else {
                dao.update(m);
                info("修改成功");
            }
            creating = false;
            reload();
            validateLive();
        } catch (Exception ex) {
            showError(ex);
        }
    }

    private void doDelete() {
        int row = table.getSelectedRow();
        if (row < 0 || row >= table.getRowCount()) {
            warn("请先在表格中选择一条记录");
            return;
        }
        Material m = dataRows.get(table.convertRowIndexToModel(row));
        int ok = JOptionPane.showConfirmDialog(this,
                "确定删除物料【" + m.getName() + "】吗？\n删除后不可恢复。",
                "确认删除", JOptionPane.YES_NO_OPTION, JOptionPane.WARNING_MESSAGE);
        if (ok != JOptionPane.YES_OPTION) {
            return;
        }
        try {
            dao.delete(m.getMaterialCode());
            clearForm();
            reload();
            validateLive();
        } catch (Exception ex) {
            showError(ex);
        }
    }

    private void info(String message) {
        JOptionPane.showMessageDialog(this, message, "提示", JOptionPane.INFORMATION_MESSAGE);
    }

    private void warn(String message) {
        JOptionPane.showMessageDialog(this, message, "提示", JOptionPane.WARNING_MESSAGE);
    }

    private void showError(Exception ex) {
        JOptionPane.showMessageDialog(this, ex.getMessage(), "操作失败", JOptionPane.ERROR_MESSAGE);
    }
}
