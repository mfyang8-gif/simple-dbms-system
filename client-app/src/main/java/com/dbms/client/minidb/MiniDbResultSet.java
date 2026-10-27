package com.dbms.client.minidb;

import java.util.List;

/**
 * 极简结果集：列名列表 + 行列表（每格为 String 或 null 表示 SQL NULL）。
 * MiniDB 协议层把 INT/VARCHAR 都作为字符串返回，调用方按需用 getInt 解析。
 */
public class MiniDbResultSet implements AutoCloseable {

    private final List<String> columns;
    private final List<List<String>> rows;
    private int cursor = -1;
    private boolean lastWasNull = false;

    public MiniDbResultSet(List<String> columns, List<List<String>> rows) {
        this.columns = columns;
        this.rows = rows;
    }

    public boolean next() {
        cursor++;
        return cursor < rows.size();
    }

    private int findColumn(String name) {
        for (int i = 0; i < columns.size(); i++) {
            if (columns.get(i).equalsIgnoreCase(name)) {
                return i;
            }
        }
        throw new MiniDbException("未知列名：" + name);
    }

    private void checkCursor(int index) {
        if (cursor < 0 || cursor >= rows.size()) {
            throw new MiniDbException("结果集游标越界（未调用 next() 或已到末尾）");
        }
        if (index < 1 || index > columns.size()) {
            throw new MiniDbException("列下标越界：" + index);
        }
    }

    public String getString(int index) {
        checkCursor(index);
        String v = rows.get(cursor).get(index - 1);
        lastWasNull = (v == null);
        return v;
    }

    public String getString(String col) {
        return getString(findColumn(col) + 1);
    }

    public int getInt(int index) {
        String v = getString(index);
        if (v == null) {
            return 0;
        }
        try {
            return Integer.parseInt(v.trim());
        } catch (NumberFormatException e) {
            throw new MiniDbException("列值不是合法整数：" + v);
        }
    }

    public int getInt(String col) {
        return getInt(findColumn(col) + 1);
    }

    public boolean wasNull() {
        return lastWasNull;
    }

    @Override
    public void close() {
        // 结果集不持有独立资源
    }
}
