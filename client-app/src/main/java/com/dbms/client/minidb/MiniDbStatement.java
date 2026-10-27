package com.dbms.client.minidb;

import java.io.BufferedReader;
import java.io.IOException;
import java.io.PrintWriter;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

/**
 * 单连接上的语句。负责把 SQL 发给 MiniDB，并解析其文本协议：
 *   OK &lt;n&gt;              → 成功 / 影响行数
 *   ROWS &lt;列数&gt; &lt;行数&gt;   → 结果集（随后是 TAB 表头、若干 TAB 行、END）
 *   ERR &lt;消息&gt;          → 错误
 *
 * 兼容处理：MiniDB 尚未实现 SELECT MAX()，这里把
 *   SELECT MAX(x) FROM t [WHERE ...]
 * 透明改写成
 *   SELECT x FROM t [WHERE ...] ORDER BY x DESC
 * 取首行即最大值，业务 DAO 无需改动。
 */
public class MiniDbStatement implements AutoCloseable {

    private static final Pattern MAX_PATTERN = Pattern.compile(
            "^\\s*SELECT\\s+MAX\\s*\\(\\s*([\\w.]+)\\s*\\)\\s+FROM\\s+(\\w+)(.*)$",
            Pattern.CASE_INSENSITIVE | Pattern.DOTALL);

    private final BufferedReader in;
    private final PrintWriter out;

    MiniDbStatement(BufferedReader in, PrintWriter out) {
        this.in = in;
        this.out = out;
    }

    public MiniDbResultSet executeQuery(String sql) {
        return parseResultSet(send(rewriteMax(sql)));
    }

    public int executeUpdate(String sql) {
        return parseUpdateCount(send(rewriteMax(sql)));
    }

    private String rewriteMax(String sql) {
        Matcher m = MAX_PATTERN.matcher(sql);
        if (m.find()) {
            String col = m.group(1);
            String table = m.group(2);
            String rest = m.group(3) == null ? "" : m.group(3);
            return "SELECT " + col + " FROM " + table + " " + rest + " ORDER BY " + col + " DESC";
        }
        return sql;
    }

    private String send(String sql) {
        out.print(sql);
        out.print("\n");
        out.flush();
        try {
            String line = in.readLine();
            if (line == null) {
                throw new MiniDbException("与 MiniDB 服务端连接中断");
            }
            return line;
        } catch (IOException e) {
            throw new MiniDbException("读取 MiniDB 响应失败：" + e.getMessage(), e);
        }
    }

    private int parseUpdateCount(String header) {
        if (header.startsWith("OK")) {
            String[] parts = header.split("\\s+");
            if (parts.length >= 2) {
                try {
                    return Integer.parseInt(parts[1]);
                } catch (NumberFormatException ignored) {
                    return 0;
                }
            }
            return 0;
        }
        if (header.startsWith("ROWS")) {
            return 0;
        }
        if (header.startsWith("ERR")) {
            throw new MiniDbException(unescape(header.substring(3).trim()));
        }
        throw new MiniDbException("无法解析的响应：" + header);
    }

    private MiniDbResultSet parseResultSet(String header) {
        if (header.startsWith("ERR")) {
            throw new MiniDbException(unescape(header.substring(3).trim()));
        }
        if (!header.startsWith("ROWS")) {
            return new MiniDbResultSet(new ArrayList<>(), new ArrayList<>());
        }
        String[] parts = header.split("\\s+");
        int rowsCount = Integer.parseInt(parts[2]);

        List<String> columns;
        try {
            String headerLine = in.readLine();
            if (headerLine == null) {
                throw new MiniDbException("协议错误：缺少表头");
            }
            columns = new ArrayList<>(Arrays.asList(headerLine.split("\t", -1)));
        } catch (IOException e) {
            throw new MiniDbException("读取表头失败：" + e.getMessage(), e);
        }

        List<List<String>> rows = new ArrayList<>();
        try {
            for (int r = 0; r < rowsCount; r++) {
                String rowLine = in.readLine();
                if (rowLine == null) {
                    throw new MiniDbException("协议错误：结果行缺失");
                }
                List<String> cells = new ArrayList<>();
                for (String cell : rowLine.split("\t", -1)) {
                    cells.add(decodeCell(cell));
                }
                rows.add(cells);
            }
            // 末尾应为 END（容错：缺失也不致命）
            in.readLine();
        } catch (IOException e) {
            throw new MiniDbException("读取结果行失败：" + e.getMessage(), e);
        }
        return new MiniDbResultSet(columns, rows);
    }

    /**
     * 协议层解码：\N → null；\\ → \；\t \n \r → 对应控制字符。
     * 字面量 "\N"（两个字符）在协议里被编码为 "\\N"，解码后还原为 "\N"，不会与 NULL 撞车。
     */
    private String decodeCell(String cell) {
        if (cell.equals("\\N")) {
            return null;
        }
        StringBuilder sb = new StringBuilder();
        int i = 0;
        int n = cell.length();
        while (i < n) {
            char c = cell.charAt(i);
            if (c == '\\' && i + 1 < n) {
                char nx = cell.charAt(i + 1);
                switch (nx) {
                    case 't': sb.append('\t'); i += 2; break;
                    case 'n': sb.append('\n'); i += 2; break;
                    case 'r': sb.append('\r'); i += 2; break;
                    case '\\': sb.append('\\'); i += 2; break;
                    case 'N': sb.append("\\N"); i += 2; break;
                    default: sb.append('\\'); i += 1; break;
                }
            } else {
                sb.append(c);
                i += 1;
            }
        }
        return sb.toString();
    }

    private String unescape(String s) {
        return s == null ? "" : decodeCell(s);
    }

    @Override
    public void close() {
        // 语句本身不持有独立资源，连接关闭时统一回收
    }
}
