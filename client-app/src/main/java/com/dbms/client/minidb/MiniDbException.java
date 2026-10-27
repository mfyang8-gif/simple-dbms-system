package com.dbms.client.minidb;

/**
 * MiniDB 客户端异常（非受检）。
 * 便于在 DAO 层透传，最终由 Service 层统一兜底（catch Exception）。
 */
public class MiniDbException extends RuntimeException {
    public MiniDbException(String message) {
        super(message);
    }

    public MiniDbException(String message, Throwable cause) {
        super(message, cause);
    }
}
