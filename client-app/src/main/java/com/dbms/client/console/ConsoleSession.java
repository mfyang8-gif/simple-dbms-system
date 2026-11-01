package com.dbms.client.console;

import com.dbms.client.minidb.MiniDbConnection;
import com.dbms.client.minidb.MiniDbException;
import com.dbms.client.minidb.MiniDbStatement;

import java.util.ArrayList;
import java.util.List;

/**
 * 控制台与 MiniDB 之间的一层薄封装。
 *
 * 职责边界：
 *   - 只负责「连接生命周期 + 单条 SQL 执行 + 计时 + 事务状态跟踪」；
 *   - 不做任何 SQL 改写（{@link MiniDbStatement#executeRaw} 原样发送），
 *     控制台的用途就是观察引擎的真实行为，任何改写都会掩盖引擎缺陷；
 *   - 不吞异常：协议错误转成 {@link Exec#error} 回传给界面显示原文。
 *
 * 为什么一条连接对应一个会话：MiniDB 的事务语义是「一连接一会话」，
 * BEGIN/COMMIT/ROLLBACK 都作用在当前连接上，因此控制台必须独占一条连接，
 * 不能在每次执行时新建连接（那样 BEGIN 之后的语句就落到别的会话里了）。
 */
public class ConsoleSession implements AutoCloseable {

    private final String host;
    private final int port;

    private MiniDbConnection connection;
    private MiniDbStatement statement;
    private boolean inTransaction;

    public ConsoleSession(String host, int port) {
        this.host = host;
        this.port = port;
        connect();
    }

    /** 建立（或重建）连接。失败抛出 MiniDbException，由界面捕获并提示。 */
    public void connect() {
        close();
        connection = new MiniDbConnection(host, port);
        statement = connection.createStatement();
        inTransaction = false;
    }

    public boolean isConnected() {
        return connection != null && statement != null;
    }

    public boolean isInTransaction() {
        return inTransaction;
    }

    public String target() {
        return host + ":" + port;
    }

    /**
     * 执行一条（只一条）SQL。任何异常都收敛成 {@link Exec#error}，不向界面抛。
     *
     * @param sql 单条语句，不允许包含分号分隔的多条语句（由 {@link SqlSplitter} 保证）
     */
    public Exec execute(String sql) {
        long start = System.nanoTime();
        try {
            if (!isConnected()) {
                connect();
            }
            MiniDbStatement.RawResult r = statement.executeRaw(sql);
            long ms = (System.nanoTime() - start) / 1_000_000L;
            updateTxnState(sql, true);
            return Exec.fromRaw(sql, r, ms);
        } catch (MiniDbException e) {
            long ms = (System.nanoTime() - start) / 1_000_000L;
            // 语句级失败（语法/约束/业务错误）不影响连接；但链路中断要标记断开
            if (isLinkBroken(e.getMessage())) {
                close();
            }
            return Exec.error(sql, e.getMessage(), ms);
        } catch (RuntimeException e) {
            long ms = (System.nanoTime() - start) / 1_000_000L;
            close();
            return Exec.error(sql, e.getClass().getSimpleName() + ": " + e.getMessage(), ms);
        }
    }

    private boolean isLinkBroken(String message) {
        if (message == null) {
            return false;
        }
        return message.contains("连接中断")
                || message.contains("读取 MiniDB 响应失败")
                || message.contains("读取结果集失败")
                || message.contains("Broken pipe")
                || message.contains("Connection reset");
    }

    private void updateTxnState(String sql, boolean success) {
        if (!success) {
            return;
        }
        String head = SqlSplitter.firstKeyword(sql);
        if ("BEGIN".equals(head)) {
            inTransaction = true;
        } else if ("COMMIT".equals(head) || "ROLLBACK".equals(head)) {
            inTransaction = false;
        }
    }

    @Override
    public void close() {
        if (connection != null) {
            try {
                connection.close();
            } catch (RuntimeException ignored) {
                // 关闭失败不影响后续：连接已不可用
            }
        }
        connection = null;
        statement = null;
        inTransaction = false;
    }

    /** 一次执行的完整结果，供界面展示与日志记录。 */
    public static final class Exec {

        public enum Kind {
            /** 执行成功（OK n） */
            OK,
            /** 结果集（ROWS） */
            ROWS,
            /** 未识别的响应原文 */
            RAW,
            /** 失败（ERR） */
            ERROR
        }

        public final String sql;
        public final Kind kind;
        public final List<String> columns;
        public final List<List<String>> rows;
        public final int affected;
        public final String raw;
        public final String error;
        public final long elapsedMs;

        private Exec(String sql, Kind kind, List<String> columns, List<List<String>> rows,
                     int affected, String raw, String error, long elapsedMs) {
            this.sql = sql;
            this.kind = kind;
            this.columns = columns;
            this.rows = rows;
            this.affected = affected;
            this.raw = raw;
            this.error = error;
            this.elapsedMs = elapsedMs;
        }

        static Exec fromRaw(String sql, MiniDbStatement.RawResult r, long ms) {
            switch (r.kind) {
                case ROWS:
                    return new Exec(sql, Kind.ROWS, r.columns, r.rows, r.rows.size(), null, null, ms);
                case OK:
                    return new Exec(sql, Kind.OK, new ArrayList<>(), new ArrayList<>(), r.affected, null, null, ms);
                default:
                    return new Exec(sql, Kind.RAW, new ArrayList<>(), new ArrayList<>(), 0, r.raw, null, ms);
            }
        }

        static Exec error(String sql, String message, long ms) {
            return new Exec(sql, Kind.ERROR, new ArrayList<>(), new ArrayList<>(), 0, null,
                    message == null ? "(无错误信息)" : message, ms);
        }

        public boolean isError() {
            return kind == Kind.ERROR;
        }

        public boolean hasRows() {
            return kind == Kind.ROWS;
        }
    }
}
