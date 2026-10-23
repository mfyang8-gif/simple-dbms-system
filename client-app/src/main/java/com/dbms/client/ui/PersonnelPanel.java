package com.dbms.client.ui;

import com.dbms.client.entity.Personnel;
import com.dbms.client.service.PersonnelService;
import com.dbms.client.util.RegexValidator;

import javax.swing.*;
import javax.swing.event.DocumentEvent;
import javax.swing.event.DocumentListener;
import javax.swing.table.DefaultTableModel;
import java.awt.*;
import java.time.LocalDate;
import java.util.ArrayList;
import java.util.List;

/**
 * 人员档案管理（子系统一 + 子系统四的权限授予/回收）。
 *
 * 【第 7 课】输入框改为"边输入边校验"：
 *   每个需要正则校验的字段右边跟一个小标签，实时显示 ✓ / 具体错误原因，
 *   全部合法时"保存"按钮才可用。这样用户不用点保存才知道哪里填错。
 */
public class PersonnelPanel extends JPanel {

    private static final String[] COLUMNS =
            {"人员代码", "姓名", "性别", "出生日期", "身份证号", "籍贯", "家庭住址", "联系电话", "权限"};

    private final PersonnelService service = new PersonnelService();
    private final Personnel currentUser;

    private final DefaultTableModel model = new DefaultTableModel(COLUMNS, 0) {
        @Override
        public boolean isCellEditable(int row, int column) {
            return false;
        }
    };
    private final JTable table = new JTable(model);
    private List<Personnel> dataRows = new ArrayList<>();

    private final JTextField keywordField = new JTextField(12);
    private final JTextField idField = new JTextField(6);
    private final JTextField nameField = new JTextField(8);
    private final JComboBox<String> genderBox = new JComboBox<>(new String[]{"男", "女"});
    private final JTextField birthField = new JTextField(9);
    private final JTextField idCardField = new JTextField(15);
    private final JTextField nativeField = new JTextField(8);
    private final JTextField addressField = new JTextField(16);
    private final JTextField phoneField = new JTextField(10);
    private final JPasswordField passwordField = new JPasswordField(10);

    // ---- 实时校验提示（每个字段一个） ----
    private final JLabel idHint = hintLabel();
    private final JLabel nameHint = hintLabel();
    private final JLabel birthHint = hintLabel();
    private final JLabel idCardHint = hintLabel();
    private final JLabel phoneHint = hintLabel();
    private final JLabel pwdHint = hintLabel();
    private JButton saveButton;

    private final JLabel countLabel = UiTheme.muted("");

    private boolean creating = false;

    public PersonnelPanel(Personnel currentUser) {
        this.currentUser = currentUser;
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
        left.add(UiTheme.muted("（姓名 / 籍贯 / 电话 子串模糊）"));

        JButton searchButton = new JButton("查询");
        searchButton.addActionListener(e -> reload());
        left.add(searchButton);

        JButton newButton = new JButton("新增");
        newButton.addActionListener(e -> startCreate());
        left.add(newButton);

        JButton deleteButton = new JButton("删除");
        deleteButton.addActionListener(e -> doDelete());
        left.add(deleteButton);

        // 只有管理员能授予/回收权限 —— 动态控制控件可见性
        if (currentUser.getRoleId() == 1) {
            JButton roleButton = new JButton("授予/回收管理员");
            roleButton.addActionListener(e -> toggleRole());
            left.add(roleButton);
        }

        keywordField.addActionListener(e -> reload());

        JPanel panel = new JPanel(new BorderLayout());
        panel.add(left, BorderLayout.WEST);
        countLabel.setBorder(BorderFactory.createEmptyBorder(0, 0, 0, UiTheme.PAD));
        panel.add(countLabel, BorderLayout.EAST);
        return panel;
    }

    // ---------------- 底部表单（两列四行 + 口令行） ----------------

