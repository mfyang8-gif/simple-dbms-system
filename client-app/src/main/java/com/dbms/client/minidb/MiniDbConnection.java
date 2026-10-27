package com.dbms.client.minidb;

import java.io.BufferedReader;
import java.io.IOException;
import java.io.InputStreamReader;
import java.io.OutputStreamWriter;
import java.io.PrintWriter;
import java.net.Socket;
import java.nio.charset.StandardCharsets;

/**
 * 一条到 MiniDB 的 TCP 连接 = 一个会话。
 *
 * MiniDB 的事务语义是“一连接一会话”，与 JDBC 的“一 Connection 一事务”天然对应：
 *   setAutoCommit(false) → 发送 BEGIN
 *   commit()             → 发送 COMMIT
 *   rollback()           → 发送 ROLLBACK
 * 因此 InventoryService 里共用同一条连接做多语句强事务的写法可以直接保留。
 */
public class MiniDbConnection implements AutoCloseable {

    private final Socket socket;
    private final BufferedReader in;
    private final PrintWriter out;
    private boolean autoCommit = true;
    private boolean closed = false;

    public MiniDbConnection(String host, int port) {
        try {
            this.socket = new Socket(host, port);
            this.in = new BufferedReader(new InputStreamReader(socket.getInputStream(), StandardCharsets.UTF_8));
            this.out = new PrintWriter(new OutputStreamWriter(socket.getOutputStream(), StandardCharsets.UTF_8), true);
        } catch (IOException e) {
            throw new MiniDbException("连接 MiniDB 失败（" + host + ":" + port + "）：" + e.getMessage(), e);
        }
    }

    public MiniDbStatement createStatement() {
        ensureOpen();
        return new MiniDbStatement(in, out);
    }

    public void setAutoCommit(boolean autoCommit) {
        ensureOpen();
        if (this.autoCommit && !autoCommit) {
            try (MiniDbStatement st = createStatement()) {
                st.executeUpdate("BEGIN");
            }
        }
        this.autoCommit = autoCommit;
    }

    public void commit() {
        ensureOpen();
        if (autoCommit) {
            return;
        }
        try (MiniDbStatement st = createStatement()) {
            st.executeUpdate("COMMIT");
        }
        this.autoCommit = true;
    }

    public void rollback() {
        ensureOpen();
        if (autoCommit) {
            return;
        }
        try (MiniDbStatement st = createStatement()) {
            st.executeUpdate("ROLLBACK");
        }
        this.autoCommit = true;
    }

    private void ensureOpen() {
        if (closed) {
            throw new MiniDbException("连接已关闭");
        }
    }

    @Override
    public void close() {
        if (closed) {
            return;
        }
        closed = true;
        try {
            // 双保险：仍有未提交事务则回滚（引擎在断连时也会回滚）
            if (!autoCommit) {
                try (MiniDbStatement st = createStatement()) {
                    st.executeUpdate("ROLLBACK");
                } catch (Exception ignored) {
                }
            }
            out.print("quit\n");
            out.flush();
        } catch (Exception ignored) {
        }
        try {
            socket.close();
        } catch (IOException ignored) {
        }
    }
}
