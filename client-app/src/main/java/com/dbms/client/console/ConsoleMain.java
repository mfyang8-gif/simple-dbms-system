package com.dbms.client.console;

import com.dbms.client.minidb.MiniDbException;

import javax.swing.*;
import java.awt.*;
import java.awt.event.WindowAdapter;
import java.awt.event.WindowEvent;
import java.io.InputStream;
import java.nio.file.Files;
import java.nio.file.Path;
import java.time.LocalTime;
import java.time.format.DateTimeFormatter;
import java.util.ArrayList;
import java.util.List;
import java.util.Properties;

/**
 * MiniDB 管理控制台（课题1 附加功能2 的客户端侧）。
 *
 * 定位：一个**通用**的数据库客户端——只认协议，不认识业务表。
 * 因此引擎将来加了 JOIN / 视图 / 索引 / 授权，这个界面无需改动即可演示；
 * 反过来，它也不会替引擎掩盖任何缺陷（不做 SQL 改写、ERR 原文照显）。
 *
 * 与仓库管理 GUI（com.dbms.client.App）的关系：
 *   那个是"基于本 DBMS 开发的数据库应用"，属于客户端的**使用方**；
 *   这个控制台是**验证方**，用于逐条验收引擎能力。两者互不依赖。
 */
public class ConsoleMain extends JFrame {

    private static final DateTimeFormatter TIME = DateTimeFormatter.ofPattern("HH:mm:ss");

    private final JTextField hostField = new JTextField(12);
    private final JTextField portField = new JTextField(6);
    private final JLabel statusLabel = new JLabel("未连接");
    private final JLabel txnLabel = new JLabel(" ");
    private final JLabel targetLabel = new JLabel(" ");

    private final ResultTablePanel resultPanel = new ResultTablePanel();
    private final JTextArea messageArea = new JTextArea();
    private final JTabbedPane bottomTabs = new JTabbedPane();
    private final ObjectTreePanel treePanel;

    private final SqlEditor editor;
    private ConsoleSession session;
    private JButton connectBtn;
    private JButton disconnectBtn;

    public ConsoleMain() {
        super("MiniDB 控制台 — 数据库客户端");
        loadConfigDefaults();

        editor = new SqlEditor(this::onExecute);
        treePanel = new ObjectTreePanel(this::executeForProbe, this::querySelectedObject);

        setDefaultCloseOperation(EXIT_ON_CLOSE);
        // 按屏幕可用区域自适应：1366x768 的演示机上固定 820 高会让状态栏被任务栏挡住
        Dimension screen = Toolkit.getDefaultToolkit().getScreenSize();
        setSize(Math.min(1280, screen.width - 80), Math.min(820, screen.height - 120));
        setMinimumSize(new Dimension(960, 600));
        setLocationRelativeTo(null);

        messageArea.setEditable(false);
        messageArea.setFont(new Font(Font.MONOSPACED, Font.PLAIN, 12));
        bottomTabs.addTab("结果", resultPanel);
        bottomTabs.addTab("消息", new JScrollPane(messageArea));

        JSplitPane rightSplit = new JSplitPane(JSplitPane.VERTICAL_SPLIT,
                buildEditorPane(), bottomTabs);
        rightSplit.setResizeWeight(0.42);
        rightSplit.setDividerLocation(300);

        JSplitPane mainSplit = new JSplitPane(JSplitPane.HORIZONTAL_SPLIT,
                treePanel, rightSplit);
        mainSplit.setResizeWeight(0.0);
        mainSplit.setDividerLocation(240);

        add(buildConnectionBar(), BorderLayout.NORTH);
        add(mainSplit, BorderLayout.CENTER);
        add(buildStatusBar(), BorderLayout.SOUTH);
        setJMenuBar(buildMenuBar());

        addWindowListener(new WindowAdapter() {
            @Override
            public void windowClosing(WindowEvent e) {
                if (session != null) {
                    session.close();
                }
            }
        });

        updateConnectionState();
        log("控制台已启动。目标：" + hostField.getText() + ":" + portField.getText());
        log("提示：引擎为「一行一条语句」，多行 SQL 会在发送前自动压成单行；可用分号分隔多条语句。");
    }

    // ==================== 界面搭建 ====================

    private JComponent buildConnectionBar() {
        JPanel bar = new JPanel(new FlowLayout(FlowLayout.LEFT, 8, 6));
        bar.setBorder(BorderFactory.createMatteBorder(0, 0, 1, 0, new Color(0xD0, 0xD4, 0xDA)));

        bar.add(new JLabel("主机"));
        bar.add(hostField);
        bar.add(new JLabel("端口"));
        bar.add(portField);

        connectBtn = new JButton("连接");
        connectBtn.addActionListener(e -> connect());
        disconnectBtn = new JButton("断开");
        disconnectBtn.addActionListener(e -> disconnect());
        bar.add(connectBtn);
        bar.add(disconnectBtn);
        return bar;
    }

