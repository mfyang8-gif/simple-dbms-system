package com.dbms.client.report;

import com.lowagie.text.Chunk;
import com.lowagie.text.Document;
import com.lowagie.text.Element;
import com.lowagie.text.PageSize;
import com.lowagie.text.Paragraph;
import com.lowagie.text.Phrase;
import com.lowagie.text.pdf.BaseFont;
import com.lowagie.text.pdf.PdfPCell;
import com.lowagie.text.pdf.PdfPTable;
import com.lowagie.text.pdf.PdfWriter;
import org.apache.poi.ss.usermodel.Cell;
import org.apache.poi.ss.usermodel.CellStyle;
import org.apache.poi.ss.usermodel.FillPatternType;
import org.apache.poi.ss.usermodel.Font;
import org.apache.poi.ss.usermodel.IndexedColors;
import org.apache.poi.ss.usermodel.Row;
import org.apache.poi.ss.usermodel.Sheet;
import org.apache.poi.ss.usermodel.Workbook;
import org.apache.poi.xssf.usermodel.XSSFWorkbook;
import org.jfree.chart.ChartUtils;
import org.jfree.chart.JFreeChart;

import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.OutputStream;
import java.util.List;

/** 导出工具：Excel / PDF / 图表 PNG。 */
public final class Exporters {

    private Exporters() {
    }

    // ================= Excel =================

    public static void exportExcel(File file, String sheetName, String[] headers, List<Object[]> rows)
            throws IOException {
        try (Workbook wb = new XSSFWorkbook()) {
            Sheet sheet = wb.createSheet(sheetName == null || sheetName.isBlank() ? "Sheet1" : sheetName);

            // 表头样式：加粗 + 灰底（第 4 课打磨）
            CellStyle headStyle = wb.createCellStyle();
            Font bold = wb.createFont();
            bold.setBold(true);
            headStyle.setFont(bold);
            headStyle.setFillForegroundColor(IndexedColors.GREY_25_PERCENT.getIndex());
            headStyle.setFillPattern(FillPatternType.SOLID_FOREGROUND);

            Row head = sheet.createRow(0);
            for (int i = 0; i < headers.length; i++) {
                Cell cell = head.createCell(i);
                cell.setCellValue(headers[i]);
                cell.setCellStyle(headStyle);
            }
            for (int r = 0; r < rows.size(); r++) {
                Row row = sheet.createRow(r + 1);
                Object[] data = rows.get(r);
                for (int c = 0; c < data.length; c++) {
                    Cell cell = row.createCell(c);
                    if (data[c] instanceof Number n) {
                        cell.setCellValue(n.doubleValue());
                    } else {
                        cell.setCellValue(data[c] == null ? "" : data[c].toString());
                    }
                }
            }
            for (int i = 0; i < headers.length; i++) {
                sheet.setColumnWidth(i, 4200);
            }
            sheet.createFreezePane(0, 1);   // 冻结首行：往下滚动时表头不动（第 4 课打磨）
            try (OutputStream out = new FileOutputStream(file)) {
                wb.write(out);
            }
        }
    }

    // ================= PDF =================

    public static void exportPdf(File file, String title, String[] headers, List<Object[]> rows)
            throws Exception {
        Document doc = new Document(PageSize.A4.rotate(), 36, 36, 42, 36);
        PdfWriter.getInstance(doc, new FileOutputStream(file));
        doc.open();

        com.lowagie.text.Font titleFont = new com.lowagie.text.Font(cjkFont(), 16, com.lowagie.text.Font.BOLD);
        com.lowagie.text.Font headFont = new com.lowagie.text.Font(cjkFont(), 11, com.lowagie.text.Font.BOLD);
        com.lowagie.text.Font cellFont = new com.lowagie.text.Font(cjkFont(), 10, com.lowagie.text.Font.NORMAL);

        Paragraph p = new Paragraph(title, titleFont);
        p.setAlignment(Element.ALIGN_CENTER);
        doc.add(p);
        doc.add(Chunk.NEWLINE);

        PdfPTable table = new PdfPTable(headers.length);
        table.setWidthPercentage(100);
        table.setHeaderRows(1);                    // 跨页时自动重复表头（第 4 课打磨）
        for (String h : headers) {
            PdfPCell cell = new PdfPCell(new Phrase(h, headFont));
            cell.setHorizontalAlignment(Element.ALIGN_CENTER);
            cell.setBackgroundColor(new java.awt.Color(235, 235, 235));
            table.addCell(cell);
        }
        for (Object[] row : rows) {
            for (Object v : row) {
                table.addCell(new Phrase(v == null ? "" : v.toString(), cellFont));
            }
        }
        doc.add(table);

        Paragraph footer = new Paragraph("共 " + rows.size() + " 条记录", cellFont);
        footer.setAlignment(Element.ALIGN_RIGHT);
        doc.add(footer);

        doc.close();
    }

    // ================= 图表 =================

    public static void exportChartPng(File file, JFreeChart chart, int width, int height) throws IOException {
        ChartUtils.saveChartAsPNG(file, chart, width, height);
    }

    /**
     * 中文字体：OpenPDF 自带字体不含中文，必须挂一个系统中文字体。
     * 依次尝试微软雅黑 / 宋体 / 黑体，都找不到再退回内置 CID 字体。
     */
    private static BaseFont cjkFont() throws Exception {
        String[][] candidates = {
                {"C:/Windows/Fonts/msyh.ttc", "0"},
                {"C:/Windows/Fonts/simsun.ttc", "0"},
                {"C:/Windows/Fonts/simhei.ttf", null}
        };
        for (String[] c : candidates) {
            try {
                if (!new File(c[0]).exists()) {
                    continue;
                }
                String path = c[1] == null ? c[0] : c[0] + "," + c[1];
                return BaseFont.createFont(path, BaseFont.IDENTITY_H, BaseFont.EMBEDDED);
            } catch (Exception ignored) {
                // 试下一个
            }
        }
        return BaseFont.createFont("STSong-Light", "UniGB-UCS2-H", BaseFont.NOT_EMBEDDED);
    }
}
