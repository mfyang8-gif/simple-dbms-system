// src/recovery/log_manager.rs
//
// 【模块职责】
// WAL 日志管理器：负责日志的追加、缓冲、落盘（fsync）与恢复期扫描。
//
// 【帧格式（与 log_record::serialize 严格对齐，含 CRC）】
// [u32 body_len][u32 crc32][body...]，帧头 = FRAME_HEADER_SIZE(8)。
// append 回填真实 LSN 会改变 body，故必须重算并回填 CRC（先定内容，最后盖章）。
// 扫描/读取时对每帧校验 CRC，不匹配即视为损坏边界并截断。
//
// 【LSN = 字节偏移量】新 LSN = 当前累计字节 + 本记录总帧长。
// 【Log Buffer】append 只写内存并推 curr_lsn；flush/flush_to 才 write_all + fsync 并推 flushed_lsn。
// 【不变量】buf.len() == curr_lsn - flushed_lsn。
// 【锁顺序】file -> log_buffer；append 只取 log_buffer，极短，无死锁。

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::sync::Mutex;

use super::log_record::{
    crc32, LogRecord, CRC_FIELD_OFFSET, CRC_SIZE, FRAME_HEADER_SIZE, LSN_FIELD_OFFSET, LEN_PREFIX_SIZE,
};
use super::lsn::{Lsn, INVALID_LSN};
use crate::config::LOG_FILE;

/// 内存日志缓冲区及其 LSN 计数（由单个 Mutex 保护，保证三者一致）
struct LogBuffer {
    /// 尚未落盘的日志字节，对应 LSN 区间 [flushed_lsn, curr_lsn)
    buf: Vec<u8>,
    /// 已追加的日志总字节数（= 最后一条记录的 LSN = 下一条记录的起始偏移）
    curr_lsn: Lsn,
    /// 已 fsync 落盘的日志总字节数
    flushed_lsn: Lsn,
}

/// WAL 日志管理器（线程安全）
pub struct LogManager {
    log_buffer: Mutex<LogBuffer>,
    file: Mutex<File>,
}

impl LogManager {
    /// 使用默认日志文件路径打开/创建
    pub fn new() -> std::io::Result<Self> {
        Self::from_path(LOG_FILE)
    }

    /// 使用指定路径打开/创建（测试可用临时文件）
    pub fn from_path(path: &str) -> std::io::Result<Self> {
        if let Some(parent) = std::path::Path::new(path).parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(path)?;

        // 扫描已有日志，恢复 LSN 计数，并顺带修复崩溃留下的撕裂/损坏尾部
        let recovered_lsn = Self::scan_and_repair(&mut file)?;

        Ok(Self {
            log_buffer: Mutex::new(LogBuffer {
                buf: Vec::new(),
                curr_lsn: recovered_lsn,
                flushed_lsn: recovered_lsn,
            }),
            file: Mutex::new(file),
        })
    }

    /// 顺序扫描日志文件，返回"完整且 CRC 有效记录的字节总量"作为恢复后的 LSN。
    /// 遇到撕裂尾、CRC 不匹配或无法反序列化的帧 → 停在该边界并截断。
    fn scan_and_repair(file: &mut File) -> std::io::Result<Lsn> {
        file.seek(SeekFrom::Start(0))?;
        let mut offset: Lsn = 0;

        loop {
            // 帧头：[u32 body_len][u32 crc32]
            let mut head = [0u8; FRAME_HEADER_SIZE];
            match file.read_exact(&mut head) {
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(e),
            }
            let body_len = u32::from_le_bytes(head[0..LEN_PREFIX_SIZE].try_into().unwrap()) as usize;
            let stored_crc =
                u32::from_le_bytes(head[LEN_PREFIX_SIZE..FRAME_HEADER_SIZE].try_into().unwrap());

            let mut body = vec![0u8; body_len];
            match file.read_exact(&mut body) {
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(e),
            }

            // CRC 校验：不匹配视为损坏边界，停在这里
            if crc32(&body) != stored_crc {
                break;
            }
            // 结构校验：无法反序列化也视为损坏边界
            if LogRecord::deserialize_body(&body).is_err() {
                break;
            }

            offset += FRAME_HEADER_SIZE as Lsn + body_len as Lsn;
        }

        // 若文件实际长度 > 有效记录总长，尾部有撕裂/损坏字节，截断之
        let file_len = file.metadata()?.len();
        if offset < file_len {
            file.set_len(offset)?;
        }

        Ok(offset)
    }

