// src/recovery/log_record.rs
//
// 【模块职责】
// 定义 WAL 日志记录的格式与序列化/反序列化。
//
// 【日志类型（ARIES-lite）】
// - Begin / Commit / Abort / End : 事务生命周期
// - Update                        : 物理日志，记录页内字节的 before/after 镜像
// - Compensate (CLR)              : 撤销时写的补偿日志，保证 undo 幂等、不重复
// - Checkpoint                    : 检查点，加速恢复（记录活跃事务 + 脏页表）
//
// 【LSN 语义】
// LSN 采用"字节偏移量"语义（借鉴 InnoDB）：一条记录的 LSN = 它在日志流中的结束偏移量。
// 由于 LSN 字段定宽（8 字节），记录长度与 LSN 取值无关，
// 因此 LogManager 可先按占位 LSN 序列化测长、再原地回填真实 LSN（见 LSN_FIELD_OFFSET）。
//
// 【磁盘格式（含 CRC 校验）】
// ┌─ frame ───────────────────────────────────────────────┐
// │ [u32 body_len][u32 crc32][body...]                     │
// └────────────────────────────────────────────────────────┘
// body:
//   [u8 tag][u64 lsn][type-specific fields...]
//
// - body_len：帧体字节数（长度前缀分帧，支持变长 body 的顺序扫描）。
// - crc32：覆盖整个 body 的校验和，用于检测"撕裂尾"之外的静默数据损坏（bit rot）。
//   注意：LogManager::append 回填真实 LSN 会改变 body，因此必须重算并回填 CRC。

use crate::recovery::lsn::{Lsn, INVALID_LSN};
use crate::types::PageId;

/// 事务 ID
pub type TxnId = u64;

// ==================== 帧布局常量 ====================

/// 长度前缀（body_len）字节数
pub const LEN_PREFIX_SIZE: usize = 4;

/// CRC 字段字节数
pub const CRC_SIZE: usize = 4;

/// 帧头总大小 = 长度前缀 + CRC（body 之前的固定部分）
pub const FRAME_HEADER_SIZE: usize = LEN_PREFIX_SIZE + CRC_SIZE; // = 8

/// CRC 字段在帧内的偏移（紧跟 body_len 之后）
pub const CRC_FIELD_OFFSET: usize = LEN_PREFIX_SIZE; // = 4

/// LSN 字段在帧内的起始字节偏移。
/// 帧布局 `[u32 body_len][u32 crc][u8 tag][u64 lsn][...]`，
/// 故 LSN 位于 `4 + 4 + 1 = 9` 处。
/// `LogManager` 据此原地回填真实 LSN，随后重算 CRC。
pub const LSN_FIELD_OFFSET: usize = FRAME_HEADER_SIZE + 1; // = 9

// ==================== 类型标签 ====================
const TAG_BEGIN: u8 = 1;
const TAG_COMMIT: u8 = 2;
const TAG_ABORT: u8 = 3;
const TAG_END: u8 = 4;
const TAG_UPDATE: u8 = 5;
const TAG_COMPENSATE: u8 = 6;
const TAG_CHECKPOINT: u8 = 7;

/// 一条 WAL 日志记录
#[derive(Debug, Clone, PartialEq)]
pub enum LogRecord {
    Begin { txn_id: TxnId },
    Commit { txn_id: TxnId },
    Abort { txn_id: TxnId },
    End { txn_id: TxnId },

    /// 物理更新日志：把 page_id 页 offset 处的字节从 before 改成 after
    Update {
        txn_id: TxnId,
        prev_lsn: Lsn,
        page_id: PageId,
        offset: u16,
        before: Vec<u8>,
        after: Vec<u8>,
    },

    /// 补偿日志（CLR）：undo 某条 Update 时写入
    Compensate {
        txn_id: TxnId,
        prev_lsn: Lsn,
        page_id: PageId,
        offset: u16,
        before: Vec<u8>,
        undo_next_lsn: Lsn,
    },