    private JComponent buildEditorPane() {
        JPanel top = new JPanel(new BorderLayout());

        JPanel tools = new JPanel(new FlowLayout(FlowLayout.LEFT, 6, 4));
        JButton run = new JButton("执行 (F5)");
        run.addActionListener(e -> onExecute());
        tools.add(run);

        JButton begin = new JButton("BEGIN");
        begin.addActionListener(e -> executeFixed("BEGIN"));
        tools.add(begin);

        JButton commit = new JButton("COMMIT");
        commit.addActionListener(e -> executeFixed("COMMIT"));
        tools.add(commit);

        JButton rollback = new JButton("ROLLBACK");
        rollback.addActionListener(e -> executeFixed("ROLLBACK"));
        tools.add(rollback);

        tools.add(Box.createHorizontalStrut(10));
        JButton clearEditor = new JButton("清空");
        clearEditor.addActionListener(e -> editor.clear());
        tools.add(clearEditor);

        JButton export = new JButton("导出 CSV");
        export.addActionListener(e -> resultPanel.exportCsv(this));
        tools.add(export);

        top.add(tools, BorderLayout.NORTH);
        top.add(editor, BorderLayout.CENTER);
        return top;
    }

    private JComponent buildStatusBar() {
        JPanel bar = new JPanel(new BorderLayout());
        bar.setBorder(BorderFactory.createMatteBorder(1, 0, 0, 0, new Color(0xD0, 0xD4, 0xDA)));
        statusLabel.setBorder(BorderFactory.createEmptyBorder(4, 8, 4, 8));
        txnLabel.setBorder(BorderFactory.createEmptyBorder(4, 8, 4, 8));
        targetLabel.setBorder(BorderFactory.createEmptyBorder(4, 8, 4, 8));

        JPanel left = new JPanel(new FlowLayout(FlowLayout.LEFT, 4, 2));
        left.add(statusLabel);
        left.add(txnLabel);

        bar.add(left, BorderLayout.WEST);
        bar.add(targetLabel, BorderLayout.EAST);
        return bar;
    }

    private JMenuBar buildMenuBar() {
        JMenuBar bar = new JMenuBar();

        JMenu conn = new JMenu("连接");
        JMenuItem c = new JMenuItem("连接");
        c.addActionListener(e -> connect());
        JMenuItem d = new JMenuItem("断开");
        d.addActionListener(e -> disconnect());
        JMenuItem exit = new JMenuItem("退出");
        exit.addActionListener(e -> {
            if (session != null) {
                session.close();
            }
            System.exit(0);
        });
        conn.add(c);
        conn.add(d);
        conn.addSeparator();
        conn.add(exit);

        JMenu exec = new JMenu("执行");
        JMenuItem run = new JMenuItem("执行 (F5)");
        run.addActionListener(e -> onExecute());
        JMenuItem export = new JMenuItem("导出结果为 CSV");
        export.addActionListener(e -> resultPanel.exportCsv(this));
        JMenuItem copy = new JMenuItem("复制选中单元格");
        copy.addActionListener(e -> resultPanel.copySelectionToClipboard(this));
        JMenuItem clearResult = new JMenuItem("清空结果");
        clearResult.addActionListener(e -> resultPanel.clear());
        exec.add(run);
        exec.addSeparator();
        exec.add(export);
        exec.add(copy);
        exec.add(clearResult);

        JMenu samples = new JMenu("示例");
        for (SqlSnippets.Snippet s : SqlSnippets.all()) {
            JMenuItem item = new JMenuItem(s.supported() ? s.name() : s.name() + "（引擎暂不支持）");
            item.addActionListener(e -> editor.appendText(s.sql() + "\n"));
            samples.add(item);
        }

        JMenu help = new JMenu("帮助");
        JMenuItem about = new JMenuItem("关于 / 协议说明");
        about.addActionListener(e -> showAbout());
        help.add(about);

        bar.add(conn);
        bar.add(exec);
        bar.add(samples);
        bar.add(help);
        return bar;
    }

    // ==================== 连接管理 ====================

