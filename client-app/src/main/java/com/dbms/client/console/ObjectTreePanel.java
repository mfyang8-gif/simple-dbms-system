package com.dbms.client.console;

import javax.swing.*;
import javax.swing.tree.DefaultMutableTreeNode;
import javax.swing.tree.DefaultTreeModel;
import javax.swing.tree.TreePath;
import java.awt.*;
import java.awt.event.MouseAdapter;
import java.awt.event.MouseEvent;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.Comparator;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Properties;
import java.util.function.Function;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

/**
 * 对象浏览树。
 *
 * 必须说清的一个前提：MiniDB 目前<b>没有</b>任何目录查询语句
 * （没有 SHOW TABLES / information_schema / sys.tables），
 * 因此客户端无法像连 MySQL 那样自动列出对象。这里采用三条互补的途径：
 *
 *   1. 本地登记：执行 CREATE TABLE / CREATE VIEW / CREATE INDEX 时自动登记，
 *      DROP 时自动注销，并持久化到 config/console-objects.properties；
 *   2. 手动维护：右键可新增/移除，用于引擎停机后重新连上时恢复视图；
 *   3. 在线探测：点击「探测」时尝试发送 SHOW TABLES，若引擎将来支持则自动同步。
 *      不支持的引擎只会返回 ERR，界面如实提示，不假装成功。
 *
 * 这是一个诚实的降级设计：不把"猜"当作"元数据"，避免演示时点开一个不存在的表。
 */
public class ObjectTreePanel extends JPanel {

    private static final Path REGISTRY = Path.of("config", "console-objects.properties");

    private static final Pattern CREATE_OBJECT = Pattern.compile(
            "^\\s*CREATE\\s+(TABLE|VIEW|INDEX)\\s+([A-Za-z_][\\w$]*)", Pattern.CASE_INSENSITIVE);
    private static final Pattern DROP_OBJECT = Pattern.compile(
            "^\\s*DROP\\s+(TABLE|VIEW|INDEX)\\s+([A-Za-z_][\\w$]*)", Pattern.CASE_INSENSITIVE);

    private static final String GROUP_TABLE = "表";
    private static final String GROUP_VIEW = "视图";
    private static final String GROUP_INDEX = "索引";

    private final Function<String, ConsoleSession.Exec> executor;
    private final Runnable queryAction;

    private final DefaultMutableTreeNode root = new DefaultMutableTreeNode("MiniDB");
    private final DefaultTreeModel treeModel = new DefaultTreeModel(root);
    private final JTree tree = new JTree(treeModel);

    /** 对象名 → 类型（表/视图/索引）。用 LinkedHashMap 保持登记顺序。 */
    private final Map<String, String> objects = new LinkedHashMap<>();

    private String target = "未连接";

    /**
     * @param executor    用于「探测」时发送 SQL 的回调
     * @param queryAction 双击对象时需要执行的动作（由主窗口插入并执行 SELECT）
     */
    public ObjectTreePanel(Function<String, ConsoleSession.Exec> executor, Runnable queryAction) {
        super(new BorderLayout());
        this.executor = executor;
        this.queryAction = queryAction;

        loadRegistry();
        rebuild();

        tree.setRootVisible(true);
        tree.setShowsRootHandles(true);
        tree.setRowHeight(24);
        tree.addMouseListener(new MouseAdapter() {
            @Override
            public void mouseClicked(MouseEvent e) {
                if (e.getClickCount() == 2) {
                    String name = selectedObject();
                    if (name != null) {
                        queryAction.run();
                    }
                }
            }
        });
        tree.addTreeSelectionListener(e -> {
            // 选中后把对象名暴露给主窗口（双击时取用）
        });

        JPanel head = new JPanel(new FlowLayout(FlowLayout.LEFT, 4, 2));
        JButton probe = new JButton("探测");
        probe.setToolTipText("尝试 SHOW TABLES 从引擎同步对象列表（引擎不支持时仅提示）");
        probe.addActionListener(e -> probeFromEngine());
        JButton add = new JButton("新增");
        add.addActionListener(e -> addManually());
        head.add(probe);
        head.add(add);

        add(head, BorderLayout.NORTH);
        add(new JScrollPane(tree), BorderLayout.CENTER);

        installPopup();
    }

