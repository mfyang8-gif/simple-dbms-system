package com.dbms.client.util;

import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;

/**
 * 口令加密工具：SHA-256，输出 64 位十六进制，正好塞进 VARCHAR(64)。
 * 数据库里永远只存密文，登录时把用户输入加密后再比对。
 */
public final class PasswordUtil {

    private PasswordUtil() {
    }

    public static String sha256(String raw) {
        if (raw == null) {
            raw = "";
        }
        try {
            MessageDigest md = MessageDigest.getInstance("SHA-256");
            byte[] digest = md.digest(raw.getBytes(StandardCharsets.UTF_8));
            StringBuilder sb = new StringBuilder(64);
            for (byte b : digest) {
                sb.append(Character.forDigit((b >> 4) & 0xF, 16));
                sb.append(Character.forDigit(b & 0xF, 16));
            }
            return sb.toString();
        } catch (Exception e) {
            throw new IllegalStateException("口令加密失败", e);
        }
    }
}
