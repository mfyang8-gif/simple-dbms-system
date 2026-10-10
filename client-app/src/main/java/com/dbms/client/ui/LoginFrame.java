package com.dbms.client.ui;

import com.dbms.client.entity.Personnel;
import com.dbms.client.service.AuthService;

import javax.swing.*;
import java.awt.*;

/** 登录界面（子系统四）。 */
public class LoginFrame extends JFrame {

    private final AuthService authService = new AuthService();
    private final JTextField accountField = new JTextField(16);
    private final JPasswordField passwordField = new JPasswordField(16);

    public LoginFrame() {
        setTitle("简易仓库管理系统 - 登录");
        setDefaultCloseOperation(EXIT_ON_CLOSE);

        JPanel form = new JPanel(new GridBagLayout());
        GridBagConstraints g = new GridBagConstraints();
        g.insets = new Insets(8, 8, 8, 8);
        g.anchor = GridBagConstraints.WEST;

        g.gridx = 0;
        g.gridy = 0;
        form.add(new JLabel("账号（人员代码）："), g);
        g.gridx = 1;
        form.add(accountField, g);

        g.gridx = 0;
        g.gridy = 1;
        form.add(new JLabel("口令："), g);
        g.gridx = 1;
        form.add(passwordField, g);

        JButton loginButton = new JButton("登 录");
        loginButton.addActionListener(e -> doLogin());

        g.gridx = 0;
        g.gridy = 2;
        g.gridwidth = 2;
        g.anchor = GridBagConstraints.CENTER;
        form.add(loginButton, g);

        add(form, BorderLayout.CENTER);

        JLabel tip = new JLabel("测试账号：1001 / 123456（管理员）  ·  1002 / 123456（普通用户）", SwingConstants.CENTER);
        tip.setBorder(BorderFactory.createEmptyBorder(0, 10, 12, 10));
        add(tip, BorderLayout.SOUTH);

        getRootPane().setDefaultButton(loginButton);   // 回车即登录
        pack();
        setLocationRelativeTo(null);
    }

    private void doLogin() {
        try {
            Personnel user = authService.login(
                    accountField.getText(), new String(passwordField.getPassword()));
            if (user == null) {
                JOptionPane.showMessageDialog(this, "账号或口令错误", "登录失败", JOptionPane.ERROR_MESSAGE);
                passwordField.setText("");
                return;
            }
            dispose();
            new MainFrame(user).setVisible(true);
        } catch (Exception ex) {
            JOptionPane.showMessageDialog(this, ex.getMessage(), "错误", JOptionPane.ERROR_MESSAGE);
        }
    }
}