    private void connect() {
        String host = hostField.getText().trim();
        int port;
        try {
            port = Integer.parseInt(portField.getText().trim());
        } catch (NumberFormatException e) {
            JOptionPane.showMessageDialog(this, "端口必须是数字。", "参数错误", JOptionPane.ERROR_MESSAGE);
            return;
        }
        if (host.isEmpty()) {
            JOptionPane.showMessageDialog(this, "主机不能为空。", "参数错误", JOptionPane.ERROR_MESSAGE);
            return;
        }

        if (session != null) {
            session.close();
        }
        try {
            session = new ConsoleSession(host, port);
            log("已连接 " + host + ":" + port);
            treePanel.setTarget(session.target());
            targetLabel.setText("目标：" + session.target());
        } catch (MiniDbException e) {
            session = null;
            log("连接失败：" + e.getMessage());
            JOptionPane.showMessageDialog(this,
                    "无法连接 " + host + ":" + port + "\n\n" + e.getMessage()
                            + "\n\n请确认 MiniDB 服务端已启动（cd db-engine && cargo run）。",
                    "连接失败", JOptionPane.ERROR_MESSAGE);
        }
        updateConnectionState();
    }

    private void disconnect() {
        if (session != null) {
            session.close();
            session = null;
            log("已断开连接");
        }
        updateConnectionState();
    }

    private void updateConnectionState() {
        boolean connected = session != null && session.isConnected();
        if (connected) {
            statusLabel.setText("● 已连接");
            statusLabel.setForeground(new Color(0x1B, 0x7F, 0x3B));
        } else {
            statusLabel.setText("● 未连接");
            statusLabel.setForeground(new Color(0xB0, 0x2A, 0x2A));
        }
        if (connectBtn != null) {
            connectBtn.setEnabled(!connected);
        }
        if (disconnectBtn != null) {
            disconnectBtn.setEnabled(connected);
        }
        updateTxnState();
    }

    private void updateTxnState() {
        boolean inTxn = session != null && session.isInTransaction();
        if (inTxn) {
            txnLabel.setText("｜事务进行中（未提交）");
            txnLabel.setForeground(new Color(0xB4, 0x6A, 0x00));
        } else {
            txnLabel.setText(session == null ? " " : "｜自动提交");
            txnLabel.setForeground(new Color(0x60, 0x66, 0x70));
        }
    }

    // ==================== 执行 ====================

    private void onExecute() {
        if (!ensureConnected()) {
            return;
        }
        String text = editor.getExecutableText();
        List<String> statements = SqlSplitter.split(text);
        if (statements.isEmpty()) {
            log("没有可执行的语句。");
            return;
        }
        executeStatements(statements);
    }

    private void executeFixed(String sql) {
        if (!ensureConnected()) {
            return;
        }
        executeStatements(List.of(sql));
    }

    private boolean ensureConnected() {
        if (session != null && session.isConnected()) {
            return true;
        }
        connect();
        return session != null && session.isConnected();
    }

    /**
     * 顺序执行一组语句。
     * 遇到错误立即停止——批处理里出现语法错误后继续执行，往往会在错误状态下
     * 产生难以解释的副作用（例如建表失败后仍继续插入），对教学演示尤其不利。
     */
    private void executeStatements(List<String> statements) {
        ConsoleSession.Exec last = null;
        int index = 0;
        for (String sql : statements) {
            index++;
            ConsoleSession.Exec exec = session.execute(sql);
            treePanel.onSqlExecuted(sql, !exec.isError());
            logResult(index, statements.size(), exec);
            last = exec;
            if (exec.isError()) {
                break;
            }
        }
        if (last != null) {
            resultPanel.showResult(last);
            if (last.isError()) {
                bottomTabs.setSelectedIndex(1);
            } else {
                bottomTabs.setSelectedIndex(0);
            }
        }
        updateTxnState();
        updateConnectionState();
    }

    private ConsoleSession.Exec executeForProbe(String sql) {
        if (session == null || !session.isConnected()) {
            return null;
        }
        ConsoleSession.Exec exec = session.execute(sql);
        logResult(1, 1, exec);
        return exec;
    }

    private void querySelectedObject() {
        String name = treePanel.selectedObject();
        if (name == null) {
            return;
        }
        if (!ensureConnected()) {
            return;
        }
        editor.appendText("SELECT * FROM " + name + ";\n");
        executeStatements(List.of("SELECT * FROM " + name));
    }

    private void logResult(int index, int total, ConsoleSession.Exec exec) {
        String prefix = total > 1 ? "[" + index + "/" + total + "] " : "";
        log(prefix + "▶ " + exec.sql);
        if (exec.isError()) {
            log("   ✗ ERR " + exec.error + "   (" + exec.elapsedMs + " ms)");
        } else {
            switch (exec.kind) {
                case ROWS -> log("   ↳ " + exec.rows.size() + " 行 · " + exec.columns.size()
                        + " 列 · " + exec.elapsedMs + " ms");
                case OK -> log("   ↳ OK，影响 " + exec.affected + " 行 · " + exec.elapsedMs + " ms");
                default -> log("   ↳ 未识别的响应：" + exec.raw);
            }
        }
    }

