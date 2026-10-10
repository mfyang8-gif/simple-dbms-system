package com.dbms.client.entity;

/** 物料档案，对应表 materials。 */
public class Material {

    private int materialCode;   // material_code 物料代码（主键）
    private String name;        // name          物料名称
    private String spec;        // spec          规格
    private String unit;        // unit          计量单位
    private int stockQty;       // stock_qty     当前库存数量

    public Material() {
    }

    public int getMaterialCode() {
        return materialCode;
    }

    public void setMaterialCode(int materialCode) {
        this.materialCode = materialCode;
    }

    public String getName() {
        return name;
    }

    public void setName(String name) {
        this.name = name;
    }

    public String getSpec() {
        return spec;
    }

    public void setSpec(String spec) {
        this.spec = spec;
    }

    public String getUnit() {
        return unit;
    }

    public void setUnit(String unit) {
        this.unit = unit;
    }

    public int getStockQty() {
        return stockQty;
    }

    public void setStockQty(int stockQty) {
        this.stockQty = stockQty;
    }
}