    public void setTarget(String target) {
        this.target = target;
        rebuild();
    }

    /** 当前选中的对象名；选中的是分组或根节点时返回 null。 */
    public String selectedObject() {
        TreePath path = tree.getSelectionPath();
        if (path == null) {
            return null;
        }
        Object node = ((DefaultMutableTreeNode) path.getLastPathComponent()).getUserObject();
        if (node instanceof String s) {
            return objects.containsKey(s) ? s : null;
        }
        return null;
    }

    /** 执行完一条 SQL 后调用：DDL 自动登记/注销。 */
    public void onSqlExecuted(String sql, boolean success) {
        if (!success) {
            return;
        }
        Matcher create = CREATE_OBJECT.matcher(sql);
        if (create.find()) {
            String type = normalizeType(create.group(1));
            String name = create.group(2);
            if (!type.equals(objects.get(name))) {
                objects.put(name, type);
                saveRegistry();
                rebuild();
            }
            return;
        }
        Matcher drop = DROP_OBJECT.matcher(sql);
        if (drop.find()) {
            String name = drop.group(2);
            if (objects.remove(name) != null) {
                saveRegistry();
                rebuild();
            }
        }
    }

    /** 供主窗口「刷新」调用：重新从磁盘读登记表。 */
    public void reload() {
        loadRegistry();
        rebuild();
    }

    private void probeFromEngine() {
        ConsoleSession.Exec exec = executor.apply("SHOW TABLES");
        if (exec == null) {
            info("当前未连接，无法探测。");
            return;
        }
        if (exec.isError()) {
            info("引擎不支持目录查询（SHOW TABLES 返回）：\n" + exec.error
                    + "\n\n对象列表仍以本地登记为准。等引擎补上目录查询语句后，此按钮会自动同步。");
            return;
        }
        if (exec.kind != ConsoleSession.Exec.Kind.ROWS || exec.columns.isEmpty()) {
            info("探测语句没有返回结果集，无法解析对象列表。");
            return;
        }
        int added = 0;
        for (List<String> row : exec.rows) {
            if (row.isEmpty() || row.get(0) == null) {
                continue;
            }
            String name = row.get(0).trim();
            if (name.isEmpty()) {
                continue;
            }
            String type = row.size() > 1 && row.get(1) != null ? normalizeType(row.get(1)) : GROUP_TABLE;
            if (!type.equals(objects.get(name))) {
                objects.put(name, type);
                added++;
            }
        }
        saveRegistry();
        rebuild();
        info("已从引擎同步对象，新增/更新 " + added + " 个。");
    }

    private void addManually() {
        JTextField nameField = new JTextField(16);
        JComboBox<String> typeBox = new JComboBox<>(new String[]{GROUP_TABLE, GROUP_VIEW, GROUP_INDEX});
        JPanel panel = new JPanel(new GridLayout(0, 2, 6, 6));
        panel.add(new JLabel("对象名："));
        panel.add(nameField);
        panel.add(new JLabel("类型："));
        panel.add(typeBox);
        int ok = JOptionPane.showConfirmDialog(this, panel, "登记数据库对象",
                JOptionPane.OK_CANCEL_OPTION, JOptionPane.PLAIN_MESSAGE);
        if (ok != JOptionPane.OK_OPTION) {
            return;
        }
        String name = nameField.getText().trim();
        if (name.isEmpty()) {
            return;
        }
        objects.put(name, (String) typeBox.getSelectedItem());
        saveRegistry();
        rebuild();
    }

