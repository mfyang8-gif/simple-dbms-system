package com.dbms.client.service;

import com.dbms.client.dao.PersonnelDao;
import com.dbms.client.entity.Personnel;
import com.dbms.client.util.PasswordUtil;
import com.dbms.client.util.RegexValidator;

import java.time.LocalDate;
import java.util.List;

/** 人员档案业务逻辑：所有输入校验集中在这里，界面层只管把值送进来。 */
public class PersonnelService {

    private final PersonnelDao dao = new PersonnelDao();

    public List<Personnel> search(String keyword) {
        return dao.search(keyword);
    }

    public int nextId() {
        return dao.nextId();
    }

    /** 新增（口令传明文，这里负责加密）。 */
    public void create(Personnel p, String plainPassword) {
        validate(p);
        if (!RegexValidator.isPassword(plainPassword)) {
            throw new IllegalArgumentException("口令需 6-20 位，且同时包含字母和数字");
        }
        if (dao.findById(p.getId()) != null) {
            throw new IllegalArgumentException("人员代码已存在：" + p.getId());
        }
        p.setPassword(PasswordUtil.sha256(plainPassword));
        dao.insert(p);
    }

    /** 修改档案。plainPassword 为 null 或空表示不修改口令。 */
    public void update(Personnel p, String plainPassword) {
        validate(p);
        if (plainPassword != null && !plainPassword.isEmpty()) {
            if (!RegexValidator.isPassword(plainPassword)) {
                throw new IllegalArgumentException("口令需 6-20 位，且同时包含字母和数字");
            }
            dao.updatePassword(p.getId(), PasswordUtil.sha256(plainPassword));
        }
        dao.update(p);
    }

    public void delete(int id) {
        dao.delete(id);
    }

    /** 权限授予 / 回收。roleId：1=管理员，0=普通用户。 */
    public void changeRole(int id, int roleId) {
        dao.updateRole(id, roleId);
    }

    // ---------- 私有校验 ----------

    private void validate(Personnel p) {
        if (p.getId() <= 0) {
            throw new IllegalArgumentException("人员代码必须是正整数");
        }
        if (!RegexValidator.isName(p.getName())) {
            throw new IllegalArgumentException("姓名需为 2-20 位中文或字母");
        }
        if (!RegexValidator.isDate(p.getBirthDate())) {
            throw new IllegalArgumentException("出生日期格式必须是 YYYY-MM-DD");
        }
        if (!isRealDate(p.getBirthDate())) {
            throw new IllegalArgumentException("出生日期不是一个真实存在的日期");
        }
        if (!RegexValidator.isIdCard(p.getIdCard())) {
            throw new IllegalArgumentException("身份证号必须是 18 位，且最后一位为数字或 X");
        }
        if (!RegexValidator.isPhone(p.getPhone())) {
            throw new IllegalArgumentException("联系电话格式不正确");
        }
    }

    private boolean isRealDate(String s) {
        try {
            LocalDate.parse(s.trim());
            return true;
        } catch (Exception e) {
            return false;
        }
    }
}