    private JComponent buildForm() {
        JPanel form = new JPanel(new GridBagLayout());
        form.setBorder(BorderFactory.createTitledBorder("档案明细"));
        GridBagConstraints g = new GridBagConstraints();
        g.insets = new Insets(4, 6, 4, 6);
        g.anchor = GridBagConstraints.WEST;

        // 两列布局：半区 0 占网格第 0~2 列，半区 1 占第 4~6 列（第 3 列是两半区之间的间隔）。
        // 短字段并排，长字段（身份证/住址）单独一侧，高度比单列省一半，表格能多看几行。
        addField(form, g, 0, 0, "人员代码：", idField, idHint);
        addField(form, g, 0, 1, "姓名：", nameField, nameHint);
        addField(form, g, 1, 0, "性别：", genderBox, null);
        addField(form, g, 1, 1, "出生日期：", birthField, birthHint);
        addField(form, g, 2, 0, "身份证号：", idCardField, idCardHint);
        addField(form, g, 2, 1, "联系电话：", phoneField, phoneHint);
        addField(form, g, 3, 0, "籍贯：", nativeField, null);
        addField(form, g, 3, 1, "家庭住址：", addressField, null);

        saveButton = new JButton("保存");
        saveButton.addActionListener(e -> doSave());

        // 最后一行：口令 + 保存。口令的规则提示放右边半区，跟其他提示同一排位置
        g.gridy = 4;
        g.gridx = 0;
        form.add(new JLabel("设置口令："), g);
        g.gridx = 1;
        form.add(passwordField, g);
        g.gridx = 2;
        form.add(pwdHint, g);
        g.gridx = 4;
        form.add(UiTheme.muted("（新增必填；修改时留空则不改）"), g);
        g.gridx = 5;
        form.add(saveButton, g);

        return form;
    }

    /**
     * 往表单里加一组「标签 + 输入框 + 提示」。
     *
     * @param row  行号
     * @param half 半区：0 = 左列（网格第 0~2 列），1 = 右列（网格第 4~6 列）
     */
    private void addField(JPanel panel, GridBagConstraints g, int row, int half, String label,
                          JComponent field, JLabel hint) {
        g.gridy = row;
        g.gridx = half == 0 ? 0 : 4;
        panel.add(new JLabel(label), g);
        g.gridx++;
        panel.add(field, g);
        if (hint != null) {
            g.gridx++;
            panel.add(hint, g);
        }
    }

    // ---------------- 第 7 课：边输入边校验 ----------------

    private static JLabel hintLabel() {
        JLabel l = new JLabel(" ");
        l.setFont(UiTheme.SMALL);
        l.setForeground(UiTheme.MUTED);
        return l;
    }

    /** 给需要校验的字段挂 DocumentListener / ActionListener，输入时实时刷新提示。 */
    private void bindLiveValidation() {
        watch(idField);
        watch(nameField);
        watch(birthField);
        watch(idCardField);
        watch(phoneField);
        watch(passwordField);
    }

