package com.dbms.client.util;

import java.util.regex.Pattern;

/**
 * 正则校验工具（子系统一、四要求使用正则表达式验证输入）。
 * 全部用 Pattern 预编译，避免每次校验重复编译。
 */
public final class RegexValidator {

    private RegexValidator() {
    }

    /** 身份证：18 位，前 17 位数字，最后一位数字或 X。 */
    private static final Pattern ID_CARD = Pattern.compile("^\\d{17}[\\dXx]$");

    /** 更严格的身份证（含地区码 + 出生日期范围）。 */
    private static final Pattern ID_CARD_STRICT =
            Pattern.compile("^[1-9]\\d{5}(18|19|20)\\d{2}(0[1-9]|1[0-2])(0[1-9]|[12]\\d|3[01])\\d{3}[\\dXx]$");

    /** 登录账号：本系统用"人员代码"，1-9 位数字。 */
    private static final Pattern ACCOUNT = Pattern.compile("^\\d{1,9}$");

    /** 用户名字符规则：字母开头，4-16 位字母/数字/下划线。 */
    private static final Pattern USERNAME = Pattern.compile("^[a-zA-Z]\\w{3,15}$");

    /** 口令规则：6-20 位，必须同时包含字母和数字，不能有空格。 */
    private static final Pattern PASSWORD = Pattern.compile("^(?=.*[A-Za-z])(?=.*\\d)\\S{6,20}$");

    /** 手机号。 */
    private static final Pattern PHONE = Pattern.compile("^1[3-9]\\d{9}$");

    /** 姓名：2-20 位中文或字母。 */
    private static final Pattern NAME = Pattern.compile("^[\\u4e00-\\u9fa5A-Za-z·]{2,20}$");

    /** 日期格式 YYYY-MM-DD。 */
    private static final Pattern DATE = Pattern.compile("^\\d{4}-\\d{2}-\\d{2}$");

    public static boolean isIdCard(String s) {
        return s != null && ID_CARD.matcher(s.trim()).matches();
    }

    public static boolean isIdCardStrict(String s) {
        return s != null && ID_CARD_STRICT.matcher(s.trim()).matches();
    }

    public static boolean isAccount(String s) {
        return s != null && ACCOUNT.matcher(s.trim()).matches();
    }

    public static boolean isUsername(String s) {
        return s != null && USERNAME.matcher(s.trim()).matches();
    }

    public static boolean isPassword(String s) {
        return s != null && PASSWORD.matcher(s).matches();
    }

    public static boolean isPhone(String s) {
        return s != null && PHONE.matcher(s.trim()).matches();
    }

    public static boolean isName(String s) {
        return s != null && NAME.matcher(s.trim()).matches();
    }

    public static boolean isDate(String s) {
        return s != null && DATE.matcher(s.trim()).matches();
    }

    /** 是不是非负整数（用于数量输入）。 */
    public static boolean isNonNegativeInt(String s) {
        if (s == null || s.trim().isEmpty()) {
            return false;
        }
        try {
            return Integer.parseInt(s.trim()) >= 0;
        } catch (NumberFormatException e) {
            return false;
        }
    }

    /** 是不是正整数。 */
    public static boolean isPositiveInt(String s) {
        if (s == null || s.trim().isEmpty()) {
            return false;
        }
        try {
            return Integer.parseInt(s.trim()) > 0;
        } catch (NumberFormatException e) {
            return false;
        }
    }
}