    /// 追加一条日志到内存缓冲区，返回分配的 LSN（= 该记录的结束字节偏移）。
    /// 【非阻塞】不做文件 I/O；回填 LSN 后必须重算 CRC。
    pub fn append(&self, record: &LogRecord) -> Lsn {
        let mut lb = self.log_buffer.lock().unwrap();

        // 先按占位 LSN 序列化以得知帧长度（LSN/CRC 定宽，长度与取值无关）
        let mut bytes = record.serialize(INVALID_LSN);
        let size = bytes.len() as Lsn;

        // 字节偏移量语义：新 LSN = 旧 LSN + 本记录字节数
        let lsn = lb.curr_lsn + size;

        // ① 原地回填真实 LSN（位于 body 内）
        bytes[LSN_FIELD_OFFSET..LSN_FIELD_OFFSET + 8].copy_from_slice(&lsn.to_le_bytes());

        // ② LSN 变了 → body 变了 → 重算并回填帧头 CRC（否则恢复期 CRC 校验会误判为损坏）
        let crc = crc32(&bytes[FRAME_HEADER_SIZE..]);
        bytes[CRC_FIELD_OFFSET..CRC_FIELD_OFFSET + CRC_SIZE].copy_from_slice(&crc.to_le_bytes());

        lb.buf.extend_from_slice(&bytes);
        lb.curr_lsn = lsn;
        lsn
    }

    /// 强制把 <= lsn 的日志落盘（fsync）。WAL 的核心保证点。
    pub fn flush_to(&self, lsn: Lsn) -> std::io::Result<()> {
        {
            let lb = self.log_buffer.lock().unwrap();
            if lb.flushed_lsn >= lsn {
                return Ok(());
            }
        }
        self.flush()
    }

    /// 把当前 log_buffer 中的全部字节落盘（write_all + fsync），并推进 flushed_lsn。
    pub fn flush(&self) -> std::io::Result<()> {
        let mut file = self.file.lock().unwrap();

        let (pending, pending_end_lsn) = {
            let mut lb = self.log_buffer.lock().unwrap();
            let taken = std::mem::take(&mut lb.buf);
            (taken, lb.curr_lsn)
        };

        if pending.is_empty() {
            return Ok(());
        }

        file.seek(SeekFrom::End(0))?;
        file.write_all(&pending)?;
        file.sync_all()?;

        self.log_buffer.lock().unwrap().flushed_lsn = pending_end_lsn;
        Ok(())
    }

    /// 当前已持久化（fsync）的 LSN
    pub fn flushed_lsn(&self) -> Lsn {
        self.log_buffer.lock().unwrap().flushed_lsn
    }

    /// 当前已产生的日志总字节数（含未落盘缓冲）
    pub fn curr_lsn(&self) -> Lsn {
        self.log_buffer.lock().unwrap().curr_lsn
    }

