package com.dbms.client;

import com.dbms.client.ui.LoginFrame;

import javax.swing.*;

/**
 * 程序入口。
 * 课设要求程序命名遵循"班级+座号+程序名"，jar 名在 pom.xml 的 finalName 里改。
 */
public class App {

    public static void main(String[] args) {
        SwingUtilities.invokeLater(() -> {
            setupLookAndFeel();
            new LoginFrame().setVisible(true);
        });
    }

    private static void setupLookAndFeel() {
        try {
            com.formdev.flatlaf.FlatLightLaf.setup();
        } catch (Throwable ignore) {
            try {
                UIManager.setLookAndFeel(UIManager.getSystemLookAndFeelClassName());
            } catch (Exception ignored) {
            }
        }
    }
}
