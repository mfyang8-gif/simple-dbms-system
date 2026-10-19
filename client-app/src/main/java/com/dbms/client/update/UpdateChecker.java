package com.dbms.client.update;

import java.io.InputStream;
import java.net.URI;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardCopyOption;
import java.util.Properties;

public class UpdateChecker {

    // 当前程序版本：以后每次发版只改这一行
    public static final String CURRENT_VERSION = "1.0.0";

    private static final String SERVER_URL = "http://127.0.0.1:8000/version.txt";

    // 下载目录和待安装文件
    private static final Path UPDATE_DIR = Path.of("updates");
    private static final String NEW_JAR_NAME = "dbms-client.jar.new";

    // 现在正在运行的程序文件（真实部署时如果 jar 位置变了，只改这一行）
    private static final Path CURRENT_JAR = Path.of("target", "dbms-client.jar");

    public static boolean isNewer(String remote, String current) {
        String[] a = remote.split("\\.");
        String[] b = current.split("\\.");
        int n = Math.min(a.length, b.length);
        for (int i = 0; i < n; i++) {
            int x = Integer.parseInt(a[i]);
            int y = Integer.parseInt(b[i]);
            if (x != y) {
                return x > y;
            }
        }
        return a.length > b.length;
    }

    public static class Info {
        public final String version;
        public final String url;
        public final String notes;

        public Info(String version, String url, String notes) {
            this.version = version;
            this.url = url;
            this.notes = notes;
        }
    }

    public static Info fetchLatest() {
        try (InputStream in = URI.create(SERVER_URL).toURL().openStream()) {
            Properties p = new Properties();
            p.load(in);
            return new Info(
                    p.getProperty("version"),
                    p.getProperty("url"),
                    p.getProperty("notes"));
        } catch (Exception e) {
            return null;
        }
    }

    public static Path download(String url) {
        try {
            Files.createDirectories(UPDATE_DIR);
            Path target = UPDATE_DIR.resolve(NEW_JAR_NAME);
            try (InputStream in = URI.create(url).toURL().openStream()) {
                Files.copy(in, target, StandardCopyOption.REPLACE_EXISTING);
            }
            return target;
        } catch (Exception e) {
            return null;
        }
    }

    // 把上次下载的 .new 覆盖到正式 jar 上；没有待安装的更新就返回 false
    public static boolean applyPendingUpdate() {
        Path newJar = UPDATE_DIR.resolve(NEW_JAR_NAME);
        if (!Files.exists(newJar)) {
            return false;
        }
        try {
            Files.move(newJar, CURRENT_JAR, StandardCopyOption.REPLACE_EXISTING);
            return true;
        } catch (Exception e) {
            return false;
        }
    }

    public static void main(String[] args) throws Exception {
        System.out.println("当前版本: " + CURRENT_VERSION);

        // 1. 先安装上次下载的更新（如果有）
        if (applyPendingUpdate()) {
            System.out.println("已安装上次下载的新版本");
        }

        // 2. 检查服务器
        Info info = fetchLatest();
        if (info == null) {
            System.out.println("连不上更新服务器，跳过检查（不影响启动）");
            return;
        }

        // 3. 判断要不要更新
        System.out.println("最新版本: " + info.version);
        if (!isNewer(info.version, CURRENT_VERSION)) {
            System.out.println("已经是最新版");
            return;
        }

        // 4. 下载
        Path file = download(info.url);
        if (file == null) {
            System.out.println("下载失败（不影响使用）");
            return;
        }
        System.out.println("更新已下载，下次启动生效");
    }
}
