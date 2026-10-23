package com.dbms.client.ui;

import com.dbms.client.entity.Personnel;

import javax.swing.*;
import java.awt.*;

/** 主窗口：菜单 + CardLayout 内容区。菜单按 roleId 动态显示（子系统四）。 */
public class MainFrame extends JFrame {

    private final Personnel currentUser;
    private final JPanel content = new JPanel(new CardLayout());

    public MainFrame(Personnel user) {
        this.currentUser = user;
        setTitle("简易仓库管理系统    当前用户：" + user.getName() + "（" + user.roleLabel() + "）");
        // 报表页有图表 + 表格 + 两行工具栏，窄了会挤，默认给足宽度
        setSize(1180, 760);
        setMinimumSize(new Dimension(980, 680));
        setLocationRelativeTo(null);
        setDefaultCloseOperation(EXIT_ON_CLOSE);
        setIconImage(UiTheme.appIcon());

        content.add(new PersonnelPanel(user), "personnel");
        content.add(new MaterialPanel(), "material");
        content.add(new InventoryPanel(user), "inventory");
        content.add(new ReportPanel(), "report");

        setJMenuBar(buildMenuBar());
        add(content, BorderLayout.CENTER);
        showCard("inventory");
    }

    private JMenuBar buildMenuBar() {
        JMenuBar bar = new JMenuBar();

        JMenu archive = new JMenu("档案管理");
        archive.add(menuItem("人员档案", "personnel"));
        archive.add(menuItem("物料档案", "material"));
        bar.add(archive);

        JMenu inout = new JMenu("物料进出仓");
        inout.add(menuItem("进出仓操作与查询", "inventory"));
        bar.add(inout);

        JMenu report = new JMenu("统计报表");
        report.add(menuItem("各类统计与导出", "report"));
        bar.add(report);

        // ---- 动态权限：只有管理员才看得到"权限管理" ----
        if (currentUser.getRoleId() == 1) {
            JMenu admin = new JMenu("权限管理");
            admin.add(menuItem("用户与权限授予", "personnel"));
            bar.add(admin);
        }

        JMenu system = new JMenu("系统");
        JMenuItem logout = new JMenuItem("退出登录");
        logout.addActionListener(e -> {
            dispose();
            new LoginFrame().setVisible(true);
        });
        system.add(logout);
        JMenuItem exit = new JMenuItem("退出程序");
        exit.addActionListener(e -> System.exit(0));
        system.add(exit);
        bar.add(system);

        return bar;
    }

    private JMenuItem menuItem(String text, String card) {
        JMenuItem item = new JMenuItem(text);
        item.addActionListener(e -> showCard(card));
        return item;
    }

    private void showCard(String card) {
        ((CardLayout) content.getLayout()).show(content, card);
    }

    public Personnel getCurrentUser() {
        return currentUser;
    }
}
