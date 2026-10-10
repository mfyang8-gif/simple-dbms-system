package com.dbms.client.entity;

/** 进出仓记录，对应表 inventory_logs。 */
public class InventoryLog {

    private int logId;           // log_id        流水单号（主键）
    private int materialCode;    // material_code 物料代码
    private String opType;       // op_type       I=进仓, O=出仓
    private int qty;             // qty           操作数量
    private String opDate;       // op_date       日期 YYYY-MM-DD
    private int operatorId;      // operator_id   操作人员 ID
    private String remarks;      // remarks       备注

    public InventoryLog() {
    }

    public int getLogId() {
        return logId;
    }

    public void setLogId(int logId) {
        this.logId = logId;
    }

    public int getMaterialCode() {
        return materialCode;
    }

    public void setMaterialCode(int materialCode) {
        this.materialCode = materialCode;
    }

    public String getOpType() {
        return opType;
    }

    public void setOpType(String opType) {
        this.opType = opType;
    }

    public int getQty() {
        return qty;
    }

    public void setQty(int qty) {
        this.qty = qty;
    }

    public String getOpDate() {
        return opDate;
    }

    public void setOpDate(String opDate) {
        this.opDate = opDate;
    }

    public int getOperatorId() {
        return operatorId;
    }

    public void setOperatorId(int operatorId) {
        this.operatorId = operatorId;
    }

    public String getRemarks() {
        return remarks;
    }

    public void setRemarks(String remarks) {
        this.remarks = remarks;
    }

    public String opTypeLabel() {
        return "I".equalsIgnoreCase(opType) ? "进仓" : "出仓";
    }
}