    /// 检查点
    Checkpoint {
        active_txns: Vec<(TxnId, Lsn)>,
        dirty_page_table: Vec<(PageId, Lsn)>,
    },
}

impl LogRecord {
    /// 序列化为完整帧：`[body_len][crc][body]`（含真实 LSN）
    pub fn serialize(&self, lsn: Lsn) -> Vec<u8> {
        let mut body = Vec::new();

        let tag: u8 = match self {
            LogRecord::Begin { .. } => TAG_BEGIN,
            LogRecord::Commit { .. } => TAG_COMMIT,
            LogRecord::Abort { .. } => TAG_ABORT,
            LogRecord::End { .. } => TAG_END,
            LogRecord::Update { .. } => TAG_UPDATE,
            LogRecord::Compensate { .. } => TAG_COMPENSATE,
            LogRecord::Checkpoint { .. } => TAG_CHECKPOINT,
        };
        body.push(tag);
        body.extend_from_slice(&lsn.to_le_bytes());

        match self {
            LogRecord::Begin { txn_id }
            | LogRecord::Commit { txn_id }
            | LogRecord::Abort { txn_id }
            | LogRecord::End { txn_id } => {
                body.extend_from_slice(&txn_id.to_le_bytes());
            }
            LogRecord::Update { txn_id, prev_lsn, page_id, offset, before, after } => {
                body.extend_from_slice(&txn_id.to_le_bytes());
                body.extend_from_slice(&prev_lsn.to_le_bytes());
                body.extend_from_slice(&page_id.to_le_bytes());
                body.extend_from_slice(&offset.to_le_bytes());
                push_bytes(&mut body, before);
                push_bytes(&mut body, after);
            }
            LogRecord::Compensate { txn_id, prev_lsn, page_id, offset, before, undo_next_lsn } => {
                body.extend_from_slice(&txn_id.to_le_bytes());
                body.extend_from_slice(&prev_lsn.to_le_bytes());
                body.extend_from_slice(&page_id.to_le_bytes());
                body.extend_from_slice(&offset.to_le_bytes());
                push_bytes(&mut body, before);
                body.extend_from_slice(&undo_next_lsn.to_le_bytes());
            }
            LogRecord::Checkpoint { active_txns, dirty_page_table } => {
                body.extend_from_slice(&(active_txns.len() as u32).to_le_bytes());
                for (txn_id, l) in active_txns {
                    body.extend_from_slice(&txn_id.to_le_bytes());
                    body.extend_from_slice(&l.to_le_bytes());
                }
                body.extend_from_slice(&(dirty_page_table.len() as u32).to_le_bytes());
                for (page_id, l) in dirty_page_table {
                    body.extend_from_slice(&page_id.to_le_bytes());
                    body.extend_from_slice(&l.to_le_bytes());
                }
            }
        }

        // 计算并写入帧头：[body_len][crc][body]
        let crc = crc32(&body);
        let mut out = Vec::with_capacity(FRAME_HEADER_SIZE + body.len());
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&body);
        out
    }

    /// 序列化后的总帧长（含 8 字节帧头）。
    /// 由于 LSN / CRC 字段均定宽，此长度与二者取值无关，
    /// 可安全地用占位 LSN 测长，再回填真实 LSN 与 CRC。
    pub fn serialized_len(&self) -> usize {
        self.serialize(INVALID_LSN).len()
    }

    /// 从 body（不含 8 字节帧头）反序列化，返回 (记录, 该记录的 LSN)。
    /// 调用方需先自行用 crc32 校验 body 完整性。
    pub fn deserialize_body(body: &[u8]) -> std::io::Result<(LogRecord, Lsn)> {
        let mut pos = 0usize;
        let tag = read_u8(body, &mut pos)?;
        let lsn = read_u64(body, &mut pos)?;

        let record = match tag {
            TAG_BEGIN => LogRecord::Begin { txn_id: read_u64(body, &mut pos)? },
            TAG_COMMIT => LogRecord::Commit { txn_id: read_u64(body, &mut pos)? },
            TAG_ABORT => LogRecord::Abort { txn_id: read_u64(body, &mut pos)? },
            TAG_END => LogRecord::End { txn_id: read_u64(body, &mut pos)? },
            TAG_UPDATE => {
                let txn_id = read_u64(body, &mut pos)?;
                let prev_lsn = read_u64(body, &mut pos)?;
                let page_id = read_u32(body, &mut pos)?;
                let offset = read_u16(body, &mut pos)?;
                let before = read_bytes(body, &mut pos)?;
                let after = read_bytes(body, &mut pos)?;
                LogRecord::Update { txn_id, prev_lsn, page_id, offset, before, after }
            }
            TAG_COMPENSATE => {
                let txn_id = read_u64(body, &mut pos)?;
                let prev_lsn = read_u64(body, &mut pos)?;
                let page_id = read_u32(body, &mut pos)?;
                let offset = read_u16(body, &mut pos)?;
                let before = read_bytes(body, &mut pos)?;
                let undo_next_lsn = read_u64(body, &mut pos)?;
                LogRecord::Compensate { txn_id, prev_lsn, page_id, offset, before, undo_next_lsn }
            }
            TAG_CHECKPOINT => {
                let n = read_u32(body, &mut pos)? as usize;
                let mut active_txns = Vec::with_capacity(n);
                for _ in 0..n {
                    let txn_id = read_u64(body, &mut pos)?;
                    let l = read_u64(body, &mut pos)?;
                    active_txns.push((txn_id, l));
                }
                let m = read_u32(body, &mut pos)? as usize;
                let mut dirty_page_table = Vec::with_capacity(m);
                for _ in 0..m {
                    let page_id = read_u32(body, &mut pos)?;
                    let l = read_u64(body, &mut pos)?;
                    dirty_page_table.push((page_id, l));
                }
                LogRecord::Checkpoint { active_txns, dirty_page_table }
            }
            _ => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("unknown log record tag {}", tag),
                ))
            }
        };

        Ok((record, lsn))
    }

    pub fn txn_id(&self) -> Option<TxnId> {
        match self {
            LogRecord::Begin { txn_id }
            | LogRecord::Commit { txn_id }
            | LogRecord::Abort { txn_id }
            | LogRecord::End { txn_id }
            | LogRecord::Update { txn_id, .. }
            | LogRecord::Compensate { txn_id, .. } => Some(*txn_id),
            LogRecord::Checkpoint { .. } => None,
        }
    }

    pub fn page_id(&self) -> Option<PageId> {
        match self {
            LogRecord::Update { page_id, .. } | LogRecord::Compensate { page_id, .. } => Some(*page_id),
            _ => None,
        }
    }

    pub fn prev_lsn(&self) -> Lsn {
        match self {
            LogRecord::Update { prev_lsn, .. } | LogRecord::Compensate { prev_lsn, .. } => *prev_lsn,
            _ => INVALID_LSN,
        }
    }
}