    private void installPopup() {
        JPopupMenu menu = new JPopupMenu();

        JMenuItem query = new JMenuItem("查询全部数据（SELECT *）");
        query.addActionListener(e -> queryAction.run());

        JMenuItem copyName = new JMenuItem("复制对象名");
        copyName.addActionListener(e -> {
            String name = selectedObject();
            if (name != null) {
                Toolkit.getDefaultToolkit().getSystemClipboard()
                        .setContents(new java.awt.datatransfer.StringSelection(name), null);
            }
        });

        JMenuItem remove = new JMenuItem("移除登记");
        remove.addActionListener(e -> {
            String name = selectedObject();
            if (name == null) {
                return;
            }
            objects.remove(name);
            saveRegistry();
            rebuild();
        });

        JMenuItem reloadItem = new JMenuItem("重新读取登记表");
        reloadItem.addActionListener(e -> reload());

        menu.add(query);
        menu.add(copyName);
        menu.addSeparator();
        menu.add(remove);
        menu.add(reloadItem);

        tree.addMouseListener(new MouseAdapter() {
            @Override
            public void mousePressed(MouseEvent e) {
                maybeShow(e);
            }

            @Override
            public void mouseReleased(MouseEvent e) {
                maybeShow(e);
            }

            private void maybeShow(MouseEvent e) {
                if (!e.isPopupTrigger()) {
                    return;
                }
                TreePath path = tree.getPathForLocation(e.getX(), e.getY());
                if (path != null) {
                    tree.setSelectionPath(path);
                }
                menu.show(tree, e.getX(), e.getY());
            }
        });
    }

    private void rebuild() {
        root.setUserObject("MiniDB · " + target);
        root.removeAllChildren();

        Map<String, DefaultMutableTreeNode> groups = new LinkedHashMap<>();
        for (String g : new String[]{GROUP_TABLE, GROUP_VIEW, GROUP_INDEX}) {
            DefaultMutableTreeNode node = new DefaultMutableTreeNode(g);
            groups.put(g, node);
            root.add(node);
        }

        List<Map.Entry<String, String>> entries = new ArrayList<>(objects.entrySet());
        entries.sort(Comparator.comparing(Map.Entry::getKey, String.CASE_INSENSITIVE_ORDER));
        for (Map.Entry<String, String> entry : entries) {
            DefaultMutableTreeNode group = groups.getOrDefault(entry.getValue(), groups.get(GROUP_TABLE));
            group.add(new DefaultMutableTreeNode(entry.getKey()));
        }

        treeModel.reload();
        for (int i = 0; i < tree.getRowCount(); i++) {
            tree.expandRow(i);
        }
    }

    private static String normalizeType(String raw) {
        if (raw == null) {
            return GROUP_TABLE;
        }
        String s = raw.trim().toUpperCase();
        if (s.startsWith("VIEW") || s.equals(GROUP_VIEW)) {
            return GROUP_VIEW;
        }
        if (s.startsWith("INDEX") || s.equals(GROUP_INDEX)) {
            return GROUP_INDEX;
        }
        return GROUP_TABLE;
    }

    private void loadRegistry() {
        objects.clear();
        if (!Files.isRegularFile(REGISTRY)) {
            return;
        }
        Properties props = new Properties();
        try (InputStream in = Files.newInputStream(REGISTRY)) {
            props.load(in);
        } catch (IOException e) {
            return;
        }
        for (String name : props.stringPropertyNames()) {
            objects.put(name, normalizeType(props.getProperty(name)));
        }
    }

    private void saveRegistry() {
        Properties props = new Properties();
        props.putAll(objects);
        try {
            Path parent = REGISTRY.getParent();
            if (parent != null) {
                Files.createDirectories(parent);
            }
            try (OutputStream out = Files.newOutputStream(REGISTRY)) {
                props.store(out, "MiniDB 控制台本地对象登记（引擎暂无目录查询，故由客户端记录）");
            }
        } catch (IOException e) {
            // 持久化失败不影响当前会话使用
            System.err.println("保存对象登记表失败：" + e.getMessage());
        }
    }

    private void info(String message) {
        JOptionPane.showMessageDialog(this, message, "对象浏览", JOptionPane.INFORMATION_MESSAGE);
    }
}
