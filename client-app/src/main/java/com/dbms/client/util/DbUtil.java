package com.dbms.client.util;

import com.dbms.client.minidb.MiniDbConnection;

import java.io.InputStream;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Properties;

/**
 * 数据库连接工具。
 *
 * MiniDB（Rust 引擎）是自定义 TCP 文本协议，不走 JDBC；
 * 连接信息来自 config/db.properties 的 db.host / db.port。
 * 切换 MySQL / Rust 引擎只改配置文件，不动业务代码。
 */
public final class DbUtil {

    private static final Properties CFG = new Properties();
    private static String HOST = "127.0.0.1";
    private static int PORT = 9000;

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
            HOST = CFG.getProperty("db.host", HOST);
            PORT = Integer.parseInt(CFG.getProperty("db.port", "9000"));
        } catch (Exception e) {
            throw new IllegalStateException("加载数据库配置失败，请检查 config/db.properties：" + e.getMessage(), e);
        }
    }

    private DbUtil() {
    }

    public static MiniDbConnection getMiniDbConnection() {
        return new MiniDbConnection(HOST, PORT);
    }

    public static String currentUrl() {
        return "minidb://" + HOST + ":" + PORT;
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