// ==================== CRC32（纯实现，无外部依赖） ====================

/// 标准 CRC-32（IEEE 802.3，反射多项式 0xEDB88320）。
/// 用于校验日志帧 body 的完整性：任何一位翻转都会导致校验值不同。
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            if crc & 1 == 1 {
                crc = (crc >> 1) ^ 0xEDB8_8320;
            } else {
                crc >>= 1;
            }
        }
    }
    !crc
}

// ==================== 序列化小工具 ====================

fn push_bytes(buf: &mut Vec<u8>, data: &[u8]) {
    buf.extend_from_slice(&(data.len() as u32).to_le_bytes());
    buf.extend_from_slice(data);
}

fn eof(what: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::UnexpectedEof, format!("log record truncated: {}", what))
}

fn read_u8(body: &[u8], pos: &mut usize) -> std::io::Result<u8> {
    if *pos + 1 > body.len() { return Err(eof("u8")); }
    let v = body[*pos];
    *pos += 1;
    Ok(v)
}

fn read_u16(body: &[u8], pos: &mut usize) -> std::io::Result<u16> {
    if *pos + 2 > body.len() { return Err(eof("u16")); }
    let v = u16::from_le_bytes(body[*pos..*pos + 2].try_into().unwrap());
    *pos += 2;
    Ok(v)
}

