package com.dbms.client.service;

import com.dbms.client.dao.PersonnelDao;
import com.dbms.client.entity.Personnel;
import com.dbms.client.util.PasswordUtil;
import com.dbms.client.util.RegexValidator;

/** 登录与身份校验。 */
public class AuthService {

    private final PersonnelDao personnelDao = new PersonnelDao();

    /** 登录成功返回该人员对象（含 roleId），失败返回 null。 */
    public Personnel login(String account, String rawPassword) {
        if (!RegexValidator.isAccount(account)) {
            throw new IllegalArgumentException("账号必须是 1-9 位数字（人员代码）");
        }
        if (rawPassword == null || rawPassword.isEmpty()) {
            throw new IllegalArgumentException("请输入口令");
        }
        Personnel p = personnelDao.findById(Integer.parseInt(account.trim()));
        if (p == null) {
            return null;
        }
        return PasswordUtil.sha256(rawPassword).equalsIgnoreCase(p.getPassword()) ? p : null;
    }
}
