package com.dbms.client.util;

/**
 * 手搓 SQL 字面量工具。
 *
 * 为什么不用 PreparedStatement？
 * 因为底层的 Rust 引擎是"纯手搓极简实现"，很可能不支持预编译语句的
 * 参数绑定与元数据交互（PreparedStatement 会走一套额外的协议流程），
 * 用普通 Statement + 手拼 SQL 最稳，也和大家规范文档里的示例写法一致。
 *
 * ⚠️ 代价是失去了防注入保护，所以所有字符串必须经过这里的 lit() 转义，
 *    绝对不要把用户输入直接拼进 SQL。
 */
public final class SqlUtil {

    private SqlUtil() {
    }

    /** 把字符串转成安全的 SQL 字面量（自动加单引号并转义）。 */
    public static String lit(String s) {
        if (s == null) {
            return "NULL";
        }
        return "'" + s.replace("\\", "\\\\").replace("'", "''") + "'";
    }

    public static String lit(int v) {
        return Integer.toString(v);
    }

    /** 生成 "%关键字%" 形式的模糊匹配字面量。 */
    public static String like(String keyword) {
        return lit("%" + (keyword == null ? "" : keyword.trim()) + "%");
    }

    /** 生成 "关键字%" 形式的前缀匹配字面量。 */
    public static String prefix(String keyword) {
        return lit((keyword == null ? "" : keyword.trim()) + "%");
    }
}