fn read_u32(body: &[u8], pos: &mut usize) -> std::io::Result<u32> {
    if *pos + 4 > body.len() { return Err(eof("u32")); }
    let v = u32::from_le_bytes(body[*pos..*pos + 4].try_into().unwrap());
    *pos += 4;
    Ok(v)
}

fn read_u64(body: &[u8], pos: &mut usize) -> std::io::Result<u64> {
    if *pos + 8 > body.len() { return Err(eof("u64")); }
    let v = u64::from_le_bytes(body[*pos..*pos + 8].try_into().unwrap());
    *pos += 8;
    Ok(v)
}

fn read_bytes(body: &[u8], pos: &mut usize) -> std::io::Result<Vec<u8>> {
    let len = read_u32(body, pos)? as usize;
    if *pos + len > body.len() { return Err(eof("bytes")); }
    let v = body[*pos..*pos + len].to_vec();
    *pos += len;
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_update_roundtrip() {
        let rec = LogRecord::Update {
            txn_id: 7, prev_lsn: 42, page_id: 3, offset: 128,
            before: b"old".to_vec(), after: b"new-value".to_vec(),
        };
        let bytes = rec.serialize(43);
        let body = &bytes[FRAME_HEADER_SIZE..];
        let (restored, lsn) = LogRecord::deserialize_body(body).unwrap();
        assert_eq!(lsn, 43);
        assert_eq!(restored, rec);
    }

    #[test]
    fn test_checkpoint_roundtrip() {
        let rec = LogRecord::Checkpoint {
            active_txns: vec![(1, 10), (2, 20)],
            dirty_page_table: vec![(5, 15), (9, 18)],
        };
        let bytes = rec.serialize(99);
        let (restored, lsn) = LogRecord::deserialize_body(&bytes[FRAME_HEADER_SIZE..]).unwrap();
        assert_eq!(lsn, 99);
        assert_eq!(restored, rec);
    }

    #[test]
    fn test_serialized_len_independent_of_lsn() {
        let rec = LogRecord::Update {
            txn_id: 1, prev_lsn: 0, page_id: 2, offset: 4,
            before: b"aa".to_vec(), after: b"bbb".to_vec(),
        };
        assert_eq!(rec.serialized_len(), rec.serialize(0).len());
        assert_eq!(rec.serialized_len(), rec.serialize(123456789).len());
    }

    #[test]
    fn test_crc_field_matches_body() {
        let rec = LogRecord::Begin { txn_id: 1 };
        let frame = rec.serialize(21);
        // 帧头里的 crc 应等于对 body 重算的 crc
        let stored_crc = u32::from_le_bytes(frame[CRC_FIELD_OFFSET..CRC_FIELD_OFFSET + 4].try_into().unwrap());
        let body = &frame[FRAME_HEADER_SIZE..];
        assert_eq!(stored_crc, crc32(body));
    }

    #[test]
    fn test_lifecycle_accessors() {
        let begin = LogRecord::Begin { txn_id: 3 };
        assert_eq!(begin.txn_id(), Some(3));
        assert_eq!(begin.page_id(), None);

        let upd = LogRecord::Update {
            txn_id: 3, prev_lsn: 8, page_id: 1, offset: 0,
            before: vec![0], after: vec![1],
        };
        assert_eq!(upd.page_id(), Some(1));
        assert_eq!(upd.prev_lsn(), 8);
    }
}