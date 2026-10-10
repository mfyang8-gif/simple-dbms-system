package com.dbms.client.ui;

import com.dbms.client.entity.Personnel;
import com.dbms.client.service.PersonnelService;

import javax.swing.*;
import javax.swing.table.DefaultTableModel;
import java.awt.*;
import java.util.ArrayList;
import java.util.List;

/** 人员档案管理（子系统一 + 子系统四的权限授予/回收）。 */
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

    private boolean creating = false;

    public PersonnelPanel(Personnel currentUser) {
        this.currentUser = currentUser;
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

    // ---------------- 顶部工具栏 ----------------

    private JComponent buildToolbar() {
        JPanel panel = new JPanel(new FlowLayout(FlowLayout.LEFT, 8, 4));
        panel.add(new JLabel("关键字："));
        panel.add(keywordField);
        panel.add(new JLabel("（姓名 / 籍贯 / 电话 子串模糊）"));

        JButton searchButton = new JButton("查询");
        searchButton.addActionListener(e -> reload());
        panel.add(searchButton);

        JButton newButton = new JButton("新增");
        newButton.addActionListener(e -> startCreate());
        panel.add(newButton);

        JButton deleteButton = new JButton("删除");
        deleteButton.addActionListener(e -> doDelete());
        panel.add(deleteButton);

        // 只有管理员能授予/回收权限 —— 动态控制控件可见性
        if (currentUser.getRoleId() == 1) {
            JButton roleButton = new JButton("授予/回收管理员");
            roleButton.addActionListener(e -> toggleRole());
            panel.add(roleButton);
        }

        keywordField.addActionListener(e -> reload());
        return panel;
    }

    // ---------------- 底部表单 ----------------

    private JComponent buildForm() {
        JPanel form = new JPanel(new GridBagLayout());
        form.setBorder(BorderFactory.createTitledBorder("档案明细"));
        GridBagConstraints g = new GridBagConstraints();
        g.insets = new Insets(4, 6, 4, 6);
        g.anchor = GridBagConstraints.WEST;

        addField(form, g, 0, "人员代码：", idField);
        addField(form, g, 1, "姓名：", nameField);
        addField(form, g, 2, "性别：", genderBox);
        addField(form, g, 3, "出生日期：", birthField);

        addField(form, g, 4, "身份证号：", idCardField);
        addField(form, g, 5, "籍贯：", nativeField);
        addField(form, g, 6, "家庭住址：", addressField);
        addField(form, g, 7, "联系电话：", phoneField);

        JPanel center = new JPanel(new FlowLayout(FlowLayout.CENTER, 10, 4));
        center.add(new JLabel("设置口令："));
        center.add(passwordField);
        center.add(new JLabel("（新增必填；修改时留空则不改；6-20 位含字母和数字）"));

        JButton saveButton = new JButton("保存");
        saveButton.addActionListener(e -> doSave());
        center.add(saveButton);

        JPanel wrapper = new JPanel(new BorderLayout());
        wrapper.add(form, BorderLayout.CENTER);
        wrapper.add(center, BorderLayout.SOUTH);
        return wrapper;
    }

    private void addField(JPanel panel, GridBagConstraints g, int row, String label, JComponent field) {
        g.gridx = 0;
        g.gridy = row;
        panel.add(new JLabel(label), g);
        g.gridx = 1;
        panel.add(field, g);
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
    }

    private void fillFormFromSelection() {
        int row = table.getSelectedRow();
        if (row < 0 || row >= dataRows.size()) {
            return;
        }
        Personnel p = dataRows.get(row);
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
    }

    private void doSave() {
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
        int row = table.getSelectedRow();
        if (row < 0 || row >= dataRows.size()) {
            JOptionPane.showMessageDialog(this, "请先在表格中选择一条记录", "提示", JOptionPane.INFORMATION_MESSAGE);
            return;
        }
        Personnel p = dataRows.get(row);
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
        int row = table.getSelectedRow();
        if (row < 0 || row >= dataRows.size()) {
            JOptionPane.showMessageDialog(this, "请先在表格中选择一条记录", "提示", JOptionPane.INFORMATION_MESSAGE);
            return;
        }
        Personnel p = dataRows.get(row);
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
