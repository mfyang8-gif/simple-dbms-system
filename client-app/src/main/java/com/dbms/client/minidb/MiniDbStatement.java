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

    /**
     * 通用执行入口：<b>原样发送 SQL，不做任何改写</b>，并按协议前缀统一解析。
     *
     * 与 executeQuery / executeUpdate 的区别（控制台必须用这个）：
     * <ol>
     *   <li>不套用 SELECT MAX 改写。改写是给业务 DAO 兜底的 hack；
     *       控制台用于观察引擎真实能力，一旦引擎实现了聚合，改写反而会破坏语义。</li>
     *   <li>executeUpdate 遇到 ROWS 响应时直接返回 0 而<b>不消费结果行</b>，
     *       剩余的表头/数据行会残留在流里，导致后续语句的响应错位；
     *       本方法对两种响应都完整读到结束符，不会破坏协议状态。</li>
     * </ol>
     *
     * 未知前缀不会抛异常，而是以 {@link Kind#RAW} 返回原文，
     * 便于引擎新增响应类型时控制台仍能显示，而不是直接崩掉。
     */
    public RawResult executeRaw(String sql) {
        String header = send(sql);
        if (header.startsWith("ROWS")) {
            String[] parts = header.split("\\s+");
            int rowsCount = parts.length >= 3 ? Integer.parseInt(parts[2]) : 0;

            List<String> columns = new ArrayList<>();
            List<List<String>> rows = new ArrayList<>();
            try {
                String headerLine = in.readLine();
                if (headerLine == null) {
                    throw new MiniDbException("协议错误：结果集缺少表头");
                }
                columns.addAll(Arrays.asList(headerLine.split("\t", -1)));
                for (int r = 0; r < rowsCount; r++) {
                    String rowLine = in.readLine();
                    if (rowLine == null) {
                        throw new MiniDbException("协议错误：结果行缺失（第 " + (r + 1) + " 行）");
                    }
                    List<String> cells = new ArrayList<>();
                    for (String cell : rowLine.split("\t", -1)) {
                        cells.add(decodeCell(cell));
                    }
                    rows.add(cells);
                }
                in.readLine(); // END（容错：缺失不致命）
            } catch (IOException e) {
                throw new MiniDbException("读取结果集失败：" + e.getMessage(), e);
            }
            return RawResult.rows(columns, rows);
        }
        if (header.startsWith("OK")) {
            String[] parts = header.split("\\s+");
            int affected = 0;
            if (parts.length >= 2) {
                try {
                    affected = Integer.parseInt(parts[1]);
                } catch (NumberFormatException ignored) {
                    // 保持 0
                }
            }
            return RawResult.ok(affected);
        }
        if (header.startsWith("ERR")) {
            throw new MiniDbException(unescape(header.substring(3).trim()));
        }
        return RawResult.raw(header);
    }

    /** 通用执行的返回类型。 */
    public enum Kind {
        /** 执行成功，affected = 影响行数 */
        OK,
        /** 结果集 */
        ROWS,
        /** 未识别的响应（前向兼容，原文放在 raw 里） */
        RAW
    }

    /** 通用执行结果：三种协议响应统一成一个对象。 */
    public static final class RawResult {
        public final Kind kind;
        public final List<String> columns;
        public final List<List<String>> rows;
        public final int affected;
        public final String raw;

        private RawResult(Kind kind, List<String> columns, List<List<String>> rows, int affected, String raw) {
            this.kind = kind;
            this.columns = columns;
            this.rows = rows;
            this.affected = affected;
            this.raw = raw;
        }

        static RawResult ok(int affected) {
            return new RawResult(Kind.OK, new ArrayList<>(), new ArrayList<>(), affected, null);
        }

        static RawResult rows(List<String> columns, List<List<String>> rows) {
            return new RawResult(Kind.ROWS, columns, rows, rows.size(), null);
        }

        static RawResult raw(String header) {
            return new RawResult(Kind.RAW, new ArrayList<>(), new ArrayList<>(), 0, header);
        }
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
