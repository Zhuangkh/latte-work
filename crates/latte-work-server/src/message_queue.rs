//! Durable FIFO admission; native launch snapshots remain memory-only.
use crate::{
    runtime::SendTurn,
    store::{Store, now},
};
use anyhow::{Result, bail};
use latte_work_protocol::{QueuedMessage, Response};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::HashSet;

pub fn initialize(db: &Connection) -> Result<()> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS message_queue(position INTEGER PRIMARY KEY AUTOINCREMENT,request_id TEXT NOT NULL UNIQUE,session_id TEXT NOT NULL REFERENCES sessions(id),admission TEXT NOT NULL,data TEXT NOT NULL,fingerprint TEXT,status TEXT NOT NULL DEFAULT 'pending');
    CREATE INDEX IF NOT EXISTS message_queue_session ON message_queue(session_id,status,position);
    CREATE TABLE IF NOT EXISTS message_queue_state(session_id TEXT PRIMARY KEY REFERENCES sessions(id),paused INTEGER NOT NULL DEFAULT 0,reason TEXT);
    UPDATE message_queue_state SET paused=1,reason='后台已重启，请核对当前任务后继续排队' WHERE EXISTS(SELECT 1 FROM message_queue WHERE message_queue.session_id=message_queue_state.session_id AND status='pending');")?;
    let has_order = db
        .prepare("PRAGMA table_info(message_queue)")?
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .iter()
        .any(|v| v == "sort_order");
    if !has_order {
        let tx = db.unchecked_transaction()?;
        tx.execute_batch(
            "ALTER TABLE message_queue ADD COLUMN sort_order INTEGER NOT NULL DEFAULT 0;
            UPDATE message_queue SET sort_order=position;",
        )?;
        tx.commit()?;
    }
    Ok(())
}
impl Store {
    pub fn queued(&self, id: &str) -> Result<Vec<(QueuedMessage, Option<String>)>> {
        self.session(id)?;
        self.db.prepare("SELECT data,fingerprint FROM message_queue WHERE session_id=?1 AND status='pending' ORDER BY sort_order,position LIMIT 16")?
            .query_map([id], |r| Ok((r.get::<_,String>(0)?,r.get::<_,Option<String>>(1)?)))?
            .map(|r| {let (data,fingerprint)=r?;Ok((serde_json::from_str(&data)?,fingerprint))}).collect()
    }
    pub fn queue_state(&self, id: &str) -> Result<(bool, Option<String>)> {
        Ok(self
            .db
            .query_row(
                "SELECT paused,reason FROM message_queue_state WHERE session_id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?
            .unwrap_or_default())
    }
    pub fn queue_snapshot(&self, id: &str) -> Result<Response> {
        let entries = self
            .queued(id)?
            .into_iter()
            .map(|(entry, _)| entry)
            .collect();
        let (paused, reason) = self.queue_state(id)?;
        Ok(Response::MessageQueue {
            entries,
            paused,
            reason,
        })
    }
    pub fn pause_queue(&self, id: &str, reason: Option<&str>) -> Result<()> {
        self.db.execute("INSERT INTO message_queue_state(session_id,paused,reason) VALUES (?1,?2,?3) ON CONFLICT(session_id) DO UPDATE SET paused=excluded.paused,reason=excluded.reason",params![id,reason.is_some(),reason])?;
        Ok(())
    }
    /// Reconcile the original admission even if an explicitly edited message later ran.
    pub fn queued_duplicate(&self, id: &str, turn: &SendTurn) -> Result<Option<Response>> {
        let row=self.db.query_row("SELECT session_id,admission,fingerprint,status FROM message_queue WHERE request_id=?1",[&turn.request_id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,Option<String>>(2)?,r.get::<_,String>(3)?))).optional()?;
        let Some((session, data, fingerprint, status)) = row else {
            return Ok(None);
        };
        let original: QueuedMessage = serde_json::from_str(&data)?;
        if session != id
            || original.text != turn.text
            || original.model != turn.model
            || original.effort != turn.config.effort
            || original.permission_mode != turn.config.permission_mode
            || fingerprint != turn.fingerprint
        {
            bail!("排队请求 ID 已被其他内容或配置使用，请核对原消息");
        }
        if status == "taken" {
            bail!("此排队消息已取出编辑，不会重新入队");
        }
        if status == "cancelled" {
            bail!("此排队消息已取消，不会重新入队");
        }
        Ok(Some(
            if matches!(status.as_str(), "dispatched" | "forwarded" | "uncertain") {
                Response::Accepted { duplicate: true }
            } else {
                Response::MessageQueued { duplicate: true }
            },
        ))
    }
    pub fn enqueue(&self, id: &str, turn: &SendTurn) -> Result<()> {
        if turn.text.trim().is_empty()
            || turn.text.len() > 128 * 1024
            || turn.request_id.is_empty()
            || turn.request_id.len() > 100
            || turn.model.as_ref().is_some_and(|v| v.len() > 256)
            || turn
                .config
                .permission_mode
                .as_ref()
                .is_some_and(|v| v.len() > 128)
        {
            bail!("排队消息内容无效（输入上限 128 KiB）");
        }
        if self.session(id)?.archived {
            bail!("请先取消归档再发送消息");
        }
        if self
            .queued(id)?
            .iter()
            .any(|(_, fingerprint)| *fingerprint != turn.fingerprint)
        {
            bail!("队列中已有其他 Provider 的消息，请待其完成或取消后再发送");
        }
        let count: u32 = self.db.query_row(
            "SELECT COUNT(*) FROM message_queue WHERE session_id=?1 AND status IN ('pending','forwarded')",
            [id],
            |r| r.get(0),
        )?;
        if count >= 16 {
            bail!("此会话最多排队 16 条消息");
        }
        let entry = QueuedMessage {
            request_id: turn.request_id.clone(),
            text: turn.text.clone(),
            model: turn.model.clone(),
            effort: turn.config.effort,
            permission_mode: turn.config.permission_mode.clone(),
            revision: 0,
            created_at: now(),
        };
        let data = serde_json::to_string(&entry)?;
        let size:u32=self.db.query_row("SELECT COALESCE(SUM(length(CAST(data AS BLOB))),0) FROM message_queue WHERE session_id=?1 AND status IN ('pending','forwarded')",[id],|r|r.get(0))?;
        if size as usize + data.len() > 512 * 1024 {
            bail!("排队内容已达 512 KiB，请等待执行或移除消息");
        }
        let tx = self.db.unchecked_transaction()?;
        tx.execute("INSERT INTO message_queue(request_id,session_id,admission,data,fingerprint,sort_order) VALUES (?1,?2,?3,?3,?4,(SELECT COALESCE(MAX(sort_order),0)+1 FROM message_queue WHERE session_id=?2 AND status='pending'))",params![turn.request_id,id,data,turn.fingerprint])?;
        tx.execute(
            "INSERT OR IGNORE INTO message_queue_state(session_id) VALUES (?1)",
            [id],
        )?;
        tx.commit()?;
        Ok(())
    }
    pub fn edit_queued(
        &self,
        id: &str,
        request_id: &str,
        revision: u32,
        text: Option<&str>,
    ) -> Result<()> {
        let old = self
            .queued(id)?
            .into_iter()
            .find(|(e, _)| e.request_id == request_id)
            .map(|(e, _)| e);
        let Some(mut entry) = old else {
            bail!("消息已开始执行或已移除，请刷新队列");
        };
        if entry.revision != revision {
            bail!("消息已被更新，请刷新后重试");
        }
        if let Some(text) = text {
            if text.trim().is_empty() || text.len() > 128 * 1024 {
                bail!("排队消息不能为空或超过 128 KiB");
            }
            entry.text = text.into();
            entry.revision = entry
                .revision
                .checked_add(1)
                .ok_or_else(|| anyhow::anyhow!("消息修改次数超出范围"))?;
            let data = serde_json::to_string(&entry)?;
            let size:u32=self.db.query_row("SELECT COALESCE(SUM(length(CAST(data AS BLOB))),0) FROM message_queue WHERE session_id=?1 AND status IN ('pending','forwarded') AND request_id<>?2",params![id,request_id],|r|r.get(0))?;
            if size as usize + data.len() > 512 * 1024 {
                bail!("排队内容已达 512 KiB");
            }
            self.db.execute("UPDATE message_queue SET data=?3 WHERE session_id=?1 AND request_id=?2 AND status='pending'",params![id,request_id,data])?;
        } else {
            self.db.execute("UPDATE message_queue SET status='cancelled' WHERE session_id=?1 AND request_id=?2 AND status='pending'",params![id,request_id])?;
        }
        Ok(())
    }
    /// Full-list compare-and-swap prevents stale clients from moving a dispatched
    /// or cancelled message back into the queue. Repeating a confirmed order is safe.
    pub fn reorder_queue(&self, id: &str, expected: &[String], order: &[String]) -> Result<()> {
        if expected.len() > 16
            || expected.len() != order.len()
            || expected
                .iter()
                .chain(order)
                .any(|v| v.is_empty() || v.len() > 100)
        {
            bail!("排队顺序无效，请刷新队列");
        }
        let before: HashSet<_> = expected.iter().collect();
        let after: HashSet<_> = order.iter().collect();
        if before.len() != expected.len() || after.len() != order.len() || before != after {
            bail!("排队消息必须完整且不能重复，请刷新队列");
        }
        let tx = self.db.unchecked_transaction()?;
        let current: Vec<_> = self
            .queued(id)?
            .into_iter()
            .map(|(e, _)| e.request_id)
            .collect();
        if current == order {
            return Ok(());
        }
        if current != expected {
            bail!("队列已变化，消息可能已开始执行，请刷新后重新排序");
        }
        for (rank, request_id) in order.iter().enumerate() {
            tx.execute("UPDATE message_queue SET sort_order=?3 WHERE session_id=?1 AND request_id=?2 AND status='pending'", params![id, request_id, rank as i64])?;
        }
        tx.commit()?;
        Ok(())
    }
    pub fn mark_forwarded(&self, id: &str, request_id: &str, revision: u32) -> Result<()> {
        let tx = self.db.unchecked_transaction()?;
        let (data, status): (String, String) = tx.query_row(
            "SELECT data,status FROM message_queue WHERE session_id=?1 AND request_id=?2",
            params![id, request_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let entry: QueuedMessage = serde_json::from_str(&data)?;
        if status != "pending" || entry.revision != revision {
            bail!("消息已发送、移除或更新，请刷新队列");
        }
        tx.execute(
            "UPDATE message_queue SET status='forwarded' WHERE session_id=?1 AND request_id=?2",
            params![id, request_id],
        )?;
        let event = latte_work_protocol::EventKind::QueuedMessageSent {
            text: entry.text,
            request_id: entry.request_id,
        };
        tx.execute(
            "INSERT INTO events(session_id,at,data) VALUES (?1,?2,?3)",
            params![id, now(), serde_json::to_string(&event)?],
        )?;
        // The CLI may start its next input before the host captures a turn boundary.
        let unavailable = crate::task_changes::Snapshot::unavailable(
            "Agent 已接收后续消息，无法为重叠执行提供安全的单轮撤销".into(),
        );
        tx.execute(
            "UPDATE turn_changes SET baseline=?2 WHERE session_id=?1 AND baseline IS NOT NULL",
            params![id, serde_json::to_string(&unavailable)?],
        )?;
        tx.commit()?;
        Ok(())
    }
    /// Retire only native-confirmed inputs, without admitting or writing a new turn.
    pub fn consume_forwarded(&self, id: &str, requests: &[String]) -> Result<()> {
        let tx = self.db.unchecked_transaction()?;
        for request in requests {
            tx.execute("UPDATE message_queue SET status='dispatched' WHERE session_id=?1 AND request_id=?2 AND status='forwarded'",params![id,request])?;
        }
        tx.commit()?;
        Ok(())
    }
    pub fn uncertain_forwarded(&self, id: &str) -> Result<()> {
        let tx = self.db.unchecked_transaction()?;
        let count=tx.execute("UPDATE message_queue SET status='uncertain' WHERE session_id=?1 AND status='forwarded'",[id])?;
        tx.commit()?;
        if count > 0 {
            self.event(id,latte_work_protocol::EventKind::Notice {text:format!("{count} 条已发送给 Agent 的消息处理状态待确认，请核对原生会话记录；不会自动重发。")})?;
        }
        Ok(())
    }
    pub fn take_queued(&self, id: &str, request_id: &str, revision: u32) -> Result<Response> {
        self.session(id)?;
        let tx = self.db.unchecked_transaction()?;
        let (data, status): (String, String) = tx.query_row(
            "SELECT data,status FROM message_queue WHERE session_id=?1 AND request_id=?2",
            params![id, request_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let entry: QueuedMessage = serde_json::from_str(&data)?;
        if !matches!(status.as_str(), "pending" | "taken") || entry.revision != revision {
            bail!("消息已执行、移除或更新，请刷新队列");
        }
        tx.execute(
            "UPDATE message_queue SET status='taken' WHERE session_id=?1 AND request_id=?2",
            params![id, request_id],
        )?;
        tx.commit()?;
        Ok(Response::QueuedMessageTaken { entry })
    }
    pub fn clear_queue(&self, id: &str) -> Result<()> {
        self.session(id)?;
        let tx = self.db.unchecked_transaction()?;
        tx.execute(
            "UPDATE message_queue SET status='cancelled' WHERE session_id=?1 AND status='pending'",
            [id],
        )?;
        tx.execute("DELETE FROM message_queue_state WHERE session_id=?1", [id])?;
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn consumed_handoff_retains_references_activity_and_identity_after_restart() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("state.sqlite");
        let store = Store::open(&path).unwrap();
        let project = store.add_project(root.path()).unwrap();
        let session = store.create_session(project.id, "claude".into()).unwrap();
        let text = "Original prompt\n引用（相对项目；绝对路径在当前主机，按现有权限读取）：\n[{\"type\":\"file\",\"path\":\"README.md\",\"name\":\"README.md\"}]";
        let entry = QueuedMessage {
            request_id: "consumed".into(),
            text: text.into(),
            model: None,
            effort: None,
            permission_mode: None,
            revision: 0,
            created_at: 1.0,
        };
        let data = serde_json::to_string(&entry).unwrap();
        store.db.execute("INSERT INTO message_queue(request_id,session_id,admission,data) VALUES ('consumed',?1,?2,?2)",params![session.id,data]).unwrap();
        store.mark_forwarded(&session.id, "consumed", 0).unwrap();
        store
            .consume_forwarded(&session.id, &["consumed".into(), "unknown".into()])
            .unwrap();
        store
            .consume_forwarded(&session.id, &["consumed".into()])
            .unwrap();
        drop(store);
        let store = Store::open(&path).unwrap();
        assert_eq!(
            store
                .db
                .query_row(
                    "SELECT status FROM message_queue WHERE request_id='consumed'",
                    [],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
            "dispatched"
        );
        let sources = store.sources(&session.id).unwrap().0;
        assert!(
            sources
                .iter()
                .any(|s| s.path.as_deref() == Some("README.md"))
        );
        let (recent, _) = store.recent_sessions(None).unwrap();
        let receipt_at:f64=store.db.query_row("SELECT at FROM events WHERE session_id=?1 AND json_extract(data,'$.kind')='queued_message_sent'",[&session.id],|r|r.get(0)).unwrap();
        assert_eq!(recent[0].updated_at, receipt_at);
        assert_eq!(store.db.query_row("SELECT COUNT(*) FROM events WHERE session_id=?1 AND json_extract(data,'$.kind') IN ('user','notice')",[&session.id],|r|r.get::<_,u32>(0)).unwrap(),0,"completion and restart do not invent a new user turn or uncertainty");
    }

    #[test]
    fn restart_quarantines_native_handoffs_and_preserves_the_prompt_receipt() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("state.sqlite");
        let store = Store::open(&path).unwrap();
        let project = store.add_project(root.path()).unwrap();
        let session = store.create_session(project.id, "claude".into()).unwrap();
        let entry = QueuedMessage {
            request_id: "sent".into(),
            text: "Native handoff original prompt".into(),
            model: None,
            effort: None,
            permission_mode: None,
            revision: 0,
            created_at: 1.0,
        };
        let data = serde_json::to_string(&entry).unwrap();
        store.db.execute("INSERT INTO message_queue(request_id,session_id,admission,data) VALUES ('sent',?1,?2,?2)",params![session.id,data]).unwrap();
        store.mark_forwarded(&session.id, "sent", 0).unwrap();
        drop(store);
        let store = Store::open(&path).unwrap();
        let status: String = store
            .db
            .query_row(
                "SELECT status FROM message_queue WHERE request_id='sent'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "uncertain");
        assert!(store.queued(&session.id).unwrap().is_empty());
        assert!(store.queue_state(&session.id).unwrap().0);
        let receipts = store
            .db
            .prepare("SELECT data FROM events WHERE session_id=?1")
            .unwrap()
            .query_map([&session.id], |r| r.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
            .join("\n");
        assert!(receipts.contains("Native handoff original prompt"));
        assert!(receipts.lines().any(|row| matches!(serde_json::from_str::<latte_work_protocol::EventKind>(row).unwrap(),
            latte_work_protocol::EventKind::QueuedMessageSent {request_id,text} if request_id=="sent" && text=="Native handoff original prompt")));
        assert!(receipts.contains("不会自动重发"));
    }
    #[test]
    fn legacy_queue_migration_preserves_fifo_and_saved_order_on_reopen() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE sessions(id TEXT PRIMARY KEY);
            INSERT INTO sessions(id) VALUES ('s');
            CREATE TABLE message_queue(position INTEGER PRIMARY KEY AUTOINCREMENT,request_id TEXT NOT NULL UNIQUE,session_id TEXT NOT NULL REFERENCES sessions(id),admission TEXT NOT NULL,data TEXT NOT NULL,fingerprint TEXT,status TEXT NOT NULL DEFAULT 'pending');
            INSERT INTO message_queue(position,request_id,session_id,admission,data) VALUES (7,'a','s','{}','{}'),(15,'b','s','{}','{}');").unwrap();
        initialize(&db).unwrap();
        let ranks = || {
            db.prepare("SELECT sort_order FROM message_queue ORDER BY position")
                .unwrap()
                .query_map([], |r| r.get::<_, i64>(0))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
        };
        assert_eq!(ranks(), [7, 15]);
        db.execute(
            "UPDATE message_queue SET sort_order=0 WHERE request_id='b'",
            [],
        )
        .unwrap();
        initialize(&db).unwrap();
        assert_eq!(
            ranks(),
            [7, 0],
            "reopening must not restore insertion order"
        );
    }
}
