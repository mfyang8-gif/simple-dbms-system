package com.dbms.client.util;

import java.io.InputStream;
import java.nio.file.Files;
import java.nio.file.Path;
import java.sql.Connection;
import java.sql.DriverManager;
import java.sql.SQLException;
import java.util.Properties;

/**
 * 数据库连接工具。
 * 连接信息全部来自 config/db.properties，切换 MySQL / Rust 引擎只改配置文件，不动代码。
 */
public final class DbUtil {

    private static final Properties CFG = new Properties();

    static {
        try {
            Path file = Path.of("config", "db.properties");
            if (Files.exists(file)) {
                try (InputStream in = Files.newInputStream(file)) {
                    CFG.load(in);
                }
            } else {
                try (InputStream in = DbUtil.class.getResourceAsStream("/db.properties")) {
                    if (in != null) {
                        CFG.load(in);
                    }
                }
            }
            // 本机私有配置覆盖（含密码，不进仓库）：存在则覆盖同名项
            Path local = Path.of("config", "db.local.properties");
            if (Files.exists(local)) {
                try (InputStream in = Files.newInputStream(local)) {
                    CFG.load(in);
                }
            }
            String driver = CFG.getProperty("db.driver", "com.mysql.cj.jdbc.Driver");
            Class.forName(driver);
        } catch (Exception e) {
            throw new IllegalStateException("加载数据库配置失败，请检查 config/db.properties：" + e.getMessage(), e);
        }
    }

    private DbUtil() {
    }

    public static Connection getConnection() throws SQLException {
        return DriverManager.getConnection(
                CFG.getProperty("db.url"),
                CFG.getProperty("db.user"),
                CFG.getProperty("db.password"));
    }

    public static String currentUrl() {
        return CFG.getProperty("db.url");
    }

    public static void closeQuietly(AutoCloseable... resources) {
        for (AutoCloseable c : resources) {
            if (c != null) {
                try {
                    c.close();
                } catch (Exception ignored) {
                }
            }
        }
    }
}
