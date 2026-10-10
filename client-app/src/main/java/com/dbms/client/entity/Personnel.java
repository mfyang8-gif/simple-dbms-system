package com.dbms.client.entity;

/** 人员档案，对应表 personnel。字段名、类型严格按《核心数据表结构字典》。 */
public class Personnel {

    private int id;              // id          人员代码（主键）
    private String name;         // name        姓名
    private String gender;       // gender      性别
    private String birthDate;    // birth_date  出生日期 YYYY-MM-DD
    private String idCard;       // id_card     身份证号
    private String nativePlace;  // native_place 籍贯
    private String address;      // address     家庭住址
    private String phone;        // phone       联系电话
    private String password;     // password    加密存储的口令
    private int roleId;          // role_id     0=普通, 1=管理

    public Personnel() {
    }

    public int getId() {
        return id;
    }

    public void setId(int id) {
        this.id = id;
    }

    public String getName() {
        return name;
    }

    public void setName(String name) {
        this.name = name;
    }

    public String getGender() {
        return gender;
    }

    public void setGender(String gender) {
        this.gender = gender;
    }

    public String getBirthDate() {
        return birthDate;
    }

    public void setBirthDate(String birthDate) {
        this.birthDate = birthDate;
    }

    public String getIdCard() {
        return idCard;
    }

    public void setIdCard(String idCard) {
        this.idCard = idCard;
    }

    public String getNativePlace() {
        return nativePlace;
    }

    public void setNativePlace(String nativePlace) {
        this.nativePlace = nativePlace;
    }

    public String getAddress() {
        return address;
    }

    public void setAddress(String address) {
        this.address = address;
    }

    public String getPhone() {
        return phone;
    }

    public void setPhone(String phone) {
        this.phone = phone;
    }

    public String getPassword() {
        return password;
    }

    public void setPassword(String password) {
        this.password = password;
    }

    public int getRoleId() {
        return roleId;
    }

    public void setRoleId(int roleId) {
        this.roleId = roleId;
    }

    public String roleLabel() {
        return roleId == 1 ? "管理员" : "普通用户";
    }
}
