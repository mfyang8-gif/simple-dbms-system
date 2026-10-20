use std::collections::HashMap;
use std::sync::Arc;

use crate::recovery::log_manager::LogManager;
use crate::recovery::log_record::{LogRecord, TxnId};

use crate::types::PageId;
use crate::recovery::lsn::{Lsn, INVALID_LSN};
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TxnStatus { Active, Committed, Aborted }

/// 一个事务(取代 Slice B 里"每条语句现造的 WalContext")
pub struct Transaction {
    pub txn_id: TxnId,
    pub status: TxnStatus,
    pub last_lsn: Lsn, // 本事务上一条日志的 LSN(和 WalContext.last_lsn 同义)
}

/// 全局唯一:管所有事务的生死 + 分配 txn_id
pub struct TransactionManager {
    next_txn_id: TxnId,
    active: HashMap<TxnId, Transaction>,
    log: Arc<LogManager>,
}

impl TransactionManager {
    pub fn new(log: Arc<LogManager>) -> Self {
        Self { next_txn_id: 1, active: HashMap::new(), log }
    }

    /// 开启事务:分配 id → 写 Begin 日志 → 登记活跃表 → 返回 txn_id
    // ... existing code ...
    /// 开启事务:分配 id → 写 Begin 日志 → 登记活跃表 → 返回 (txn_id, begin_lsn)
    pub fn begin(&mut self) -> (TxnId, Lsn) {
        let txn_id = self.next_txn_id;
        self.next_txn_id += 1;
        let lsn = self.log.append(&LogRecord::Begin { txn_id });
        self.active.insert(txn_id, Transaction { txn_id, status: TxnStatus::Active, last_lsn: lsn });
        (txn_id, lsn)
    }
    // ... existing code ...

    /// 声明回滚:写 Abort 日志 → 从 active 摘除。
    /// 注意：真正的 undo（把页字节改回去）发生在 Phase 4，这里只“登记它是输家”。
    pub fn abort(&mut self, txn_id: TxnId) {
        // TODO 你来写，3 步：
        // 1) let lsn = self.log.append(&LogRecord::Abort { txn_id });
        self.log.append(&LogRecord::Abort { txn_id });
        // 2) 依你对上面定向题的判断，决定要不要 self.log.flush_to(lsn)（要的话用 .expect(...)）
        // 3) self.active.remove(&txn_id);
        self.active.remove(&txn_id);
    }
    pub fn log_update(
        &mut self,
        txn_id: TxnId,
        page_id: PageId,
        offset: u16,
        before: Vec<u8>,
        after: Vec<u8>,
    ) -> Lsn {
        // TODO(你来写,3 步):
        // 1) 从 self.active 取该事务的 last_lsn 作 prev_lsn；取不到用 INVALID_LSN

        let prev_lsn =self.active.get(&txn_id).map(|t| t.last_lsn).unwrap_or(INVALID_LSN);


        //    提示：self.active.get(&txn_id) -> Option<&Transaction>，用 .map(|t| t.last_lsn).unwrap_or(INVALID_LSN)
        // 2) let lsn = self.log.append(&LogRecord::Update { txn_id, prev_lsn, page_id, offset, before, after });
        let lsn = self.log.append(&LogRecord::Update { txn_id, prev_lsn, page_id, offset, before, after });
        // 3) 用 self.active.get_mut(&txn_id) 拿到可变引用，把它的 last_lsn 改成 lsn；返回 lsn
        self.active.get_mut(&txn_id).unwrap().last_lsn = lsn;
        lsn
    }
    /// 提交:写 Commit → 从活跃表摘除(下一轮再写)
    pub fn commit(&mut self, txn_id: TxnId) {
       let lsn= self.log.append(&LogRecord::Commit { txn_id: txn_id });
        self.log.flush_to(lsn).expect("flush commit log failed");
        self.active.remove(&txn_id);

    }



}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::recovery::log_manager::LogManager;
    use std::sync::Arc;

    #[test]
    fn test_full_txn_lifecycle_chain() {
        std::fs::create_dir_all("./data").unwrap();
        let path = "./data/test_txn_lifecycle.log";
        let _ = std::fs::remove_file(path);

        let log = Arc::new(LogManager::from_path(path).unwrap());
        let mut tm = TransactionManager::new(log.clone());

        let (txn_id, _begin_lsn) = tm.begin();
        tm.log_update(txn_id, 7, 16, vec![0, 0, 0], vec![1, 2, 3]);
        tm.log_update(txn_id, 7, 24, vec![0, 0], vec![9, 8]);
        tm.commit(txn_id);

        // 从磁盘读回：Vec<(Lsn, LogRecord)>
        let records = LogManager::read_all(path).unwrap();

        // 1) 一共 4 条
        assert_eq!(records.len(), 4, "应有 Begin/Update/Update/Commit 四条");

        // 2) 顺序与类型
        assert!(matches!(&records[0].1, LogRecord::Begin { .. }), "第0条应为 Begin");
        assert!(matches!(&records[1].1, LogRecord::Update { .. }), "第1条应为 Update");
        assert!(matches!(&records[2].1, LogRecord::Update { .. }), "第2条应为 Update");
        assert!(matches!(&records[3].1, LogRecord::Commit { .. }), "第3条应为 Commit");

        // 3) 四条记录同属一个事务
        for (_, rec) in &records {
            assert_eq!(rec.txn_id(), Some(txn_id), "所有记录 txn_id 应一致");
        }

        // 4) 验链：每条 Update 的 prev_lsn 指向“本事务上一条记录”的 LSN
        if let LogRecord::Update { prev_lsn, .. } = &records[1].1 {
            assert_eq!(*prev_lsn, records[0].0, "第1条 Update.prev_lsn 应 = Begin 的 lsn");
        } else {
            panic!("records[1] 不是 Update");
        }
        if let LogRecord::Update { prev_lsn, .. } = &records[2].1 {
            assert_eq!(*prev_lsn, records[1].0, "第2条 Update.prev_lsn 应 = 第1条 Update 的 lsn");
        } else {
            panic!("records[2] 不是 Update");
        }
    }}