package com.dbms.client.console;

import java.util.ArrayList;
import java.util.List;

/**
 * SQL 文本切分器。
 *
 * 存在的理由（这是 MiniDB 的硬约束，不是排版偏好）：
 *   MiniDB 的协议是「一行 = 一条语句」，多行语句会被拆成残句后逐行解析而失败。
 *   用户在编辑器里习惯写多行、并用分号分隔多条语句，因此发送前必须做两件事：
 *     1. 按分号切成独立语句；
 *     2. 把每条语句内部的换行压成空格，保证一条语句只占一行。
 *
 * 切分时会跳过引号内的分号与注释，避免把 {@code 'a;b'} 或 {@code -- 说明; 说明} 切开。
 *
 * 已知局限：字符串字面量内部若含真实换行，压缩空白会把它变成空格。
 * 这是引擎「一行一语句」约束带来的必然代价；需要存换行时请用转义或由客户端编码。
 */
public final class SqlSplitter {

    private SqlSplitter() {
    }

    /**
     * 把用户输入切成若干条可直接发送的单行 SQL。
     * 空语句、纯注释会被丢弃。
     */
    public static List<String> split(String text) {
        List<String> out = new ArrayList<>();
        if (text == null || text.isEmpty()) {
            return out;
        }

        StringBuilder current = new StringBuilder();
        char quote = 0;
        boolean lineComment = false;
        boolean blockComment = false;

        for (int i = 0; i < text.length(); i++) {
            char c = text.charAt(i);
            char next = (i + 1 < text.length()) ? text.charAt(i + 1) : '\0';

            if (lineComment) {
                if (c == '\n') {
                    lineComment = false;
                    current.append(' ');
                }
                continue;
            }
            if (blockComment) {
                if (c == '*' && next == '/') {
                    blockComment = false;
                    i++;
                    current.append(' ');
                }
                continue;
            }
            if (quote != 0) {
                current.append(c);
                if (c == quote) {
                    if (next == quote) {
                        // SQL 用双写表示转义：'' 或 ""
                        current.append(next);
                        i++;
                    } else {
                        quote = 0;
                    }
                } else if (c == '\\' && next != '\0') {
                    current.append(next);
                    i++;
                }
                continue;
            }

            if (c == '-' && next == '-') {
                lineComment = true;
                i++;
                continue;
            }
            if (c == '/' && next == '*') {
                blockComment = true;
                i++;
                continue;
            }
            if (c == '\'' || c == '"') {
                quote = c;
                current.append(c);
                continue;
            }
            if (c == ';') {
                flush(current, out);
                continue;
            }
            current.append(c);
        }

        flush(current, out);
        return out;
    }

    private static void flush(StringBuilder current, List<String> out) {
        String statement = collapse(current.toString());
        current.setLength(0);
        if (!statement.isEmpty()) {
            out.add(statement);
        }
    }

    /** 去掉首尾空白，并把内部连续空白（含换行）压成单个空格。 */
    static String collapse(String s) {
        if (s == null) {
            return "";
        }
        StringBuilder sb = new StringBuilder(s.length());
        boolean pendingSpace = false;
        for (int i = 0; i < s.length(); i++) {
            char c = s.charAt(i);
            if (Character.isWhitespace(c)) {
                pendingSpace = sb.length() > 0;
            } else {
                if (pendingSpace) {
                    sb.append(' ');
                    pendingSpace = false;
                }
                sb.append(c);
            }
        }
        return sb.toString().trim();
    }

    /** 取首个关键字（大写），用于识别 BEGIN/COMMIT/ROLLBACK 与 DDL 类型。 */
    public static String firstKeyword(String sql) {
        if (sql == null) {
            return "";
        }
        String s = sql.trim();
        int end = 0;
        while (end < s.length() && (Character.isLetterOrDigit(s.charAt(end)) || s.charAt(end) == '_')) {
            end++;
        }
        return s.substring(0, end).toUpperCase();
    }
}
