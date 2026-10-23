package com.dbms.client.ui;

import com.dbms.client.entity.Personnel;
import com.dbms.client.service.AuthService;

import javax.swing.*;
import java.awt.*;

/**
 * 登录界面（子系统四）。
 *
 * 【第 10 课】界面走查：
 *   ① 顶部加标题区（系统名 + 副标题），原来是光秃秃一个表单；
 *   ② 窗口和应用统一图标，任务栏里认得出来；
 *   ③ 打开就聚焦账号框，输入完口令直接回车登录；
 *   ④ 尺寸按 pack 算，不写死，换字体也不会被裁。
 */
public class LoginFrame extends JFrame {

    private final AuthService authService = new AuthService();
    private final JTextField accountField = new JTextField(16);
    private final JPasswordField passwordField = new JPasswordField(16);

    public LoginFrame() {
        setTitle("简易仓库管理系统 - 登录");
        setDefaultCloseOperation(EXIT_ON_CLOSE);
        setIconImage(UiTheme.appIcon());

        add(buildHeader(), BorderLayout.NORTH);
        add(buildForm(), BorderLayout.CENTER);
        add(buildFooter(), BorderLayout.SOUTH);

        pack();
        setResizable(false);
        setLocationRelativeTo(null);

        // 打开即可输入，不用先用鼠标点一下账号框
        SwingUtilities.invokeLater(accountField::requestFocusInWindow);
    }

    private JComponent buildHeader() {
        JPanel header = new JPanel();
        header.setLayout(new BoxLayout(header, BoxLayout.Y_AXIS));
        header.setBorder(BorderFactory.createEmptyBorder(26, 30, 14, 30));

        JLabel title = new JLabel("简易仓库管理系统");
        title.setFont(UiTheme.APP_TITLE);
        title.setAlignmentX(Component.CENTER_ALIGNMENT);

        JLabel subtitle = UiTheme.muted("数据库课程设计 · 桌面客户端");
        subtitle.setAlignmentX(Component.CENTER_ALIGNMENT);

        header.add(title);
        header.add(Box.createVerticalStrut(6));
        header.add(subtitle);
        return header;
    }

    private JComponent buildForm() {
        JPanel form = new JPanel(new GridBagLayout());
        form.setBorder(BorderFactory.createEmptyBorder(6, 30, 10, 30));

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
        loginButton.setPreferredSize(new Dimension(120, 32));
        loginButton.addActionListener(e -> doLogin());

        g.gridx = 0;
        g.gridy = 2;
        g.gridwidth = 2;
        g.anchor = GridBagConstraints.CENTER;
        g.insets = new Insets(14, 8, 4, 8);
        form.add(loginButton, g);

        // 回车即登录：口令框里按回车和点按钮等价
        getRootPane().setDefaultButton(loginButton);
        return form;
    }

    private JComponent buildFooter() {
        JPanel footer = new JPanel(new BorderLayout());
        footer.setBorder(BorderFactory.createEmptyBorder(0, 16, 14, 16));
        JLabel tip = UiTheme.muted("测试账号：1001 / 123456（管理员）　1002 / 123456（普通用户）");
        tip.setHorizontalAlignment(SwingConstants.CENTER);
        footer.add(tip, BorderLayout.CENTER);
        return footer;
    }

    private void doLogin() {
        try {
            Personnel user = authService.login(
                    accountField.getText(), new String(passwordField.getPassword()));
            if (user == null) {
                JOptionPane.showMessageDialog(this, "账号或口令错误", "登录失败",
                        JOptionPane.ERROR_MESSAGE);
                passwordField.setText("");
                passwordField.requestFocusInWindow();
                return;
            }
            dispose();
            new MainFrame(user).setVisible(true);
        } catch (Exception ex) {
            JOptionPane.showMessageDialog(this, ex.getMessage(), "错误", JOptionPane.ERROR_MESSAGE);
        }
    }
}
