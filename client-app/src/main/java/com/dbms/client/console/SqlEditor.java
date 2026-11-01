package com.dbms.client.console;

import javax.swing.*;
import javax.swing.event.DocumentEvent;
import javax.swing.event.DocumentListener;
import java.awt.*;
import java.awt.event.InputEvent;
import java.awt.event.KeyEvent;

/**
 * SQL 编辑区。
 *
 * 交互约定（对齐常见数据库客户端的肌肉记忆，答辩演示时不必解释）：
 *   F5 / Ctrl+Enter  执行（有选中内容则只执行选中部分）
 *   Tab              插入 4 个空格，而不是把焦点跳到下一个控件
 *   Ctrl+A           全选（JTextArea 默认行为，这里显式保留以免被快捷键覆盖）
 *
 * 这里只负责"取文本"，切分与发送都在 {@link SqlSplitter} 和 {@link ConsoleSession} 里，
 * 保证编辑器可以被替换（比如以后换成 RSyntaxTextArea）而不影响执行逻辑。
 */
public class SqlEditor extends JPanel {

    private final JTextArea area = new JTextArea();
    private final JLabel caret = new JLabel("行 1，列 1");

    public SqlEditor(Runnable onExecute) {
        super(new BorderLayout());

        area.setFont(new Font(Font.MONOSPACED, Font.PLAIN, 14));
        area.setTabSize(4);
        area.setLineWrap(false);
        area.setMargin(new Insets(6, 8, 6, 8));
        area.setText("-- 连接后可在此输入 SQL，按 F5 或 Ctrl+Enter 执行\n");

        installKeyBindings(onExecute);

        area.getDocument().addDocumentListener(new DocumentListener() {
            @Override
            public void insertUpdate(DocumentEvent e) {
                updateCaret();
            }

            @Override
            public void removeUpdate(DocumentEvent e) {
                updateCaret();
            }

            @Override
            public void changedUpdate(DocumentEvent e) {
                updateCaret();
            }
        });
        area.addCaretListener(e -> updateCaret());

        caret.setBorder(BorderFactory.createEmptyBorder(2, 8, 2, 8));
        caret.setFont(caret.getFont().deriveFont(Font.PLAIN, 12f));

        add(new JScrollPane(area), BorderLayout.CENTER);
        add(caret, BorderLayout.SOUTH);
    }

    private void installKeyBindings(Runnable onExecute) {
        InputMap im = area.getInputMap(JComponent.WHEN_FOCUSED);
        ActionMap am = area.getActionMap();

        im.put(KeyStroke.getKeyStroke(KeyEvent.VK_F5, 0), "console-execute");
        im.put(KeyStroke.getKeyStroke(KeyEvent.VK_ENTER, InputEvent.CTRL_DOWN_MASK), "console-execute");
        im.put(KeyStroke.getKeyStroke(KeyEvent.VK_ENTER, InputEvent.META_DOWN_MASK), "console-execute");

        am.put("console-execute", new AbstractAction() {
            @Override
            public void actionPerformed(java.awt.event.ActionEvent e) {
                onExecute.run();
            }
        });

        // Tab 插入缩进，而不是切换焦点
        im.put(KeyStroke.getKeyStroke(KeyEvent.VK_TAB, 0), "console-tab");
        am.put("console-tab", new AbstractAction() {
            @Override
            public void actionPerformed(java.awt.event.ActionEvent e) {
                area.insert("    ", area.getCaretPosition());
            }
        });
    }

    private void updateCaret() {
        try {
            int pos = area.getCaretPosition();
            int line = area.getLineOfOffset(pos) + 1;
            int col = pos - area.getLineStartOffset(line - 1) + 1;
            caret.setText("行 " + line + "，列 " + col);
        } catch (Exception ignored) {
            caret.setText(" ");
        }
    }

    /** 有选中内容则返回选中部分，否则返回全文。 */
    public String getExecutableText() {
        String selected = area.getSelectedText();
        if (selected != null && !selected.isBlank()) {
            return selected;
        }
        return area.getText();
    }

    public void setText(String text) {
        area.setText(text);
        area.setCaretPosition(0);
        area.requestFocusInWindow();
    }

    public void appendText(String text) {
        int len = area.getDocument().getLength();
        if (len > 0 && !area.getText().endsWith("\n")) {
            area.append("\n");
        }
        area.append(text);
        area.setCaretPosition(area.getDocument().getLength());
        area.requestFocusInWindow();
    }

    public void clear() {
        area.setText("");
        area.requestFocusInWindow();
    }

    public JTextArea textArea() {
        return area;
    }
}