    /// 读取日志文件中的全部有效记录（恢复期使用），返回按 LSN 升序的 (lsn, record)。
    /// 遇到 CRC 不匹配 / 撕裂尾即停止。
    pub fn read_all(path: &str) -> std::io::Result<Vec<(Lsn, LogRecord)>> {
        let mut file = OpenOptions::new().read(true).open(path)?;
        file.seek(SeekFrom::Start(0))?;

        let mut out = Vec::new();
        loop {
            let mut head = [0u8; FRAME_HEADER_SIZE];
            match file.read_exact(&mut head) {
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(e),
            }
            let body_len = u32::from_le_bytes(head[0..LEN_PREFIX_SIZE].try_into().unwrap()) as usize;
            let stored_crc =
                u32::from_le_bytes(head[LEN_PREFIX_SIZE..FRAME_HEADER_SIZE].try_into().unwrap());

            let mut body = vec![0u8; body_len];
            match file.read_exact(&mut body) {
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(e),
            }

            if crc32(&body) != stored_crc {
                break;
            }

            let (record, lsn) = LogRecord::deserialize_body(&body)?;
            out.push((lsn, record));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_log(name: &str) -> String {
        let _ = std::fs::create_dir_all("./data");
        let path = format!("./data/{}", name);
        let _ = std::fs::remove_file(&path);
        path
    }

    #[test]
    fn test_append_lsn_is_byte_offset() {
        let path = temp_log("test_offset.log");
        let lm = LogManager::from_path(&path).unwrap();

        let r1 = LogRecord::Begin { txn_id: 1 };
        let l1 = lm.append(&r1);
        assert_eq!(l1, r1.serialized_len() as Lsn);

        let r2 = LogRecord::Commit { txn_id: 1 };
        let l2 = lm.append(&r2);
        assert_eq!(l2, l1 + r2.serialized_len() as Lsn);
        assert!(l2 > l1);
        assert_eq!(lm.curr_lsn(), l2);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_append_is_memory_only_until_flush() {
        let path = temp_log("test_buffer.log");
        let lm = LogManager::from_path(&path).unwrap();

        let lsn = lm.append(&LogRecord::Begin { txn_id: 1 });
        lm.append(&LogRecord::Update {
            txn_id: 1, prev_lsn: lsn, page_id: 5, offset: 8,
            before: b"aa".to_vec(), after: b"bb".to_vec(),
        });
        lm.append(&LogRecord::Commit { txn_id: 1 });

        assert_eq!(std::fs::metadata(&path).unwrap().len(), 0);
        assert_eq!(lm.flushed_lsn(), INVALID_LSN);

        lm.flush().unwrap();

        let size = std::fs::metadata(&path).unwrap().len();
        assert_eq!(size, lm.curr_lsn());
        assert_eq!(lm.flushed_lsn(), lm.curr_lsn());

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_flush_to_advances_persisted_lsn() {
        let path = temp_log("test_flush_to.log");
        let lm = LogManager::from_path(&path).unwrap();

        let l1 = lm.append(&LogRecord::Begin { txn_id: 1 });
        let l2 = lm.append(&LogRecord::Commit { txn_id: 1 });

        lm.flush_to(l1).unwrap();
        assert!(lm.flushed_lsn() >= l1);

        lm.flush_to(l2).unwrap();
        assert!(lm.flushed_lsn() >= l2);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_read_all_after_flush() {
        let path = temp_log("test_read_all.log");
        let lm = LogManager::from_path(&path).unwrap();

        lm.append(&LogRecord::Begin { txn_id: 1 });
        lm.append(&LogRecord::Update {
            txn_id: 1, prev_lsn: 0, page_id: 5, offset: 8,
            before: b"aa".to_vec(), after: b"bb".to_vec(),
        });
        lm.append(&LogRecord::Commit { txn_id: 1 });
        lm.flush().unwrap();

        let records = LogManager::read_all(&path).unwrap();
        assert_eq!(records.len(), 3);
        assert_eq!(records[2].1, LogRecord::Commit { txn_id: 1 });
        assert!(records[0].0 < records[1].0 && records[1].0 < records[2].0);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_reopen_resumes_byte_offset() {
        let path = temp_log("test_reopen.log");
        let last_lsn;
        {
            let lm = LogManager::from_path(&path).unwrap();
            lm.append(&LogRecord::Begin { txn_id: 1 });
            last_lsn = lm.append(&LogRecord::Commit { txn_id: 1 });
            lm.flush().unwrap();
        }
        let lm2 = LogManager::from_path(&path).unwrap();
        assert_eq!(lm2.curr_lsn(), last_lsn);
        let next = lm2.append(&LogRecord::Begin { txn_id: 2 });
        assert!(next > last_lsn);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_read_all_detects_crc_corruption() {
        let path = temp_log("test_crc.log");
        {
            let lm = LogManager::from_path(&path).unwrap();
            lm.append(&LogRecord::Begin { txn_id: 1 });
            lm.append(&LogRecord::Commit { txn_id: 1 });
            lm.flush().unwrap();
        }
        // 翻转第二帧 body 内的一个字节，制造"撕裂尾之外"的静默损坏
        {
            let mut data = std::fs::read(&path).unwrap();
            let first_body_len = u32::from_le_bytes(data[0..LEN_PREFIX_SIZE].try_into().unwrap()) as usize;
            let second_frame_start = FRAME_HEADER_SIZE + first_body_len;
            let corrupt = second_frame_start + FRAME_HEADER_SIZE + 1;
            data[corrupt] ^= 0xFF;
            std::fs::write(&path, &data).unwrap();
        }
        // read_all 遇到 CRC 不匹配应停止，只返回第一帧
        let records = LogManager::read_all(&path).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].1, LogRecord::Begin { txn_id: 1 });

        let _ = std::fs::remove_file(&path);
    }
}