    private void log(String message) {
        messageArea.append("[" + LocalTime.now().format(TIME) + "] " + message + "\n");
        messageArea.setCaretPosition(messageArea.getDocument().getLength());
    }

    // ==================== 配置与帮助 ====================

    private void loadConfigDefaults() {
        String host = "127.0.0.1";
        String port = "9000";
        Path config = Path.of("config", "db.properties");
        if (Files.isRegularFile(config)) {
            Properties props = new Properties();
            try (InputStream in = Files.newInputStream(config)) {
                props.load(in);
                host = props.getProperty("db.host", host);
                port = props.getProperty("db.port", port);
            } catch (Exception ignored) {
                // 配置读取失败就用默认值，不阻断启动
            }
        }
        hostField.setText(host);
        portField.setText(port);
    }

    private void showAbout() {
        JOptionPane.showMessageDialog(this, """
                MiniDB 管理控制台

                用途：课题1「小型 DBMS 系统开发」的客户端（附加功能2：执行引擎与界面分离）。
                通信：TCP 自定义文本协议（一行一条语句）。
                  OK <n>            执行成功，n 为影响行数
                  ROWS <c> <r>      结果集，随后是 TAB 分隔的表头、数据行与 END
                  ERR <msg>         错误，原文照显
                  NULL 记为 \\N

                设计原则：
                  · 不做任何 SQL 改写，引擎支持什么就显示什么；
                  · 未知响应前缀原样显示，便于引擎升级时前向兼容；
                  · 对象树以本地登记为主（引擎暂无目录查询语句），并支持 SHOW TABLES 探测。

                快捷键：F5 / Ctrl+Enter 执行；Tab 缩进。
                """, "关于", JOptionPane.INFORMATION_MESSAGE);
    }

    // ==================== 入口 ====================

    public static void main(String[] args) {
        if (args.length > 0 && "--selftest".equals(args[0])) {
            runSelfTest();
            return;
        }
        SwingUtilities.invokeLater(() -> {
            installTheme();
            new ConsoleMain().setVisible(true);
        });
    }

    private static void installTheme() {
        try {
            UIManager.setLookAndFeel(new com.formdev.flatlaf.FlatLightLaf());
        } catch (Exception ignored) {
            // 退化到系统默认外观
        }
        Font ui = new Font("Microsoft YaHei UI", Font.PLAIN, 13);
        if (!"Microsoft YaHei UI".equals(ui.getFamily())) {
            ui = new Font(Font.SANS_SERIF, Font.PLAIN, 13);
        }
        for (Object key : new java.util.ArrayList<>(UIManager.getLookAndFeelDefaults().keySet())) {
            if (key instanceof String s && s.endsWith(".font")) {
                UIManager.put(key, ui);
            }
        }
    }

    /**
     * 无界面自检：连接 → 建表 → 插入 → 查询 → 事务回滚 → 错误路径。
     * 用于在无法打开窗口的环境里验证协议层与控制台执行链路（不替代 GUI 验收）。
     */
    private static void runSelfTest() {
        String host = System.getProperty("db.host", "127.0.0.1");
        int port = Integer.getInteger("db.port", 9000);
        System.out.println("selftest target = " + host + ":" + port);

        try (ConsoleSession s = new ConsoleSession(host, port)) {
            List<String> script = new ArrayList<>(List.of(
                    "CREATE TABLE console_probe (id INT PRIMARY KEY, name VARCHAR(20), qty INT)",
                    "INSERT INTO console_probe VALUES (1, '甲', 10)",
                    "INSERT INTO console_probe VALUES (2, '乙', 20)",
                    "SELECT * FROM console_probe",
                    "BEGIN",
                    "UPDATE console_probe SET qty = 99 WHERE id = 1",
                    "SELECT qty FROM console_probe WHERE id = 1",
                    "ROLLBACK",
                    "SELECT qty FROM console_probe WHERE id = 1",
                    "SELECT COUNT(*) FROM console_probe",
                    "SHOW TABLES",
                    "DROP TABLE console_probe"
            ));
            for (String sql : script) {
                ConsoleSession.Exec r = s.execute(sql);
                if (r.isError()) {
                    System.out.println("ERR  | " + sql + "\n     | " + r.error);
                } else if (r.hasRows()) {
                    System.out.println("ROWS | " + r.rows.size() + " 行 | " + sql);
                    for (List<String> row : r.rows) {
                        System.out.println("     | " + String.join(" | ", row.stream()
                                .map(v -> v == null ? "NULL" : v).toList()));
                    }
                } else {
                    System.out.println("OK   | affected=" + r.affected + " | " + sql);
                }
            }
            System.out.println("selftest finished; txnState=" + s.isInTransaction());
        } catch (RuntimeException e) {
            System.out.println("selftest aborted: " + e.getMessage());
            System.exit(2);
        }
    }
}
