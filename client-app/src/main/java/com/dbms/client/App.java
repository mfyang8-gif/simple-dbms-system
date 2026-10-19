package com.dbms.client;

import com.dbms.client.ui.LoginFrame;
import com.dbms.client.update.UpdateChecker;

import javax.swing.*;

/**
 * 程序入口。
 * 课设要求程序命名遵循"班级+座号+程序名"，jar 名在 pom.xml 的 finalName 里改。
 */
public class App {

    public static void main(String[] args) {
        // 自动升级 ①：先把上次下载好的新版本装上（失败不影响启动）
        // Windows 下正在运行的 jar 无法被覆盖，真正可靠的替换点在 run.bat 里；
        // 这里保留调用，兼容 Linux/macOS，且 Windows 上失败只会返回 false。
        UpdateChecker.applyPendingUpdate();

        // 自动升级 ②：后台守护线程检查新版本，绝不阻塞登录界面
        startUpdateCheck();

        SwingUtilities.invokeLater(() -> {
            setupLookAndFeel();
            new LoginFrame().setVisible(true);
        });
    }

    /** 后台检查更新：连不上服务器、下载失败都静默跳过，不影响正常使用。 */
    private static void startUpdateCheck() {
        Thread t = new Thread(() -> {
            UpdateChecker.Info info = UpdateChecker.fetchLatest();
            if (info == null || info.version == null || info.url == null) {
                return;                                   // 没有更新服务器 / 连不上
            }
            if (!UpdateChecker.isNewer(info.version, UpdateChecker.CURRENT_VERSION)) {
                return;                                   // 已是最新版
            }
            UpdateChecker.download(info.url);             // 下载到 updates/，下次启动前替换
        }, "update-check");
        t.setDaemon(true);                                // 守护线程：主窗口关闭即随之退出
        t.start();
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