    private void watch(JTextField field) {
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

    /**
     * 逐项校验并刷新提示标签；返回值表示"是否可以保存"。
     * 校验规则全部来自 RegexValidator，与业务层保持一致。
     */
    private boolean validateLive() {
        boolean ok = true;

        // 人员代码：正整数
        String idText = idField.getText().trim();
        if (idText.isEmpty()) {
            setHint(idHint, "必填", false);
            ok = false;
        } else if (!RegexValidator.isPositiveInt(idText)) {
            setHint(idHint, "必须是正整数", false);
            ok = false;
        } else {
            setHint(idHint, "✓", true);
        }

        // 姓名：2-20 位中文或字母
        String nameText = nameField.getText().trim();
        if (nameText.isEmpty()) {
            setHint(nameHint, "必填", false);
            ok = false;
        } else if (!RegexValidator.isName(nameText)) {
            setHint(nameHint, "2-20 位中文或字母", false);
            ok = false;
        } else {
            setHint(nameHint, "✓", true);
        }

        // 出生日期：格式 + 真实存在
        String birthText = birthField.getText().trim();
        if (birthText.isEmpty()) {
            setHint(birthHint, "必填，如 1990-05-12", false);
            ok = false;
        } else if (!RegexValidator.isDate(birthText)) {
            setHint(birthHint, "格式须为 YYYY-MM-DD", false);
            ok = false;
        } else if (!isRealDate(birthText)) {
            setHint(birthHint, "该日期不存在", false);
            ok = false;
        } else {
            setHint(birthHint, "✓", true);
        }

        // 身份证号：18 位，前 17 位数字，末位数字或 X
        String idCardText = idCardField.getText().trim();
        if (idCardText.isEmpty()) {
            setHint(idCardHint, "必填，18 位", false);
            ok = false;
        } else if (!RegexValidator.isIdCard(idCardText)) {
            setHint(idCardHint, "18 位：前 17 位数字，末位数字或 X", false);
            ok = false;
        } else if (!RegexValidator.isIdCardStrict(idCardText)) {
            setHint(idCardHint, "地区码或出生日期段不合法", false);
            ok = false;
        } else {
            setHint(idCardHint, "✓", true);
        }

        // 联系电话：1[3-9] 开头 11 位
        String phoneText = phoneField.getText().trim();
        if (phoneText.isEmpty()) {
            setHint(phoneHint, "必填，11 位手机号", false);
            ok = false;
        } else if (!RegexValidator.isPhone(phoneText)) {
            setHint(phoneHint, "须为 1 开头的 11 位手机号", false);
            ok = false;
        } else {
            setHint(phoneHint, "✓", true);
        }

        // 口令：新增必填；修改时留空表示不改
        String pwdText = new String(passwordField.getPassword());
        if (pwdText.isEmpty()) {
            if (creating) {
                setHint(pwdHint, "新增必填：6-20 位含字母和数字", false);
                ok = false;
            } else {
                setHint(pwdHint, "留空则不修改", true);
            }
        } else if (!RegexValidator.isPassword(pwdText)) {
            setHint(pwdHint, "6-20 位，须同时含字母和数字", false);
            ok = false;
        } else {
            setHint(pwdHint, "✓", true);
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

    private boolean isRealDate(String s) {
        try {
            LocalDate.parse(s.trim());
            return true;
        } catch (Exception e) {
            return false;
        }
    }

    // ---------------- 数据操作 ----------------

    private void reload() {
        try {
            dataRows = service.search(keywordField.getText());
            model.setRowCount(0);
            for (Personnel p : dataRows) {
                model.addRow(new Object[]{
                        p.getId(), p.getName(), p.getGender(), p.getBirthDate(), p.getIdCard(),
                        p.getNativePlace(), p.getAddress(), p.getPhone(), p.roleLabel()
                });
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
            idField.setText(String.valueOf(service.nextId()));
        } catch (Exception ex) {
            idField.setText("");
        }
        nameField.setText("");
        genderBox.setSelectedIndex(0);
        birthField.setText("1990-01-01");
        idCardField.setText("");
        nativeField.setText("");
        addressField.setText("");
        phoneField.setText("");
        passwordField.setText("");
        nameField.requestFocusInWindow();
        validateLive();
    }

    private void fillFormFromSelection() {
        int viewRow = table.getSelectedRow();
        if (viewRow < 0) {
            return;
        }
        // 表头可排序后，视图行号 ≠ 数据行号，必须转换
        int modelRow = table.convertRowIndexToModel(viewRow);
        if (modelRow < 0 || modelRow >= dataRows.size()) {
            return;
        }
        Personnel p = dataRows.get(modelRow);
        creating = false;
        idField.setText(String.valueOf(p.getId()));
        nameField.setText(p.getName());
        genderBox.setSelectedItem(p.getGender());
        birthField.setText(p.getBirthDate());
        idCardField.setText(p.getIdCard());
        nativeField.setText(p.getNativePlace());
        addressField.setText(p.getAddress());
        phoneField.setText(p.getPhone());
        passwordField.setText("");
        validateLive();
    }

    private void doSave() {
        // 兜底：按钮虽然按校验结果禁用，但键盘触发等路径也可能进来，再检查一次
        if (!validateLive()) {
            JOptionPane.showMessageDialog(this,
                    "还有字段填写不正确，请看每个输入框右侧的红色提示",
                    "校验失败", JOptionPane.WARNING_MESSAGE);
            return;
        }
        Personnel p = new Personnel();
        try {
            p.setId(Integer.parseInt(idField.getText().trim()));
        } catch (Exception ex) {
            JOptionPane.showMessageDialog(this, "人员代码必须是数字", "校验失败", JOptionPane.WARNING_MESSAGE);
            return;
        }
        p.setName(nameField.getText().trim());
        p.setGender((String) genderBox.getSelectedItem());
        p.setBirthDate(birthField.getText().trim());
        p.setIdCard(idCardField.getText().trim());
        p.setNativePlace(nativeField.getText().trim());
        p.setAddress(addressField.getText().trim());
        p.setPhone(phoneField.getText().trim());
        p.setRoleId(creating ? 0 : currentRoleOf(p.getId()));
        String plain = new String(passwordField.getPassword());

        try {
            if (creating) {
                service.create(p, plain);
                JOptionPane.showMessageDialog(this, "新增成功");
            } else {
                service.update(p, plain);
                JOptionPane.showMessageDialog(this, "修改成功");
            }
            creating = false;
            passwordField.setText("");
            reload();
        } catch (Exception ex) {
            showError(ex);
        }
    }

    private int currentRoleOf(int id) {
        for (Personnel p : dataRows) {
            if (p.getId() == id) {
                return p.getRoleId();
            }
        }
        return 0;
    }

    private void doDelete() {
        int viewRow = table.getSelectedRow();
        if (viewRow < 0) {
            JOptionPane.showMessageDialog(this, "请先在表格中选择一条记录", "提示", JOptionPane.INFORMATION_MESSAGE);
            return;
        }
        int modelRow = table.convertRowIndexToModel(viewRow);
        if (modelRow < 0 || modelRow >= dataRows.size()) {
            return;
        }
        Personnel p = dataRows.get(modelRow);
        if (p.getId() == currentUser.getId()) {
            JOptionPane.showMessageDialog(this, "不能删除当前登录用户", "提示", JOptionPane.WARNING_MESSAGE);
            return;
        }
        int ok = JOptionPane.showConfirmDialog(this, "确定删除【" + p.getName() + "】吗？",
                "确认删除", JOptionPane.YES_NO_OPTION);
        if (ok != JOptionPane.YES_OPTION) {
            return;
        }
        try {
            service.delete(p.getId());
            reload();
        } catch (Exception ex) {
            showError(ex);
        }
    }

    private void toggleRole() {
        int viewRow = table.getSelectedRow();
        if (viewRow < 0) {
            JOptionPane.showMessageDialog(this, "请先在表格中选择一条记录", "提示", JOptionPane.INFORMATION_MESSAGE);
            return;
        }
        int modelRow = table.convertRowIndexToModel(viewRow);
        if (modelRow < 0 || modelRow >= dataRows.size()) {
            return;
        }
        Personnel p = dataRows.get(modelRow);
        if (p.getId() == currentUser.getId()) {
            JOptionPane.showMessageDialog(this, "不能修改自己的权限", "提示", JOptionPane.WARNING_MESSAGE);
            return;
        }
        int newRole = p.getRoleId() == 1 ? 0 : 1;
        String action = newRole == 1 ? "授予管理员" : "回收管理员";
        int ok = JOptionPane.showConfirmDialog(this, "确定对【" + p.getName() + "】" + action + "吗？",
                "权限变更", JOptionPane.YES_NO_OPTION);
        if (ok != JOptionPane.YES_OPTION) {
            return;
        }
        try {
            service.changeRole(p.getId(), newRole);
            reload();
        } catch (Exception ex) {
            showError(ex);
        }
    }

    private void showError(Exception ex) {
        JOptionPane.showMessageDialog(this, ex.getMessage(), "操作失败", JOptionPane.ERROR_MESSAGE);
    }
}
