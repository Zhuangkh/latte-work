//! SQLite is the source of truth for sessions and replayable presentation events.
use anyhow::{Context, Result, bail};
use latte_work_protocol::{
    Effort, Event, EventKind, Project, RecentCursor, RecentSession, Session, Status, Subagent,
    SubagentStatus, SubagentUpdate,
};
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

pub struct Store {
    pub(crate) db: Connection,
}
pub fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as f64
}
impl Store {
    #[cfg(windows)]
    pub fn windows_undo_stores(&self) -> Result<Vec<crate::managed_recovery::Registration>> {
        let mut statement = self.db.prepare("SELECT substr(store_id,1,257),substr(project_path,1,32769),substr(data,1,65537) FROM windows_undo_stores LIMIT 1001")?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if rows.len() > 1000 {
            bail!("Undo store registry exceeds the capture limit");
        }
        rows.into_iter()
            .map(|(store_id, project_path, data)| {
                if store_id.len() > 256 || project_path.len() > 32768 || data.len() > 64 * 1024 {
                    bail!("Undo registration exceeds the limit");
                }
                let record: crate::managed_recovery::Registration = serde_json::from_str(&data)?;
                if store_id != record.store_uuid || Path::new(&project_path) != record.project_path
                {
                    bail!("Undo registry keys do not match the registered identities");
                }
                Ok(record)
            })
            .collect()
    }
    #[cfg(windows)]
    pub fn register_windows_undo_store(
        &mut self,
        record: &crate::managed_recovery::Registration,
    ) -> Result<()> {
        let transaction = self.db.transaction()?;
        transaction.execute(
            "INSERT INTO windows_undo_stores(store_id,project_path,data) VALUES(?1,?2,?3)",
            params![
                record.store_uuid,
                record
                    .project_path
                    .to_str()
                    .context("project path is not UTF-8")?,
                serde_json::to_string(record)?
            ],
        )?;
        transaction.commit()?;
        Ok(())
    }
    pub fn open(path: &Path) -> Result<Self> {
        let db = Connection::open(path)?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000;
        CREATE TABLE IF NOT EXISTS projects(id TEXT PRIMARY KEY, name TEXT NOT NULL, path TEXT NOT NULL UNIQUE);
        CREATE TABLE IF NOT EXISTS hidden_projects(project_id TEXT PRIMARY KEY REFERENCES projects(id));
        CREATE TABLE IF NOT EXISTS sessions(id TEXT PRIMARY KEY, project_id TEXT NOT NULL REFERENCES projects(id), data TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS events(seq INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT NOT NULL REFERENCES sessions(id), at REAL NOT NULL, data TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS subagents(session_id TEXT NOT NULL REFERENCES sessions(id), id TEXT NOT NULL, native_id TEXT NOT NULL, tool_use_id TEXT, active INTEGER NOT NULL, at REAL NOT NULL, data TEXT NOT NULL, PRIMARY KEY(session_id,id));
        CREATE INDEX IF NOT EXISTS subagents_session ON subagents(session_id,active,at);
        CREATE INDEX IF NOT EXISTS subagents_native ON subagents(session_id,native_id);
        CREATE INDEX IF NOT EXISTS subagents_tool ON subagents(session_id,tool_use_id);
        CREATE TABLE IF NOT EXISTS subagent_events(seq INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT NOT NULL, task_id TEXT NOT NULL, at REAL NOT NULL, data TEXT NOT NULL, truncated INTEGER NOT NULL DEFAULT 0, FOREIGN KEY(session_id,task_id) REFERENCES subagents(session_id,id));
        CREATE INDEX IF NOT EXISTS subagent_events_task ON subagent_events(session_id,task_id,seq);
        CREATE TABLE IF NOT EXISTS subagent_history(session_id TEXT NOT NULL,task_id TEXT NOT NULL,source TEXT NOT NULL,seq REAL NOT NULL,at REAL NOT NULL,data TEXT NOT NULL,PRIMARY KEY(session_id,task_id,source,seq),FOREIGN KEY(session_id,task_id) REFERENCES subagents(session_id,id));
        CREATE TABLE IF NOT EXISTS subagent_history_sources(session_id TEXT NOT NULL,task_id TEXT NOT NULL,cursor TEXT NOT NULL,PRIMARY KEY(session_id,task_id),FOREIGN KEY(session_id,task_id) REFERENCES subagents(session_id,id));
        CREATE TABLE IF NOT EXISTS subagent_native_transcripts(session_id TEXT NOT NULL,task_id TEXT NOT NULL,data TEXT NOT NULL,PRIMARY KEY(session_id,task_id),FOREIGN KEY(session_id,task_id) REFERENCES subagents(session_id,id));
        CREATE INDEX IF NOT EXISTS events_session ON events(session_id,seq);
        CREATE TABLE IF NOT EXISTS turn_changes(session_id TEXT NOT NULL REFERENCES sessions(id), request_id TEXT NOT NULL REFERENCES requests(id), baseline TEXT, data TEXT, PRIMARY KEY(session_id,request_id));
        CREATE TABLE IF NOT EXISTS windows_undo_stores(store_id TEXT PRIMARY KEY, project_path TEXT NOT NULL UNIQUE, data TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS task_baselines(session_id TEXT PRIMARY KEY REFERENCES sessions(id), data TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS task_sources(session_id TEXT NOT NULL REFERENCES sessions(id), id TEXT NOT NULL, data TEXT NOT NULL, at REAL NOT NULL, PRIMARY KEY(session_id,id));
        CREATE TABLE IF NOT EXISTS observed_tools(session_id TEXT NOT NULL REFERENCES sessions(id), id TEXT NOT NULL, PRIMARY KEY(session_id,id));
        CREATE TABLE IF NOT EXISTS reference_access(project_id TEXT NOT NULL REFERENCES projects(id), path TEXT NOT NULL, directory INTEGER NOT NULL, PRIMARY KEY(project_id,path));
        CREATE TABLE IF NOT EXISTS requests(id TEXT PRIMARY KEY,session_id TEXT NOT NULL,text TEXT NOT NULL);")?;
        let migration = db.unchecked_transaction()?;
        migration.execute_batch("CREATE INDEX IF NOT EXISTS events_recent_activity_v2 ON events(session_id,seq DESC) WHERE json_extract(data,'$.kind') IN ('user','queued_message_sent','text','tool','tool_result','approval','approval_resolved','subagent');
        DROP INDEX IF EXISTS events_recent_activity;")?;
        migration.commit()?;
        let columns = db
            .prepare("PRAGMA table_info(requests)")?
            .query_map([], |r| r.get::<_, String>(1))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if !columns.iter().any(|name| name == "model") {
            db.execute("ALTER TABLE requests ADD COLUMN model TEXT", [])?;
        }
        if !columns.iter().any(|name| name == "effort") {
            db.execute("ALTER TABLE requests ADD COLUMN effort TEXT", [])?;
        }
        if !columns.iter().any(|name| name == "provider_fingerprint") {
            db.execute(
                "ALTER TABLE requests ADD COLUMN provider_fingerprint TEXT",
                [],
            )?;
        }
        if !columns.iter().any(|name| name == "permission_mode") {
            db.execute("ALTER TABLE requests ADD COLUMN permission_mode TEXT", [])?;
        }
        crate::message_queue::initialize(&db)?;
        let store = Self { db };
        let forwarded = store
            .db
            .prepare("SELECT DISTINCT session_id FROM message_queue WHERE status='forwarded'")?
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for id in forwarded {
            store.uncertain_forwarded(&id)?;
            store.pause_queue(&id, Some("后台已重启，请核对已发送的消息"))?;
        }
        // Native child processes cannot survive a daemon restart, even if the main
        // turn had already completed while background work continued.
        let child_sessions = store
            .db
            .prepare("SELECT DISTINCT session_id FROM subagents WHERE active=1")?
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for id in child_sessions {
            store.end_subagents(&id, SubagentStatus::Unknown)?;
        }
        let pending = store
            .db
            .prepare("SELECT session_id,request_id FROM turn_changes WHERE baseline IS NOT NULL")?
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (id, request_id) in pending {
            let summary = latte_work_protocol::ChangeSummary {
                entries: vec![],
                added: 0,
                removed: 0,
                binary_files: 0,
                truncated: false,
                baseline_at: None,
                unavailable: Some("Server 已重启，此轮结束时的文件快照未知；请核对工作区。".into()),
            };
            store.event(
                &id,
                EventKind::TurnChanges {
                    changes: latte_work_protocol::TurnChanges {
                        request_id: request_id.clone(),
                        summary,
                        interrupted: true,
                        background_pending: false,
                        undo: latte_work_protocol::TurnUndoStatus::Unknown,
                        undo_unavailable: None,
                    },
                },
            )?;
            store.db.execute(
                "UPDATE turn_changes SET baseline=NULL WHERE session_id=?1 AND request_id=?2",
                params![id, request_id],
            )?;
        }
        let interrupted = store.all_sessions()?;
        for mut session in interrupted.into_iter().filter(|s| s.status.active()) {
            session.status = Status::Unknown;
            store.save(&session)?;
            store.event(
                &session.id,
                EventKind::State {
                    status: Status::Unknown,
                    message: Some("Server 已重启，先前执行结果未知；请核对文件后继续。".into()),
                },
            )?;
        }
        Ok(store)
    }
    pub fn projects(&self) -> Result<Vec<Project>> {
        let hidden = self
            .db
            .prepare("SELECT project_id FROM hidden_projects")?
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(self
            .all_projects()?
            .into_iter()
            .filter(|p| !hidden.contains(&p.id))
            .collect())
    }
    fn all_projects(&self) -> Result<Vec<Project>> {
        Ok(self
            .db
            .prepare("SELECT id,name,path FROM projects ORDER BY name")?
            .query_map([], |r| {
                Ok(Project {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    path: r.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?)
    }
    pub fn project(&self, id: &str) -> Result<Project> {
        self.all_projects()?
            .into_iter()
            .find(|p| p.id == id)
            .context("项目不存在")
    }
    #[cfg(test)]
    pub fn add_project(&self, path: &Path) -> Result<Project> {
        self.add_named_project(path, None)
    }
    pub fn add_named_project(&self, path: &Path, name: Option<&str>) -> Result<Project> {
        let name = name.map(str::trim).filter(|v| !v.is_empty());
        if name.is_some_and(|v| v.chars().count() > 100 || v.chars().any(char::is_control)) {
            bail!("项目名称不能超过 100 个字符或包含控制字符");
        }
        if !path.is_absolute() {
            bail!("项目路径必须是绝对路径");
        }
        latte_work_platform::paths::absolute(path)?;
        let canonical = path.canonicalize().context("项目目录不存在或无法访问")?;
        if !canonical.is_dir() {
            bail!("请选择目录");
        }
        let path = canonical.to_str().context("项目路径不是 UTF-8")?.to_owned();
        if let Some(existing) = self.all_projects()?.into_iter().find(|p| p.path == path) {
            self.db.execute(
                "DELETE FROM hidden_projects WHERE project_id=?1",
                [&existing.id],
            )?;
            return Ok(existing);
        }
        let project = Project {
            id: Uuid::new_v4().to_string(),
            name: name.map(str::to_owned).unwrap_or_else(|| {
                canonical
                    .file_name()
                    .unwrap_or_default()
                    .to_str()
                    .expect("canonical path checked as UTF-8")
                    .to_owned()
            }),
            path,
        };
        self.db.execute(
            "INSERT INTO projects VALUES (?1,?2,?3)",
            params![project.id, project.name, project.path],
        )?;
        Ok(project)
    }
    pub fn rename_project(&self, id: &str, name: &str) -> Result<Project> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 100 || name.chars().any(char::is_control) {
            bail!("项目名称不能为空、超过 100 个字符或包含控制字符");
        }
        let mut project = self.project(id)?;
        self.db
            .execute("UPDATE projects SET name=?1 WHERE id=?2", params![name, id])?;
        project.name = name.into();
        Ok(project)
    }
    pub fn remove_project(&self, id: &str) -> Result<()> {
        self.project(id)?;
        if self
            .all_sessions()?
            .iter()
            .any(|s| s.project_id == id && s.status.active())
        {
            bail!("项目中还有运行中的任务，请等待完成或停止后再移除");
        }
        self.db.execute(
            "INSERT OR IGNORE INTO hidden_projects(project_id) VALUES(?1)",
            [id],
        )?;
        Ok(())
    }
    pub fn has_active_sessions(&self) -> Result<bool> {
        Ok(self.db.query_row("SELECT EXISTS(SELECT 1 FROM sessions WHERE json_extract(data, '$.status') IN ('running', 'waiting'))", [], |row| row.get(0))?)
    }
    fn all_sessions(&self) -> Result<Vec<Session>> {
        let rows = self
            .db
            .prepare("SELECT data FROM sessions ORDER BY rowid DESC")?
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|s| Ok(serde_json::from_str(&s)?))
            .collect()
    }
    pub fn sessions(&self, project: &str) -> Result<Vec<Session>> {
        self.project(project)?;
        Ok(self
            .all_sessions()?
            .into_iter()
            .filter(|s| s.project_id == project)
            .take(500)
            .collect())
    }
    pub fn recent_sessions(
        &self,
        before: Option<&RecentCursor>,
    ) -> Result<(Vec<RecentSession>, Option<RecentCursor>)> {
        if before.is_some_and(|cursor| {
            !cursor.updated_at.is_finite()
                || cursor.updated_at < 0.0
                || cursor.id.is_empty()
                || cursor.id.len() > 128
        }) {
            bail!("最近记录游标无效");
        }
        // Lifecycle/reading metadata does not make an old conversation recent.
        // The partial event index also supports histories created before this feature.
        let rows = self.db.prepare("WITH recent AS (
            SELECT s.id,s.data,COALESCE((SELECT e.at FROM events e WHERE e.session_id=s.id
                AND json_extract(e.data,'$.kind') IN ('user','queued_message_sent','text','tool','tool_result','approval','approval_resolved','subagent')
                ORDER BY e.seq DESC LIMIT 1),json_extract(s.data,'$.created_at')) AS updated_at
            FROM sessions s WHERE COALESCE(json_extract(s.data,'$.archived'),0)=0
                AND s.project_id NOT IN (SELECT project_id FROM hidden_projects))
            SELECT data,updated_at FROM recent
            WHERE ?1 IS NULL OR updated_at<?1 OR (updated_at=?1 AND id<?2)
            ORDER BY updated_at DESC,id DESC LIMIT 101")?
            .query_map(params![before.map(|cursor|cursor.updated_at), before.map(|cursor|cursor.id.as_str())], |row| Ok((row.get::<_,String>(0)?, row.get::<_,f64>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut sessions = rows
            .into_iter()
            .map(|(data, updated_at)| {
                Ok(RecentSession {
                    session: serde_json::from_str(&data)?,
                    updated_at,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let has_more = sessions.len() > 100;
        sessions.truncate(100);
        let next = has_more.then(|| {
            let last = sessions.last().expect("full recent page");
            RecentCursor {
                updated_at: last.updated_at,
                id: last.session.id.clone(),
            }
        });
        Ok((sessions, next))
    }
    pub fn session(&self, id: &str) -> Result<Session> {
        let data: String = self
            .db
            .query_row("SELECT data FROM sessions WHERE id=?1", [id], |r| r.get(0))
            .context("会话不存在")?;
        Ok(serde_json::from_str(&data)?)
    }
    pub fn create_session(&self, project_id: String, agent: String) -> Result<Session> {
        self.project(&project_id)?;
        let session = Session {
            id: Uuid::new_v4().to_string(),
            project_id,
            title: "新任务".into(),
            agent,
            native_id: None,
            model: None,
            effort: None,
            permission_mode: None,
            status: Status::Ready,
            created_at: now(),
            custom_title: false,
            pinned_at: None,
            unread: false,
            archived: false,
            agent_session_open: None,
            agent_session_busy: None,
        };
        self.db.execute(
            "INSERT INTO sessions VALUES (?1,?2,?3)",
            params![
                session.id,
                session.project_id,
                serde_json::to_string(&session)?
            ],
        )?;
        Ok(session)
    }
    pub fn pinned_sessions(&self) -> Result<Vec<Session>> {
        let rows = self.db.prepare("SELECT data FROM sessions WHERE json_extract(data, '$.pinned_at') IS NOT NULL AND COALESCE(json_extract(data, '$.archived'),0)=0 AND project_id NOT IN (SELECT project_id FROM hidden_projects) ORDER BY json_extract(data, '$.pinned_at') DESC LIMIT 100")?
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|s| Ok(serde_json::from_str(&s)?))
            .collect()
    }
    pub fn sync_live_native_title(&self, id: &str, title: &str) -> Result<Session> {
        // Explicit native names remain authoritative after a previous manual
        // rename. AI-generated titles use sync_native_title(..., false).
        self.sync_native_title(id, title, true)
    }
    pub fn sync_native_title(&self, id: &str, title: &str, custom: bool) -> Result<Session> {
        let mut session = self.session(id)?;
        if title.trim().is_empty()
            || title.chars().count() > 200
            || title.chars().any(char::is_control)
        {
            return Ok(session);
        }
        if session.custom_title && !custom {
            return Ok(session);
        }
        if session.title != title || (custom && !session.custom_title) {
            session.title = title.into();
            session.custom_title |= custom;
            self.save(&session)?;
        }
        Ok(session)
    }
    pub fn validate_session_title(title: &str) -> Result<&str> {
        let title = title.trim();
        if title.is_empty() || title.chars().count() > 100 || title.chars().any(char::is_control) {
            bail!("会话名称不能为空、超过 100 个字符或包含控制字符");
        }
        Ok(title)
    }
    pub fn rename_session(&self, id: &str, title: &str) -> Result<Session> {
        let title = Self::validate_session_title(title)?;
        let mut session = self.session(id)?;
        session.title = title.into();
        session.custom_title = true;
        self.save(&session)?;
        Ok(session)
    }
    pub fn pin_session(&self, id: &str, pinned: bool) -> Result<Session> {
        let mut session = self.session(id)?;
        if pinned && session.archived {
            bail!("请先取消归档再置顶");
        }
        if pinned && session.pinned_at.is_none() && self.pinned_sessions()?.len() >= 100 {
            bail!("每台主机最多置顶 100 个会话");
        }
        session.pinned_at = if pinned {
            Some(session.pinned_at.unwrap_or_else(now))
        } else {
            None
        };
        self.save(&session)?;
        Ok(session)
    }
    pub fn mark_session_unread(&self, id: &str, unread: bool) -> Result<Session> {
        let mut session = self.session(id)?;
        session.unread = unread;
        self.save(&session)?;
        Ok(session)
    }
    pub fn archive_session(&self, id: &str, archived: bool) -> Result<Session> {
        let mut session = self.session(id)?;
        if archived && session.status.active() {
            bail!("请先停止任务，再归档会话");
        }
        session.archived = archived;
        if archived {
            session.pinned_at = None;
        }
        self.save(&session)?;
        Ok(session)
    }
    pub fn save(&self, session: &Session) -> Result<()> {
        self.db.execute(
            "UPDATE sessions SET data=?2 WHERE id=?1",
            params![session.id, serde_json::to_string(session)?],
        )?;
        Ok(())
    }
    pub fn subagents(&self, id: &str) -> Result<(Vec<Subagent>, bool)> {
        let mut tasks = self.db.prepare("SELECT data FROM subagents WHERE session_id=?1 ORDER BY active DESC,at DESC,id LIMIT 129")?.query_map([id], |row| row.get::<_, String>(0))?.map(|row| Ok(serde_json::from_str(&row?)?)).collect::<Result<Vec<Subagent>>>()?;
        let truncated = tasks.len() > 128;
        tasks.truncate(128);
        Ok((tasks, truncated))
    }
    pub fn end_subagents(&self, id: &str, status: SubagentStatus) -> Result<()> {
        let tasks = self
            .db
            .prepare("SELECT data FROM subagents WHERE session_id=?1 AND active=1")?
            .query_map([id], |row| row.get::<_, String>(0))?
            .map(|row| Ok(serde_json::from_str(&row?)?))
            .collect::<Result<Vec<Subagent>>>()?;
        for task in tasks {
            self.event(
                id,
                EventKind::Subagent {
                    update: SubagentUpdate {
                        agent_type: None,
                        id: task.native_id,
                        tool_use_id: task.tool_use_id,
                        title: None,
                        status: Some(status),
                        summary: None,
                        last_tool: None,
                    },
                },
            )?;
        }
        Ok(())
    }
    pub fn subagent_event(
        &self,
        session_id: &str,
        tool_use_id: &str,
        event: EventKind,
    ) -> Result<()> {
        let tx = self.db.unchecked_transaction()?;
        let task_id: String = tx.query_row("SELECT id FROM subagents WHERE session_id=?1 AND tool_use_id=?2 ORDER BY at DESC LIMIT 1", params![session_id, tool_use_id], |row| row.get(0)).optional()?.context("子智能体输出无法关联到原生任务")?;
        // Keep the full redacted stream separately from the legacy bounded snapshot.
        let seq: f64 = tx.query_row("SELECT COALESCE(MAX(seq),0)+1 FROM subagent_history WHERE session_id=?1 AND task_id=?2 AND source='stream'",params![session_id,task_id],|row| row.get(0))?;
        tx.execute("INSERT INTO subagent_history(session_id,task_id,source,seq,at,data) VALUES (?1,?2,'stream',?3,?4,?5)",params![session_id,task_id,seq,now(),serde_json::to_string(&event)?])?;
        if let EventKind::ToolResult { id, .. } = &event {
            let tool:Option<String>=tx.query_row("SELECT data FROM subagent_history WHERE session_id=?1 AND task_id=?2 AND source='stream' AND json_extract(data,'$.kind')='tool' AND json_extract(data,'$.id')=?3 ORDER BY seq DESC LIMIT 1",params![session_id,task_id,id],|r|r.get(0)).optional()?;
            if let Some(tool) = tool
                && let Some(text) =
                    crate::agents::confirmed_child_report(&serde_json::from_str(&tool)?, &event)
            {
                tx.execute("INSERT INTO subagent_history(session_id,task_id,source,seq,at,data) VALUES (?1,?2,'stream',?3,?4,?5)",params![session_id,task_id,seq+1.0,now(),serde_json::to_string(&EventKind::Text{text})?])?;
            }
        }
        let (mut event, mut truncated) = bounded_subagent_event(event)?;
        if let EventKind::Text { text } = &event {
            let previous: Option<(i64, String, bool)> = tx.query_row("SELECT seq,data,truncated FROM subagent_events WHERE session_id=?1 AND task_id=?2 ORDER BY seq DESC LIMIT 1", params![session_id, task_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).optional()?;
            if let Some((seq, data, partial)) = previous
                && let EventKind::Text { text: mut joined } = serde_json::from_str(&data)?
            {
                if !partial {
                    joined.push_str(text);
                }
                let bounded = bounded_subagent_event(EventKind::Text { text: joined })?;
                event = bounded.0;
                truncated |= partial || bounded.1;
                tx.execute(
                    "UPDATE subagent_events SET at=?2,data=?3,truncated=?4 WHERE seq=?1",
                    params![seq, now(), serde_json::to_string(&event)?, truncated],
                )?;
                tx.commit()?;
                return Ok(());
            }
        }
        tx.execute("INSERT INTO subagent_events(session_id,task_id,at,data,truncated) VALUES (?1,?2,?3,?4,?5)", params![session_id, task_id, now(), serde_json::to_string(&event)?, truncated])?;
        tx.commit()?;
        Ok(())
    }
    pub fn subagent(&self, session_id: &str, task_id: &str) -> Result<Subagent> {
        self.session(session_id)?;
        let data: String = self
            .db
            .query_row(
                "SELECT data FROM subagents WHERE session_id=?1 AND id=?2",
                params![session_id, task_id],
                |row| row.get(0),
            )
            .optional()?
            .context("此对话没有对应的子智能体")?;
        Ok(serde_json::from_str(&data)?)
    }
    pub fn native_subagent(&self, session_id: &str, native_id: &str) -> Result<Subagent> {
        let data: String = self.db.query_row("SELECT data FROM subagents WHERE session_id=?1 AND (native_id=?2 OR id=?2) ORDER BY at DESC LIMIT 1", params![session_id,native_id], |row| row.get(0)).optional()?.context("子智能体原生记录不存在")?;
        Ok(serde_json::from_str(&data)?)
    }
    /// Bind historical native reads to the accepted request before this child
    /// started. Only its digest is stored; credential snapshots stay ephemeral.
    pub fn child_provider_fingerprint(
        &self,
        session_id: &str,
        task_id: &str,
    ) -> Result<Option<Option<String>>> {
        let task = self.subagent(session_id, task_id)?;
        let Some(started) = task.started_at else {
            return Ok(None);
        };
        self.db.query_row("SELECT r.provider_fingerprint FROM requests r JOIN events e ON e.session_id=r.session_id AND json_extract(e.data,'$.request_id')=r.id WHERE r.session_id=?1 AND json_extract(e.data,'$.kind')='user' AND e.at<=?2 ORDER BY e.seq DESC LIMIT 1",params![session_id,started],|row|row.get(0)).optional().map_err(Into::into)
    }
    pub fn child_history_cursor(
        &self,
        session_id: &str,
        task_id: &str,
    ) -> Result<crate::agents::ChildHistoryCursor> {
        self.subagent(session_id, task_id)?;
        let data: Option<String> = self
            .db
            .query_row(
                "SELECT cursor FROM subagent_history_sources WHERE session_id=?1 AND task_id=?2",
                params![session_id, task_id],
                |row| row.get(0),
            )
            .optional()?;
        data.map(|s| serde_json::from_str(&s).map_err(Into::into))
            .transpose()
            .map(|c| c.unwrap_or_default())
    }
    pub fn cache_child_history(
        &self,
        session_id: &str,
        task_id: &str,
        page: &crate::agents::ChildHistoryPage,
    ) -> Result<()> {
        self.subagent(session_id, task_id)?;
        let tx = self.db.unchecked_transaction()?;
        for e in &page.events {
            if e.session_id != session_id {
                bail!("子智能体过程会话身份不匹配");
            }
            tx.execute("INSERT INTO subagent_history(session_id,task_id,source,seq,at,data) VALUES (?1,?2,'native',?3,?4,?5) ON CONFLICT(session_id,task_id,source,seq) DO NOTHING",params![session_id,task_id,e.seq,e.at,serde_json::to_string(&e.event)?])?;
        }
        tx.execute("INSERT INTO subagent_history_sources(session_id,task_id,cursor) VALUES (?1,?2,?3) ON CONFLICT(session_id,task_id) DO UPDATE SET cursor=excluded.cursor WHERE json_extract(subagent_history_sources.cursor,'$.offset')<=json_extract(excluded.cursor,'$.offset')",params![session_id,task_id,serde_json::to_string(&page.cursor)?])?;
        tx.commit()?;
        Ok(())
    }
    pub fn subagent_history(
        &self,
        session_id: &str,
        task_id: &str,
        after: Option<f64>,
        requested_source: Option<&str>,
        source_has_more: bool,
    ) -> Result<latte_work_protocol::Response> {
        let task = self.subagent(session_id, task_id)?;
        // Read the original, already-redacted parent tool input independently of UI history pages.
        let prompt: Option<String> = if let Some(tool) = task.tool_use_id {
            self.db.query_row(
                "SELECT json_extract(data,'$.input.prompt') FROM events WHERE session_id=?1 AND json_extract(data,'$.kind')='tool' AND json_extract(data,'$.name') IN ('Agent','Task') AND json_extract(data,'$.id')=?2 AND json_type(data,'$.input.prompt')='text' ORDER BY seq DESC LIMIT 1",
                params![session_id, tool], |row| row.get(0),
            ).optional()?.flatten()
        } else {
            None
        };
        if prompt
            .as_ref()
            .is_some_and(|text| text.len() > latte_work_protocol::MAX_FRAME / 2)
        {
            bail!("子智能体任务指令超出传输范围，内容未截断");
        }
        if after.is_some_and(|v| !v.is_finite() || v < 0.0) {
            bail!("子智能体事件游标无效");
        }
        let native:bool=self.db.query_row("SELECT EXISTS(SELECT 1 FROM subagent_history_sources WHERE session_id=?1 AND task_id=?2) AND EXISTS(SELECT 1 FROM subagent_history WHERE session_id=?1 AND task_id=?2 AND source='native')",params![session_id,task_id],|r|r.get(0))?;
        let stream:bool=self.db.query_row("SELECT EXISTS(SELECT 1 FROM subagent_history WHERE session_id=?1 AND task_id=?2 AND source='stream')",params![session_id,task_id],|r|r.get(0))?;
        if !native && !stream {
            // Pre-upgrade snapshots are already redacted, but discarded fields
            // cannot be recovered if the owning native context is unavailable.
            let snapshot = self.subagent_transcript(session_id, task_id)?;
            let (events, incomplete) = crate::agents::normalize_child_transcript(
                &self.session(session_id)?.agent,
                snapshot,
            )?;
            let after = if requested_source == Some("legacy") {
                after
            } else {
                None
            };
            let mut page: Vec<Event> = events
                .into_iter()
                .filter(|e| e.seq > after.unwrap_or(-1.0))
                .take(129)
                .collect();
            let has_more = page.len() > 128;
            page.truncate(128);
            let after = page.last().map(|e| e.seq).or(after);
            return Ok(latte_work_protocol::Response::SubagentHistory {
                prompt,
                events: page,
                has_more,
                after,
                source_has_more: false,
                incomplete,
                source: "legacy".into(),
            });
        }
        let source = if native { "native" } else { "stream" };
        let after = if requested_source == Some(source) {
            after
        } else {
            None
        };
        let mut statement=self.db.prepare("SELECT seq,at,data FROM subagent_history WHERE session_id=?1 AND task_id=?2 AND source=?3 AND seq>?4 ORDER BY seq ASC LIMIT 129")?;
        let mut rows =
            statement.query(params![session_id, task_id, source, after.unwrap_or(-1.0)])?;
        let (mut events, mut bytes, mut has_more) = (Vec::new(), 0, false);
        while let Some(row) = rows.next()? {
            let data: String = row.get(2)?;
            let size = data.len() + session_id.len() + 128;
            if events.len() == 128 || (bytes + size > 256 * 1024 && !events.is_empty()) {
                has_more = true;
                break;
            }
            if size + prompt.as_ref().map_or(0, String::len) > latte_work_protocol::MAX_FRAME - 8192
            {
                bail!("单条子智能体事件超出传输范围，内容未截断");
            }
            bytes += size;
            events.push(Event {
                seq: row.get(0)?,
                session_id: session_id.into(),
                at: row.get(1)?,
                event: serde_json::from_str(&data)?,
            });
        }
        let after = events.last().map(|e| e.seq).or(after);
        let cursor = self.child_history_cursor(session_id, task_id)?;
        let incomplete = native
            && ((cursor.has_more && !source_has_more)
                || (cursor.pending_tail && !self.subagent(session_id, task_id)?.status.active()));
        Ok(latte_work_protocol::Response::SubagentHistory {
            prompt,
            events,
            has_more,
            after,
            source_has_more,
            incomplete,
            source: source.into(),
        })
    }
    pub fn cache_subagent_transcript(
        &self,
        session_id: &str,
        task_id: &str,
        transcript: &crate::agents::ChildTranscript,
    ) -> Result<()> {
        self.subagent(session_id, task_id)?;
        if transcript.0.iter().any(|e| e.session_id != session_id) {
            bail!("子智能体过程会话身份不匹配");
        }
        let data = serde_json::to_string(transcript)?;
        if data.len() > 512 * 1024 || transcript.0.len() > 500 {
            bail!("子智能体过程超出缓存范围");
        }
        self.db.execute("INSERT INTO subagent_native_transcripts(session_id,task_id,data) VALUES (?1,?2,?3) ON CONFLICT(session_id,task_id) DO UPDATE SET data=excluded.data", params![session_id,task_id,data])?;
        Ok(())
    }
    /// Bounded child snapshot; streaming fragments are coalesced on write.
    pub fn subagent_transcript(
        &self,
        session_id: &str,
        task_id: &str,
    ) -> Result<(Vec<Event>, bool)> {
        self.session(session_id)?;
        let cached: Option<String> = self
            .db
            .query_row(
                "SELECT data FROM subagent_native_transcripts WHERE session_id=?1 AND task_id=?2",
                params![session_id, task_id],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(cached) = cached {
            return Ok(serde_json::from_str(&cached)?);
        }
        let exists: bool = self.db.query_row(
            "SELECT EXISTS(SELECT 1 FROM subagents WHERE session_id=?1 AND id=?2)",
            params![session_id, task_id],
            |row| row.get(0),
        )?;
        if !exists {
            bail!("此对话没有对应的子智能体");
        }
        let mut statement = self.db.prepare("SELECT seq,at,data,truncated FROM subagent_events WHERE session_id=?1 AND task_id=?2 ORDER BY seq DESC LIMIT 501")?;
        let mut rows = statement.query(params![session_id, task_id])?;
        let (mut events, mut bytes, mut truncated) = (Vec::new(), 0, false);
        while let Some(row) = rows.next()? {
            let data: String = row.get(2)?;
            if events.len() == 500 || bytes + data.len() + 128 > 256 * 1024 {
                truncated = true;
                break;
            }
            bytes += data.len() + 128;
            truncated |= row.get::<_, bool>(3)?;
            events.push(Event {
                seq: row.get::<_, i64>(0)? as f64,
                session_id: session_id.into(),
                at: row.get(1)?,
                event: serde_json::from_str(&data)?,
            });
        }
        events.reverse();
        Ok((events, truncated))
    }
    pub fn event(&self, id: &str, mut event: EventKind) -> Result<()> {
        let at = now();
        let tx = self.db.unchecked_transaction()?;
        if let EventKind::Subagent { update } = &mut event {
            let data = tx.query_row("SELECT data FROM subagents WHERE session_id=?1 AND (id=?2 OR native_id=?2 OR (tool_use_id IS NOT NULL AND tool_use_id=?3)) ORDER BY at DESC LIMIT 1", params![id, update.id, update.tool_use_id], |row| row.get::<_, String>(0)).optional()?;
            let mut task = if let Some(data) = data {
                serde_json::from_str::<Subagent>(&data)?
            } else {
                Subagent {
                    id: update.id.clone(),
                    native_id: update.id.clone(),
                    tool_use_id: update.tool_use_id.clone(),
                    title: "子智能体".into(),
                    agent_type: None,
                    status: SubagentStatus::Unknown,
                    summary: None,
                    last_tool: None,
                    started_at: None,
                    updated_at: at,
                }
            };
            task.native_id = update.id.clone();
            if let Some(tool) = &update.tool_use_id {
                task.tool_use_id = Some(tool.clone());
            }
            if let Some(title) = &update.title {
                task.title = title.clone();
            }
            if let Some(agent_type) = &update.agent_type {
                task.agent_type = Some(agent_type.clone());
            }
            if let Some(status) = update.status {
                // Out-of-order progress cannot resurrect a confirmed terminal task.
                if task.status.active()
                    || task.status == SubagentStatus::Unknown
                    || !status.active()
                {
                    task.status = status;
                }
                if status.active() && task.started_at.is_none() {
                    task.started_at = Some(at);
                }
            }
            if let Some(summary) = &update.summary {
                task.summary = Some(summary.clone());
            }
            if let Some(tool) = &update.last_tool {
                task.last_tool = Some(tool.clone());
            }
            task.updated_at = at;
            tx.execute("INSERT INTO subagents(session_id,id,native_id,tool_use_id,active,at,data) VALUES (?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(session_id,id) DO UPDATE SET native_id=excluded.native_id,tool_use_id=excluded.tool_use_id,active=excluded.active,at=excluded.at,data=excluded.data", params![id, task.id, task.native_id, task.tool_use_id, task.status.active(), at, serde_json::to_string(&task)?])?;
            update.id = task.id;
        }
        tx.execute(
            "INSERT INTO events(session_id,at,data) VALUES (?1,?2,?3)",
            params![id, at, serde_json::to_string(&event)?],
        )?;
        index_event(&tx, id, &event)?;
        tx.commit()?;
        Ok(())
    }
    pub fn baseline(&self, id: &str) -> Result<Option<crate::task_changes::Snapshot>> {
        self.session(id)?;
        self.db
            .query_row(
                "SELECT data FROM task_baselines WHERE session_id=?1",
                [id],
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .map(|s| serde_json::from_str(&s).map_err(Into::into))
            .transpose()
    }
    pub fn save_baseline(&self, id: &str, snapshot: &crate::task_changes::Snapshot) -> Result<()> {
        self.db.execute(
            "INSERT OR IGNORE INTO task_baselines(session_id,data) VALUES (?1,?2)",
            params![id, serde_json::to_string(snapshot)?],
        )?;
        Ok(())
    }
    pub fn save_turn_baseline(
        &self,
        id: &str,
        request_id: &str,
        baseline: &crate::task_changes::Snapshot,
    ) -> Result<()> {
        self.db.execute(
            "INSERT OR IGNORE INTO turn_changes(session_id,request_id,baseline) VALUES (?1,?2,?3)",
            params![id, request_id, serde_json::to_string(baseline)?],
        )?;
        Ok(())
    }
    pub fn pending_turn(
        &self,
        id: &str,
    ) -> Result<Option<(String, crate::task_changes::Snapshot)>> {
        self.db.query_row("SELECT request_id,baseline FROM turn_changes WHERE session_id=?1 AND baseline IS NOT NULL ORDER BY rowid DESC LIMIT 1",[id],
            |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?))).optional()?
            .map(|(id,data)| Ok((id,serde_json::from_str(&data)?))).transpose()
    }
    pub fn finish_turn(&self, id: &str, turn: &crate::task_changes::FrozenTurn) -> Result<()> {
        let tx = self.db.unchecked_transaction()?;
        let updated = tx.execute("UPDATE turn_changes SET baseline=NULL,data=?3 WHERE session_id=?1 AND request_id=?2 AND baseline IS NOT NULL",
            params![id,turn.changes.request_id,serde_json::to_string(turn)?])?;
        if updated == 1 {
            tx.execute(
                "INSERT INTO events(session_id,at,data) VALUES (?1,?2,?3)",
                params![
                    id,
                    now(),
                    serde_json::to_string(&EventKind::TurnChanges {
                        changes: turn.changes.clone()
                    })?
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
    pub fn turn_changes(
        &self,
        id: &str,
        request_id: &str,
    ) -> Result<crate::task_changes::FrozenTurn> {
        self.session(id)?;
        let data: Option<String> = self
            .db
            .query_row(
                "SELECT data FROM turn_changes WHERE session_id=?1 AND request_id=?2",
                params![id, request_id],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        let mut turn: crate::task_changes::FrozenTurn =
            serde_json::from_str(&data.context("此轮尚未结束或没有保存变更快照")?)?;
        turn.refresh_undo_availability();
        Ok(turn)
    }
    pub fn last_turn_changes(&self, id: &str) -> Result<Option<crate::task_changes::FrozenTurn>> {
        self.session(id)?;
        let request_id: Option<String> = self.db.query_row(
            "SELECT request_id FROM turn_changes WHERE session_id=?1 AND data IS NOT NULL ORDER BY rowid DESC LIMIT 1",
            [id], |row| row.get(0),
        ).optional()?;
        request_id
            .map(|request_id| self.turn_changes(id, &request_id))
            .transpose()
    }
    pub fn latest_request(&self, id: &str) -> Result<String> {
        self.db
            .query_row(
                "SELECT id FROM requests WHERE session_id=?1 ORDER BY rowid DESC LIMIT 1",
                [id],
                |r| r.get(0),
            )
            .map_err(Into::into)
    }
    pub fn update_turn(&self, id: &str, turn: &crate::task_changes::FrozenTurn) -> Result<()> {
        let tx = self.db.unchecked_transaction()?;
        if tx.execute("UPDATE turn_changes SET data=?3 WHERE session_id=?1 AND request_id=?2 AND data IS NOT NULL",
            params![id,turn.changes.request_id,serde_json::to_string(turn)?])? != 1 { bail!("本轮变更记录缺失"); }
        tx.execute(
            "INSERT INTO events(session_id,at,data) VALUES (?1,?2,?3)",
            params![
                id,
                now(),
                serde_json::to_string(&EventKind::TurnChanges {
                    changes: turn.changes.clone()
                })?
            ],
        )?;
        tx.commit()?;
        Ok(())
    }
    pub fn first_request(&self, id: &str) -> Result<bool> {
        Ok(self.db.query_row(
            "SELECT COUNT(*) FROM requests WHERE session_id=?1",
            [id],
            |r| r.get::<_, u32>(0),
        )? == 1)
    }
    pub fn observe_tool(&self, session: &str, id: &str, name: &str) -> Result<()> {
        let tx = self.db.unchecked_transaction()?;
        index_tool(&tx, session, id, name)?;
        tx.commit()?;
        Ok(())
    }
    pub fn sources(&self, session: &str) -> Result<(Vec<latte_work_protocol::TaskSource>, bool)> {
        self.session(session)?;
        // Bounded lazy indexing supports older histories without inventing sources.
        let history = self.db.prepare("SELECT data FROM events WHERE session_id=?1 AND json_extract(data,'$.kind') IN ('user','queued_message_sent','tool') ORDER BY seq DESC LIMIT 257")?.query_map([session], |r| r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        let mut partial = history.len() > 256;
        let tx = self.db.unchecked_transaction()?;
        for data in history.iter().take(256).rev() {
            let event: EventKind = serde_json::from_str(data)?;
            index_event(&tx, session, &event)?;
        }
        tx.commit()?;
        let rows = self
            .db
            .prepare(
                "SELECT data FROM task_sources WHERE session_id=?1 ORDER BY at DESC,id LIMIT 129",
            )?
            .query_map([session], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        partial |= rows.len() > 128;
        Ok((
            rows.iter()
                .take(128)
                .map(|row| serde_json::from_str(row).map_err(Into::into))
                .collect::<Result<_>>()?,
            partial,
        ))
    }
    pub fn grant_reference(&self, project: &str, path: &str, directory: bool) -> Result<()> {
        let changed = self.db.execute(
            "INSERT INTO reference_access(project_id,path,directory)
            SELECT ?1,?2,?3 WHERE (SELECT COUNT(*) FROM reference_access WHERE project_id=?1)<512
            OR EXISTS(SELECT 1 FROM reference_access WHERE project_id=?1 AND path=?2)
            ON CONFLICT(project_id,path) DO UPDATE SET directory=excluded.directory",
            params![project, path, directory],
        )?;
        if changed == 0 {
            bail!("可预览的项目外文件数量已达上限");
        }
        Ok(())
    }
    pub fn reference_access(
        &self,
        project: &str,
    ) -> Result<(Vec<latte_work_protocol::FileEntry>, bool)> {
        self.project(project)?;
        let mut query = self.db.prepare("SELECT path,directory FROM reference_access WHERE project_id=?1 ORDER BY path LIMIT 513")?;
        let rows = query.query_map([project], |r| {
            let path: String = r.get(0)?;
            Ok(latte_work_protocol::FileEntry {
                name: Path::new(&path)
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.clone()),
                path,
                directory: r.get(1)?,
            })
        })?;
        let mut entries = Vec::new();
        let mut bytes = 0;
        let mut truncated = false;
        for row in rows {
            let entry = row?;
            bytes += serde_json::to_vec(&entry)?.len();
            if entries.len() == 512 || bytes > latte_work_protocol::MAX_FRAME / 2 {
                truncated = true;
                break;
            }
            entries.push(entry);
        }
        Ok((entries, truncated))
    }
    pub fn revoke_reference_access(&self, project: &str, path: &str) -> Result<bool> {
        self.project(project)?;
        // Delete the recorded canonical key even if the file was moved or removed.
        let removed = self.db.execute(
            "DELETE FROM reference_access WHERE project_id=?1 AND path=?2",
            params![project, path],
        )?;
        Ok(removed != 0)
    }
    pub fn reference_allowed(&self, project: &str, target: &Path) -> Result<bool> {
        let rows = self
            .db
            .prepare("SELECT path,directory FROM reference_access WHERE project_id=?1 ORDER BY path LIMIT 512")?
            .query_map([project], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows
            .iter()
            .any(|(path, dir)| target == Path::new(path) || (*dir && target.starts_with(path))))
    }
    pub fn state(&mut self, id: &str, status: Status, message: Option<String>) -> Result<()> {
        let mut session = self.session(id)?;
        if session.status.active()
            && matches!(status, Status::Completed | Status::Failed | Status::Stopped)
        {
            session.unread = true;
        }
        session.status = status.clone();
        let tx = self.db.transaction()?;
        tx.execute(
            "UPDATE sessions SET data=?2 WHERE id=?1",
            params![id, serde_json::to_string(&session)?],
        )?;
        tx.execute(
            "INSERT INTO events(session_id,at,data) VALUES (?1,?2,?3)",
            params![
                id,
                now(),
                serde_json::to_string(&EventKind::State { status, message })?
            ],
        )?;
        tx.commit()?;
        Ok(())
    }
    /// Checks durable request identity before current launch configuration is consulted.
    #[allow(clippy::too_many_arguments)]
    pub fn already_accepted(
        &self,
        id: &str,
        request_id: &str,
        text: &str,
        model: Option<&str>,
        effort: Option<Effort>,
        provider_fingerprint: Option<&str>,
        permission_mode: Option<&str>,
    ) -> Result<bool> {
        if request_id.is_empty()
            || request_id.len() > 100
            || text.trim().is_empty()
            || text.len() > 128 * 1024
        {
            bail!("任务内容或请求 ID 无效（输入上限 128 KiB）");
        }
        let existing = self
            .db
            .query_row(
                "SELECT session_id,text,model,effort,provider_fingerprint,permission_mode FROM requests WHERE id=?1",
                [request_id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Option<String>>(2)?,
                        r.get::<_, Option<String>>(3)?,
                        r.get::<_, Option<String>>(4)?,
                        r.get::<_, Option<String>>(5)?,
                    ))
                },
            )
            .optional()?;
        if let Some((
            session,
            previous,
            previous_model,
            previous_effort,
            previous_provider,
            previous_permission,
        )) = existing
        {
            if session != id
                || previous != text
                || previous_model.as_deref() != model
                || previous_effort.as_deref() != effort.map(Effort::as_str)
                || previous_permission.as_deref() != permission_mode
            {
                bail!("请求 ID 已被其他任务使用");
            }
            if previous_provider.as_deref() != provider_fingerprint {
                bail!("此请求已接受，但 Provider 配置已改变；请核对原请求状态后再发送新消息");
            }
            return Ok(true);
        }
        Ok(false)
    }
    /// Returns false for an exact already-accepted request; conflicting reuse fails.
    #[allow(clippy::too_many_arguments)]
    pub fn begin(
        &mut self,
        id: &str,
        request_id: &str,
        text: &str,
        model: Option<&str>,
        effort: Option<Effort>,
        provider_fingerprint: Option<&str>,
        permission_mode: Option<&str>,
    ) -> Result<bool> {
        if self.already_accepted(
            id,
            request_id,
            text,
            model,
            effort,
            provider_fingerprint,
            permission_mode,
        )? {
            return Ok(false);
        }
        let mut session = self.session(id)?;
        if session.status.active() {
            bail!("会话已有运行中的任务");
        }
        if session.archived {
            bail!("请先取消归档再发送消息");
        }
        session.status = Status::Running;
        session.model = model.map(str::to_owned);
        session.effort = effort;
        session.permission_mode = permission_mode.map(str::to_owned);
        if !session.custom_title && session.title == "新任务" {
            session.title = text.chars().take(48).collect();
        }
        let tx = self.db.transaction()?;
        tx.execute(
            "INSERT INTO requests(id,session_id,text,model,effort,provider_fingerprint,permission_mode) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![request_id, id, text, model, effort.map(Effort::as_str), provider_fingerprint, permission_mode],
        )?;
        tx.execute(
            "UPDATE sessions SET data=?2 WHERE id=?1",
            params![id, serde_json::to_string(&session)?],
        )?;
        for event in [
            EventKind::User {
                text: text.into(),
                request_id: request_id.into(),
            },
            EventKind::State {
                status: Status::Running,
                message: None,
            },
        ] {
            tx.execute(
                "INSERT INTO events(session_id,at,data) VALUES (?1,?2,?3)",
                params![id, now(), serde_json::to_string(&event)?],
            )?;
            index_event(&tx, id, &event)?;
        }
        tx.execute("UPDATE message_queue SET status='dispatched' WHERE request_id=?1 AND session_id=?2 AND status IN ('pending','forwarded')", params![request_id,id])?;
        tx.commit()?;
        Ok(true)
    }
    fn event_for_display(&self, session: &str, data: &str) -> Result<EventKind> {
        let mut event: EventKind = serde_json::from_str(data)?;
        if let EventKind::TurnChanges { changes } = &mut event
            && changes.undo_unavailable.is_none()
            && changes.undo == latte_work_protocol::TurnUndoStatus::Ready
        {
            // Project old Windows events without loading their byte snapshots into
            // the history response. Durable events themselves remain unchanged.
            changes.undo_unavailable = self.db.query_row(
                "SELECT CASE WHEN json_type(data,'$.before.root_identity.windows_v1') IS NOT NULL AND json_type(data,'$.before.windows_files_v1') IS NULL AND json_type(data,'$.after.windows_files_v1') IS NULL THEN '此历史快照缺少 Windows 文件权限元数据，不能撤销' ELSE json_extract(data,'$.changes.undo_unavailable') END FROM turn_changes WHERE session_id=?1 AND request_id=?2 AND data IS NOT NULL",
                params![session, changes.request_id], |row| row.get::<_, Option<String>>(0),
            ).optional()?.flatten();
        }
        Ok(event)
    }
    /// Reverse pages avoid replaying every historical tool output to show the latest reply.
    pub fn history(
        &self,
        id: &str,
        before: Option<f64>,
    ) -> Result<(Vec<Event>, bool, bool, Option<f64>)> {
        if before.is_some_and(|v| !v.is_finite() || v < 0.0) {
            bail!("事件游标无效");
        }
        let mut statement = self.db.prepare(
            "SELECT seq,at,data FROM events WHERE session_id=?1 AND seq<?2 ORDER BY seq DESC LIMIT 16385"
        )?;
        let mut rows = statement.query(params![id, before.unwrap_or(f64::MAX)])?;
        let mut events: Vec<Event> = Vec::new();
        let mut text_fragments: Vec<String> = Vec::new();
        let mut bytes = 0;
        let mut oldest = None;
        let mut count = 0;
        let mut turns = 0;
        let mut has_more = false;
        let mut split_text = false;
        while let Some(row) = rows.next()? {
            let data: String = row.get(2)?;
            let event = self.event_for_display(id, &data)?;
            let text = match &event {
                EventKind::Text { text } => Some(text),
                _ => None,
            };
            let joins = text.is_some()
                && events
                    .last()
                    .is_some_and(|e| matches!(e.event, EventKind::Text { .. }));
            let size = text.map_or(data.len() + id.len() + 96, |t| {
                t.len() + if joins { 0 } else { id.len() + 96 }
            });
            if turns == 2
                || count == 16384
                || (!joins && events.len() == 128)
                || (bytes + size > 256 * 1024 && !events.is_empty())
            {
                has_more = true;
                split_text = joins;
                break;
            }
            if !joins {
                flush_history_text(&mut events, &mut text_fragments);
            }
            oldest = Some(row.get::<_, i64>(0)? as f64);
            if matches!(event, EventKind::User { .. }) {
                turns += 1;
            }
            let event = match event {
                EventKind::Text { text } => {
                    text_fragments.push(text);
                    EventKind::Text {
                        text: String::new(),
                    }
                }
                other => other,
            };
            if !joins {
                events.push(Event {
                    seq: oldest.unwrap(),
                    session_id: id.into(),
                    at: row.get(1)?,
                    event,
                });
            }
            count += 1;
            bytes += size;
        }
        flush_history_text(&mut events, &mut text_fragments);
        events.reverse();
        let mut needs_earlier = has_more && split_text;
        if has_more && let Some(first) = oldest {
            // Only unresolved approvals in the current live turn force an earlier window.
            let pending: bool = self.db.query_row(
                "SELECT EXISTS(SELECT 1 FROM events a WHERE a.session_id=?1 AND a.seq<?2
                 AND json_extract(a.data,'$.kind')='approval'
                 AND a.seq>COALESCE((SELECT MAX(u.seq) FROM events u WHERE u.session_id=a.session_id
                 AND json_extract(u.data,'$.kind')='user'),0)
                 AND NOT EXISTS(SELECT 1 FROM events e WHERE e.session_id=a.session_id AND e.seq>a.seq
                 AND json_extract(e.data,'$.kind')='state' AND json_extract(e.data,'$.status') NOT IN ('running','waiting'))
                 AND NOT EXISTS(SELECT 1 FROM events r WHERE r.session_id=a.session_id AND r.seq>a.seq
                 AND json_extract(r.data,'$.kind')='approval_resolved'
                 AND json_extract(r.data,'$.request_id')=json_extract(a.data,'$.request_id')))",
                params![id, first], |r| r.get(0)
            )?;
            needs_earlier |= pending;
        }
        Ok((
            events,
            has_more,
            needs_earlier,
            if has_more { oldest } else { None },
        ))
    }
    pub fn events(&self, id: &str, after: f64) -> Result<(Vec<Event>, bool)> {
        if !after.is_finite() || after < 0.0 {
            bail!("事件游标无效");
        }
        let rows=self.db.prepare("SELECT seq,at,data FROM events WHERE session_id=?1 AND seq>?2 ORDER BY seq LIMIT 201")?.query_map(params![id,after],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,f64>(1)?,r.get::<_,String>(2)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
        let mut events = Vec::new();
        let mut bytes = 0;
        for (seq, at, data) in rows {
            bytes += data.len();
            if events.len() == 200 || (bytes > 1024 * 1024 && !events.is_empty()) {
                return Ok((events, true));
            }
            events.push(Event {
                seq: seq as f64,
                session_id: id.into(),
                at,
                event: self.event_for_display(id, &data)?,
            });
        }
        Ok((events, false))
    }
}
fn flush_history_text(events: &mut [Event], fragments: &mut Vec<String>) {
    if let Some(Event {
        event: EventKind::Text { text },
        ..
    }) = events.last_mut()
    {
        *text = fragments.drain(..).rev().collect();
    }
}
pub(crate) fn bounded_subagent_event(mut event: EventKind) -> Result<(EventKind, bool)> {
    fn clip(text: &mut String, limit: usize) -> bool {
        if text.len() <= limit {
            return false;
        }
        let mut end = limit;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push_str("\n…（内容已截断）");
        true
    }
    fn value(value: &mut serde_json::Value) -> Result<bool> {
        let mut encoded = serde_json::to_string(value)?;
        if encoded.len() <= 32 * 1024 {
            return Ok(false);
        }
        clip(&mut encoded, 16 * 1024);
        *value = serde_json::Value::String(encoded);
        Ok(true)
    }
    let truncated = match &mut event {
        EventKind::Text { text } | EventKind::Notice { text } => {
            if serde_json::to_string(text)?.len() > 32 * 1024 {
                clip(text, 16 * 1024);
                true
            } else {
                false
            }
        }
        EventKind::Tool { id, name, input } => {
            if id.len() > 256 {
                bail!("子智能体工具标识过长");
            }
            clip(name, 256) | value(input)?
        }
        EventKind::ToolResult { id, content, .. } => {
            if id.len() > 256 {
                bail!("子智能体工具标识过长");
            }
            value(content)?
        }
        EventKind::Progress { .. } => false,
        _ => bail!("不支持的子智能体过程事件"),
    };
    Ok((event, truncated))
}

fn index_event(tx: &rusqlite::Transaction<'_>, session: &str, event: &EventKind) -> Result<()> {
    if let EventKind::Tool { id, name, .. } = event {
        return index_tool(tx, session, id, name);
    }
    for source in crate::sources::event_sources(event) {
        index_source(tx, session, source)?;
    }
    Ok(())
}
fn index_tool(tx: &rusqlite::Transaction<'_>, session: &str, id: &str, name: &str) -> Result<()> {
    if id.is_empty() || id.len() > 512 {
        return Ok(());
    }
    if tx.execute(
        "INSERT OR IGNORE INTO observed_tools(session_id,id) VALUES (?1,?2)",
        params![session, id],
    )? == 0
    {
        return Ok(());
    }
    if let Some(source) = crate::sources::tool_source(name) {
        index_source(tx, session, source)?;
    }
    Ok(())
}
fn index_source(
    tx: &rusqlite::Transaction<'_>,
    session: &str,
    mut source: latte_work_protocol::TaskSource,
) -> Result<()> {
    if let Some(data) = tx
        .query_row(
            "SELECT data FROM task_sources WHERE session_id=?1 AND id=?2",
            params![session, source.id],
            |r| r.get::<_, String>(0),
        )
        .optional()?
    {
        let previous: latte_work_protocol::TaskSource = serde_json::from_str(&data)?;
        source.uses = previous.uses.saturating_add(source.uses);
        for tool in previous.tools {
            if !source.tools.contains(&tool) && source.tools.len() < 64 {
                source.tools.push(tool);
            }
        }
    } else {
        let count: u32 = tx.query_row(
            "SELECT COUNT(*) FROM task_sources WHERE session_id=?1",
            [session],
            |r| r.get(0),
        )?;
        if count >= 512 {
            return Ok(());
        }
    }
    tx.execute("INSERT INTO task_sources(session_id,id,data,at) VALUES (?1,?2,?3,?4) ON CONFLICT(session_id,id) DO UPDATE SET data=excluded.data,at=excluded.at",params![session,source.id,serde_json::to_string(&source)?,now()])?;
    Ok(())
}
#[cfg(test)]
mod tests {
    #[test]
    fn concurrent_preview_grants_cannot_exceed_the_limit() {
        use super::*;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("access.db");
        let store = Store::open(&path).unwrap();
        let project = store.add_project(dir.path()).unwrap();
        for i in 0..511 {
            store
                .grant_reference(&project.id, &format!("/outside/{i}"), false)
                .unwrap();
        }
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
        let workers: Vec<_> = (0..4)
            .map(|i| {
                let path = path.clone();
                let project = project.id.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let other = Store::open(&path).unwrap();
                    barrier.wait();
                    other
                        .grant_reference(&project, &format!("/racing/{i}"), false)
                        .is_ok()
                })
            })
            .collect();
        let granted = workers
            .into_iter()
            .map(|worker| usize::from(worker.join().unwrap()))
            .sum::<usize>();
        assert_eq!(granted, 1);
        assert_eq!(store.reference_access(&project.id).unwrap().0.len(), 512);
    }
    #[test]
    fn activity_index_migration_keeps_history_and_survives_reopening() {
        use super::*;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("legacy.db");
        let store = Store::open(&path).unwrap();
        let project = store.add_project(dir.path()).unwrap();
        let session = store.create_session(project.id, "claude".into()).unwrap();
        store
            .event(
                &session.id,
                EventKind::Text {
                    text: "preserved".into(),
                },
            )
            .unwrap();
        store.db.execute_batch("DROP INDEX events_recent_activity_v2; CREATE INDEX events_recent_activity ON events(session_id,seq);").unwrap();
        drop(store);
        for _ in 0..2 {
            let store = Store::open(&path).unwrap();
            assert_eq!(
                store
                    .db
                    .query_row(
                        "SELECT COUNT(*) FROM events WHERE session_id=?1",
                        [&session.id],
                        |row| row.get::<_, u32>(0)
                    )
                    .unwrap(),
                1
            );
            assert_eq!(store.db.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name IN ('events_recent_activity','events_recent_activity_v2')", [], |row| row.get::<_, u32>(0)).unwrap(), 1);
        }
    }
    #[test]
    fn retained_preview_access_lists_fit_the_wire_budget_even_with_long_paths() {
        use super::*;
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("access.db")).unwrap();
        let project = store.add_project(dir.path()).unwrap();
        for i in 0..512 {
            let path = format!("/{}{i}.md", "many\\folder/".repeat(300));
            store.grant_reference(&project.id, &path, false).unwrap();
        }
        let (entries, truncated) = store.reference_access(&project.id).unwrap();
        assert!(truncated && !entries.is_empty() && entries.len() < 512);
        let response = latte_work_protocol::Response::ReferenceAccess { entries, truncated };
        assert!(serde_json::to_vec(&response).unwrap().len() < latte_work_protocol::MAX_FRAME);
    }
    #[test]
    fn retained_preview_access_is_bounded_durable_and_revocable_without_the_file() {
        use super::*;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("access.db");
        let store = Store::open(&path).unwrap();
        let project = store.add_project(dir.path()).unwrap();
        for i in 0..512 {
            store
                .grant_reference(&project.id, &format!("/outside/{i:03}"), true)
                .unwrap();
        }
        assert!(
            store
                .grant_reference(&project.id, "/overflow", false)
                .is_err()
        );
        store
            .grant_reference(&project.id, "/outside/000", false)
            .unwrap();
        assert!(
            !store
                .reference_allowed(&project.id, Path::new("/outside/000/child.md"))
                .unwrap()
        );
        assert!(
            store
                .reference_allowed(&project.id, Path::new("/outside/001/child.md"))
                .unwrap()
        );
        assert!(
            !store
                .reference_allowed(&project.id, Path::new("/outside/001-other/child.md"))
                .unwrap()
        );
        drop(store);
        let store = Store::open(&path).unwrap();
        let (entries, truncated) = store.reference_access(&project.id).unwrap();
        assert_eq!(entries.len(), 512);
        assert!(!truncated);
        assert_eq!(entries[0].path, "/outside/000");
        // Legacy versions could retain more than the new admission limit.
        store.db.execute("INSERT INTO reference_access(project_id,path,directory) VALUES (?1,'/legacy-a',0),(?1,'/legacy-b',0)", [&project.id]).unwrap();
        let (entries, truncated) = store.reference_access(&project.id).unwrap();
        assert_eq!(entries.len(), 512);
        assert!(truncated);
        store
            .revoke_reference_access(&project.id, "/legacy-a")
            .unwrap();
        store
            .revoke_reference_access(&project.id, "/legacy-b")
            .unwrap();
        store
            .revoke_reference_access(&project.id, "/outside/001")
            .unwrap();
        store
            .revoke_reference_access(&project.id, "/outside/001")
            .unwrap();
        assert!(
            !store
                .reference_allowed(&project.id, Path::new("/outside/001/child.md"))
                .unwrap()
        );
        store
            .grant_reference(&project.id, "/overflow", false)
            .unwrap();
        assert!(store.reference_access("missing-project").is_err());
        assert!(
            store
                .revoke_reference_access("missing-project", "/overflow")
                .is_err()
        );
        drop(store);
        let store = Store::open(&path).unwrap();
        assert!(
            !store
                .reference_allowed(&project.id, Path::new("/outside/001"))
                .unwrap()
        );
    }
    #[test]
    fn native_names_follow_external_renames_while_generated_titles_preserve_manual_names() {
        use super::*;
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("titles.db")).unwrap();
        let project = store.add_project(dir.path()).unwrap();
        let session = store.create_session(project.id, "claude".into()).unwrap();
        assert_eq!(
            store
                .sync_native_title(&session.id, "Generated", false)
                .unwrap()
                .title,
            "Generated"
        );
        store.rename_session(&session.id, "Manual").unwrap();
        let current = store
            .sync_native_title(&session.id, "Later generated", false)
            .unwrap();
        assert_eq!(current.title, "Manual");
        assert!(current.custom_title);
        assert_eq!(
            store
                .sync_live_native_title(&session.id, "New manual")
                .unwrap()
                .title,
            "New manual"
        );
    }
    #[test]
    fn subagent_transcripts_coalesce_streams_keep_aliases_and_survive_restart() {
        use super::*;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("children.db");
        let store = Store::open(&path).unwrap();
        let project = store.add_project(dir.path()).unwrap();
        let session = store.create_session(project.id, "claude".into()).unwrap();
        for (native, tool) in [
            ("tool:first", "first"),
            ("native-first", "first"),
            ("tool:second", "second"),
        ] {
            store
                .event(
                    &session.id,
                    EventKind::Subagent {
                        update: SubagentUpdate {
                            agent_type: None,
                            id: native.into(),
                            tool_use_id: Some(tool.into()),
                            title: None,
                            status: Some(SubagentStatus::Running),
                            summary: None,
                            last_tool: None,
                        },
                    },
                )
                .unwrap();
        }
        let parent_count = store.events(&session.id, 0.0).unwrap().0.len();
        for (tool, text) in [("first", "一"), ("second", "other"), ("first", "二")] {
            store
                .subagent_event(&session.id, tool, EventKind::Text { text: text.into() })
                .unwrap();
        }
        let (first, truncated) = store
            .subagent_transcript(&session.id, "tool:first")
            .unwrap();
        assert!(!truncated);
        assert_eq!(first.len(), 1);
        assert!(matches!(&first[0].event, EventKind::Text { text } if text == "一二"));
        let second = store
            .subagent_transcript(&session.id, "tool:second")
            .unwrap()
            .0;
        assert!(matches!(&second[0].event, EventKind::Text { text } if text == "other"));
        assert_eq!(
            store.events(&session.id, 0.0).unwrap().0.len(),
            parent_count
        );
        let other = store
            .create_session(session.project_id.clone(), "claude".into())
            .unwrap();
        assert!(store.subagent_transcript(&other.id, "tool:first").is_err());
        assert!(
            store
                .subagent_event(
                    &other.id,
                    "first",
                    EventKind::Text {
                        text: "wrong session".into()
                    }
                )
                .is_err()
        );
        drop(store);
        let restored = Store::open(&path).unwrap();
        assert_eq!(
            restored
                .subagent_transcript(&session.id, "tool:first")
                .unwrap()
                .0
                .len(),
            1
        );
        assert_eq!(
            restored.subagents(&session.id).unwrap().0[0].status,
            SubagentStatus::Unknown
        );
    }

    #[test]
    fn full_child_stream_history_pages_without_losing_records_or_long_fields() {
        use super::*;
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("history.db")).unwrap();
        let project = store.add_project(dir.path()).unwrap();
        let session = store.create_session(project.id, "claude".into()).unwrap();
        store
            .event(
                &session.id,
                EventKind::Subagent {
                    update: SubagentUpdate {
                        agent_type: None,
                        id: "child".into(),
                        tool_use_id: Some("p".into()),
                        title: None,
                        status: Some(SubagentStatus::Running),
                        summary: None,
                        last_tool: None,
                    },
                },
            )
            .unwrap();
        for i in 0..600 {
            store
                .subagent_event(
                    &session.id,
                    "p",
                    EventKind::Tool {
                        id: format!("tool-{i}"),
                        name: "Read".into(),
                        input: serde_json::json!({"text":"参数".repeat(100)}),
                    },
                )
                .unwrap();
        }
        let report = "中文完整报告".repeat(20_000);
        store
            .subagent_event(
                &session.id,
                "p",
                EventKind::Tool {
                    id: "report".into(),
                    name: "SubagentHandback".into(),
                    input: serde_json::json!({"message":report}),
                },
            )
            .unwrap();
        store
            .subagent_event(
                &session.id,
                "p",
                EventKind::ToolResult {
                    id: "report".into(),
                    content: serde_json::json!("{\"success\":true}"),
                    is_error: false,
                },
            )
            .unwrap();
        let mut after = None;
        let mut found = Vec::new();
        loop {
            let response = store
                .subagent_history(&session.id, "child", after, Some("stream"), false)
                .unwrap();
            assert!(serde_json::to_vec(&response).unwrap().len() < latte_work_protocol::MAX_FRAME);
            let latte_work_protocol::Response::SubagentHistory {
                events,
                has_more,
                after: next,
                incomplete,
                ..
            } = response
            else {
                panic!("history");
            };
            assert!(!incomplete);
            assert!(next > after);
            found.extend(events);
            after = next;
            if !has_more {
                break;
            }
        }
        assert_eq!(found.len(), 603);
        assert!(matches!(&found[600].event,EventKind::Tool{input,..} if input["message"]==report));
        assert!(matches!(&found[602].event,EventKind::Text{text} if text==&report));
        assert!(
            store
                .subagent_history(&session.id, "other", None, None, false)
                .is_err()
        );
        assert!(
            store
                .subagent_history(&session.id, "child", Some(f64::NAN), None, false)
                .is_err()
        );
        drop(store);
        let reopened = Store::open(&dir.path().join("history.db")).unwrap();
        assert!(
            matches!(reopened.subagent_history(&session.id,"child",after,Some("stream"),false).unwrap(),latte_work_protocol::Response::SubagentHistory{events,has_more:false,..} if events.is_empty())
        );
    }

    #[test]
    fn subagent_transcripts_bound_content_and_records_without_inventing_results() {
        use super::*;
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("children.db")).unwrap();
        let project = store.add_project(dir.path()).unwrap();
        let session = store.create_session(project.id, "claude".into()).unwrap();
        store
            .event(
                &session.id,
                EventKind::Subagent {
                    update: SubagentUpdate {
                        agent_type: None,
                        id: "child".into(),
                        tool_use_id: Some("p".into()),
                        title: None,
                        status: Some(SubagentStatus::Running),
                        summary: None,
                        last_tool: None,
                    },
                },
            )
            .unwrap();
        store
            .subagent_event(
                &session.id,
                "p",
                EventKind::Text {
                    text: "中".repeat(100_000),
                },
            )
            .unwrap();
        let (events, partial) = store.subagent_transcript(&session.id, "child").unwrap();
        assert!(partial);
        assert!(serde_json::to_vec(&events).unwrap().len() < 128 * 1024);
        for i in 0..501 {
            store
                .subagent_event(
                    &session.id,
                    "p",
                    EventKind::Tool {
                        id: format!("t{i}"),
                        name: "Read".into(),
                        input: serde_json::json!({}),
                    },
                )
                .unwrap();
        }
        let (events, partial) = store.subagent_transcript(&session.id, "child").unwrap();
        assert!(partial);
        assert_eq!(events.len(), 500);
        assert!(
            events
                .iter()
                .all(|e| matches!(e.event, EventKind::Tool { .. }))
        );
        assert!(
            store
                .subagent_event(
                    &session.id,
                    "p",
                    EventKind::State {
                        status: Status::Completed,
                        message: None
                    }
                )
                .is_err()
        );
    }

    #[test]
    fn recent_history_pages_all_visible_projects_and_ignores_read_metadata() {
        use super::*;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recent.db");
        let store = Store::open(&path).unwrap();
        let project = store.add_project(dir.path()).unwrap();
        let other_dir = dir.path().join("other");
        std::fs::create_dir(&other_dir).unwrap();
        let other = store.add_project(&other_dir).unwrap();
        let mut ids = Vec::new();
        for i in 0..105 {
            let mut session = store
                .create_session(
                    if i % 2 == 0 {
                        project.id.clone()
                    } else {
                        other.id.clone()
                    },
                    "claude".into(),
                )
                .unwrap();
            session.created_at = 10.0;
            store.save(&session).unwrap();
            ids.push(session.id);
        }
        // Existing history is projected without rewriting session data.
        store
            .event(
                &ids[0],
                EventKind::Text {
                    text: "older conversation, new reply".into(),
                },
            )
            .unwrap();
        store
            .db
            .execute("UPDATE events SET at=20 WHERE session_id=?1", [&ids[0]])
            .unwrap();
        store.mark_session_unread(&ids[1], false).unwrap();
        store.rename_session(&ids[1], "metadata only").unwrap();
        store.pin_session(&ids[1], true).unwrap();
        store
            .event(
                &ids[1],
                EventKind::State {
                    status: Status::Ready,
                    message: None,
                },
            )
            .unwrap();
        let (first, next) = store.recent_sessions(None).unwrap();
        assert_eq!(first.len(), 100);
        assert_eq!(first[0].session.id, ids[0]);
        assert_eq!(first[0].updated_at, 20.0);
        assert!(first.iter().any(|row| row.session.project_id == other.id));
        let (second, end) = store.recent_sessions(next.as_ref()).unwrap();
        assert_eq!(second.len(), 5);
        assert!(end.is_none());
        let loaded: std::collections::HashSet<_> = first
            .iter()
            .chain(second.iter())
            .map(|row| row.session.id.clone())
            .collect();
        assert_eq!(loaded.len(), 105);
        store.archive_session(&ids[0], true).unwrap();
        store.remove_project(&other.id).unwrap();
        let (visible, _) = store.recent_sessions(None).unwrap();
        assert!(
            visible
                .iter()
                .all(|row| row.session.project_id == project.id && !row.session.archived)
        );
        assert!(!visible.iter().any(|row| row.session.id == ids[0]));
        assert!(
            store
                .recent_sessions(Some(&RecentCursor {
                    updated_at: -1.0,
                    id: "x".into()
                }))
                .is_err()
        );
        drop(store);
        let store = Store::open(&path).unwrap();
        assert_eq!(store.recent_sessions(None).unwrap().0.len(), visible.len());
    }
    #[test]
    fn subagents_merge_native_identity_and_survive_history_windows_and_restart() {
        use super::*;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("children.db");
        let mut store = Store::open(&path).unwrap();
        let project = store.add_project(dir.path()).unwrap();
        let session = store
            .create_session(project.id.clone(), "claude".into())
            .unwrap();
        let other = store
            .create_session(project.id.clone(), "claude".into())
            .unwrap();
        store.event(&session.id, EventKind::Tool { id: "tool".into(), name: "Agent".into(), input: serde_json::json!({"prompt":"Inspect the exact changes. Original input."}) }).unwrap();
        store
            .event(
                &other.id,
                EventKind::Tool {
                    id: "tool".into(),
                    name: "Agent".into(),
                    input: serde_json::json!({"prompt":"Wrong session"}),
                },
            )
            .unwrap();
        let update = |id: &str, status| EventKind::Subagent {
            update: SubagentUpdate {
                agent_type: (id == "tool:tool").then(|| "review:code-reviewer".into()),
                id: id.into(),
                tool_use_id: Some("tool".into()),
                title: Some("检查代码".into()),
                status: Some(status),
                summary: None,
                last_tool: None,
            },
        };
        store
            .event(&session.id, update("tool:tool", SubagentStatus::Running))
            .unwrap();
        store
            .event(&session.id, update("native", SubagentStatus::Running))
            .unwrap();
        for _ in 0..140 {
            store
                .event(
                    &session.id,
                    EventKind::Notice {
                        text: "later".into(),
                    },
                )
                .unwrap();
        }
        let (tasks, _) = store.subagents(&session.id).unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].id, "tool:tool");
        assert_eq!(tasks[0].native_id, "native");
        assert_eq!(tasks[0].agent_type.as_deref(), Some("review:code-reviewer"));
        assert!(store.subagents(&other.id).unwrap().0.is_empty());
        store.state(&session.id, Status::Completed, None).unwrap();
        drop(store);
        let store = Store::open(&path).unwrap();
        assert_eq!(
            store.subagents(&session.id).unwrap().0[0]
                .agent_type
                .as_deref(),
            Some("review:code-reviewer")
        );
        assert_eq!(
            store.subagents(&session.id).unwrap().0[0].status,
            SubagentStatus::Unknown
        );
        assert!(
            matches!(store.subagent_history(&session.id, "tool:tool", None, None, false).unwrap(), latte_work_protocol::Response::SubagentHistory { prompt: Some(prompt), .. } if prompt == "Inspect the exact changes. Original input.")
        );
        store
            .end_subagents(&session.id, SubagentStatus::Stopped)
            .unwrap();
        store
            .event(&session.id, update("native", SubagentStatus::Completed))
            .unwrap();
        store
            .event(&session.id, update("native", SubagentStatus::Running))
            .unwrap();
        assert_eq!(
            store.subagents(&session.id).unwrap().0[0].status,
            SubagentStatus::Completed
        );
        assert_eq!(
            store.subagents(&session.id).unwrap().0[0]
                .agent_type
                .as_deref(),
            Some("review:code-reviewer")
        );
        for n in 0..130 {
            store
                .event(
                    &session.id,
                    EventKind::Subagent {
                        update: SubagentUpdate {
                            agent_type: None,
                            id: format!("child-{n}"),
                            tool_use_id: None,
                            title: None,
                            status: Some(SubagentStatus::Completed),
                            summary: None,
                            last_tool: None,
                        },
                    },
                )
                .unwrap();
        }
        let (tasks, truncated) = store.subagents(&session.id).unwrap();
        assert_eq!(tasks.len(), 128);
        assert!(truncated);
    }

    #[test]
    fn history_compacts_stream_chunks_and_pages_with_raw_cursors_without_gaps() {
        let dir = tempfile::tempdir().unwrap();
        let store = super::Store::open(&dir.path().join("history.db")).unwrap();
        let project = store.add_project(dir.path()).unwrap();
        let session = store.create_session(project.id, "claude".into()).unwrap();
        // Large older tool output must not delay the latest compact reply.
        for i in 0..300 {
            store
                .event(
                    &session.id,
                    super::EventKind::Notice {
                        text: format!("old-{i}"),
                    },
                )
                .unwrap();
        }
        store
            .event(
                &session.id,
                super::EventKind::User {
                    text: "latest".into(),
                    request_id: "latest".into(),
                },
            )
            .unwrap();
        for _ in 0..3000 {
            store
                .event(
                    &session.id,
                    super::EventKind::Text {
                        text: "你好".into(),
                    },
                )
                .unwrap();
        }
        store
            .event(
                &session.id,
                super::EventKind::State {
                    status: super::Status::Completed,
                    message: None,
                },
            )
            .unwrap();
        let raw = store
            .db
            .query_row(
                "SELECT COUNT(*) FROM events WHERE session_id=?1",
                [&session.id],
                |r| r.get::<_, i64>(0),
            )
            .unwrap();
        let (latest, more, needs, mut cursor) = store.history(&session.id, None).unwrap();
        assert!(more);
        assert!(!needs);
        assert!(latest.len() <= 128);
        assert_eq!(
            latest
                .iter()
                .filter_map(|e| match &e.event {
                    super::EventKind::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<String>(),
            "你好".repeat(3000)
        );
        let newest = latest.last().unwrap().seq;
        assert!(store.events(&session.id, newest).unwrap().0.is_empty());
        let mut merged = latest;
        while more && cursor.is_some() {
            let (page, has_more, _, next) = store.history(&session.id, cursor).unwrap();
            assert!(page.last().unwrap().seq < cursor.unwrap());
            assert!(next.is_none_or(|n| n < cursor.unwrap()));
            merged.splice(0..0, page);
            cursor = if has_more { next } else { None };
        }
        assert_eq!(
            merged
                .iter()
                .filter(|e| matches!(e.event, super::EventKind::Notice { .. }))
                .count(),
            300
        );
        assert_eq!(
            store
                .db
                .query_row(
                    "SELECT COUNT(*) FROM events WHERE session_id=?1",
                    [&session.id],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            raw
        );
        assert!(store.history(&session.id, Some(-1.0)).is_err());
    }
    #[test]
    fn bounded_history_preserves_split_text_and_pending_approvals_until_resolved_or_expired() {
        let dir = tempfile::tempdir().unwrap();
        let store = super::Store::open(&dir.path().join("history.db")).unwrap();
        let p = store.add_project(dir.path()).unwrap();
        let s = store.create_session(p.id, "claude".into()).unwrap();
        store
            .event(
                &s.id,
                super::EventKind::Approval {
                    request_id: "a".into(),
                    tool_use_id: None,
                    tool: "Write".into(),
                    input: serde_json::json!({}),
                },
            )
            .unwrap();
        for _ in 0..300 {
            store
                .event(
                    &s.id,
                    super::EventKind::Notice {
                        text: "background".into(),
                    },
                )
                .unwrap();
        }
        assert!(store.history(&s.id, None).unwrap().2);
        store
            .event(
                &s.id,
                super::EventKind::ApprovalResolved {
                    request_id: "a".into(),
                    allow: true,
                },
            )
            .unwrap();
        assert!(!store.history(&s.id, None).unwrap().2);
        store
            .event(
                &s.id,
                super::EventKind::Approval {
                    request_id: "b".into(),
                    tool_use_id: None,
                    tool: "Read".into(),
                    input: serde_json::json!({}),
                },
            )
            .unwrap();
        for _ in 0..300 {
            store
                .event(
                    &s.id,
                    super::EventKind::Notice {
                        text: "background".into(),
                    },
                )
                .unwrap();
        }
        store
            .event(
                &s.id,
                super::EventKind::State {
                    status: super::Status::Failed,
                    message: None,
                },
            )
            .unwrap();
        assert!(!store.history(&s.id, None).unwrap().2);
        for _ in 0..400 {
            store
                .event(
                    &s.id,
                    super::EventKind::Text {
                        text: "x".repeat(1024),
                    },
                )
                .unwrap();
        }
        let (tail, more, split, cursor) = store.history(&s.id, None).unwrap();
        assert!(more && split);
        assert!(serde_json::to_vec(&tail).unwrap().len() < 270 * 1024);
        let (head, _, split, _) = store.history(&s.id, cursor).unwrap();
        assert!(!split);
        let text = head
            .iter()
            .chain(&tail)
            .filter_map(|e| match &e.event {
                super::EventKind::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<String>();
        assert_eq!(text.len(), 400 * 1024);
    }
    #[test]
    fn permission_survives_restart_and_conflicting_request_reuse_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("permissions.db");
        let mut store = super::Store::open(&path).unwrap();
        let p = store.add_project(dir.path()).unwrap();
        let session = store.create_session(p.id, "claude".into()).unwrap();
        store
            .begin(
                &session.id,
                "permission",
                "hello",
                None,
                None,
                None,
                Some("plan"),
            )
            .unwrap();
        drop(store);
        let mut store = super::Store::open(&path).unwrap();
        assert_eq!(
            store
                .session(&session.id)
                .unwrap()
                .permission_mode
                .as_deref(),
            Some("plan")
        );
        assert!(
            !store
                .begin(
                    &session.id,
                    "permission",
                    "hello",
                    None,
                    None,
                    None,
                    Some("plan")
                )
                .unwrap()
        );
        assert!(
            store
                .begin(
                    &session.id,
                    "permission",
                    "hello",
                    None,
                    None,
                    None,
                    Some("bypassPermissions")
                )
                .is_err()
        );
        assert!(
            store
                .begin(&session.id, "permission", "hello", None, None, None, None)
                .is_err()
        );
        store
            .begin(&session.id, "native", "hello", None, None, None, None)
            .unwrap();
        assert!(
            store
                .session(&session.id)
                .unwrap()
                .permission_mode
                .is_none()
        );
    }

    use super::*;
    #[test]
    fn turn_fingerprint_is_durable_and_conflicting_retries_fail() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("state.db");
        let mut store = Store::open(&path).unwrap();
        let project = store.add_project(tmp.path()).unwrap();
        let session = store.create_session(project.id, "claude".into()).unwrap();
        assert!(
            store
                .begin(
                    &session.id,
                    "snapshot",
                    "hello",
                    None,
                    None,
                    Some("digest-a"),
                    None
                )
                .unwrap()
        );
        drop(store);
        let mut reopened = Store::open(&path).unwrap();
        assert_eq!(
            reopened.session(&session.id).unwrap().status,
            Status::Unknown
        );
        assert!(
            !reopened
                .begin(
                    &session.id,
                    "snapshot",
                    "hello",
                    None,
                    None,
                    Some("digest-a"),
                    None
                )
                .unwrap()
        );
        assert!(
            reopened
                .begin(
                    &session.id,
                    "snapshot",
                    "hello",
                    None,
                    None,
                    Some("digest-b"),
                    None
                )
                .is_err()
        );
        assert!(
            reopened
                .begin(&session.id, "snapshot", "hello", None, None, None, None)
                .is_err()
        );
    }
    #[test]
    fn session_organization_preserves_history_and_survives_restart() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("state.db");
        let mut store = Store::open(&path).unwrap();
        let p = store.add_project(tmp.path()).unwrap();
        let s = store.create_session(p.id.clone(), "claude".into()).unwrap();
        assert!(store.rename_session(&s.id, "  ").is_err());
        assert!(store.rename_session(&s.id, "bad\nname").is_err());
        store.rename_session(&s.id, "新任务").unwrap();
        store.mark_session_unread(&s.id, true).unwrap();
        let pinned = store.pin_session(&s.id, true).unwrap().pinned_at;
        assert_eq!(store.pin_session(&s.id, true).unwrap().pinned_at, pinned);
        store
            .begin(
                &s.id,
                "request",
                "must not overwrite title",
                None,
                None,
                None,
                None,
            )
            .unwrap();
        assert_eq!(store.session(&s.id).unwrap().title, "新任务");
        assert!(store.archive_session(&s.id, true).is_err());
        store.state(&s.id, Status::Completed, None).unwrap();
        let before = store.events(&s.id, 0.0).unwrap().0.len();
        store.archive_session(&s.id, true).unwrap();
        assert!(store.pinned_sessions().unwrap().is_empty());
        assert!(store.pin_session(&s.id, true).is_err());
        assert!(
            store
                .begin(&s.id, "blocked", "hello", None, None, None, None)
                .is_err()
        );
        drop(store);
        let store = Store::open(&path).unwrap();
        let restored = store.session(&s.id).unwrap();
        assert!(restored.archived && restored.unread && restored.custom_title);
        assert_eq!(store.events(&s.id, 0.0).unwrap().0.len(), before);
        assert_eq!(store.sessions(&p.id).unwrap().len(), 1);
        store.archive_session(&s.id, false).unwrap();
        assert!(store.session(&s.id).unwrap().pinned_at.is_none());
        store.pin_session(&s.id, true).unwrap();
        store.remove_project(&p.id).unwrap();
        assert!(store.pinned_sessions().unwrap().is_empty());
    }
    #[test]
    fn turn_completion_persists_unread_until_acknowledged() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("state.db");
        let mut store = Store::open(&path).unwrap();
        let project = store.add_project(tmp.path()).unwrap();
        let session = store.create_session(project.id, "claude".into()).unwrap();
        for (index, final_status) in [Status::Completed, Status::Failed, Status::Stopped]
            .into_iter()
            .enumerate()
        {
            store
                .begin(
                    &session.id,
                    &format!("request-{index}"),
                    "hello",
                    None,
                    None,
                    None,
                    None,
                )
                .unwrap();
            assert!(!store.session(&session.id).unwrap().unread);
            store.state(&session.id, Status::Waiting, None).unwrap();
            assert!(!store.session(&session.id).unwrap().unread);
            store
                .state(&session.id, final_status.clone(), None)
                .unwrap();
            assert!(store.session(&session.id).unwrap().unread);
            drop(store);
            store = Store::open(&path).unwrap();
            assert!(store.session(&session.id).unwrap().unread);
            store.mark_session_unread(&session.id, false).unwrap();
            store.state(&session.id, final_status, None).unwrap();
            assert!(!store.session(&session.id).unwrap().unread);
        }
    }
    #[test]
    fn legacy_sessions_default_to_unarchived_and_unpinned() {
        let session: Session = serde_json::from_value(serde_json::json!({
            "id":"legacy", "project_id":"p", "title":"old", "agent":"claude",
            "native_id":null, "status":"ready", "created_at":1.0
        }))
        .unwrap();
        assert!(!session.archived && !session.unread && !session.custom_title);
        assert!(session.pinned_at.is_none());
        assert!(session.effort.is_none());
    }
    #[test]
    fn legacy_request_migration_preserves_duplicate_protection() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("legacy.db");
        let db = Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE requests(id TEXT PRIMARY KEY,session_id TEXT NOT NULL,text TEXT NOT NULL); INSERT INTO requests VALUES('old','session','hello');").unwrap();
        drop(db);
        let mut store = Store::open(&path).unwrap();
        assert!(
            !store
                .begin("session", "old", "hello", None, None, None, None)
                .unwrap()
        );
        assert!(
            store
                .begin("session", "old", "hello", Some("sonnet"), None, None, None)
                .is_err()
        );
        drop(store);
        let mut reopened = Store::open(&path).unwrap();
        assert!(
            reopened
                .begin(
                    "session",
                    "old",
                    "hello",
                    None,
                    Some(Effort::High),
                    None,
                    None
                )
                .is_err()
        );
        assert!(
            !reopened
                .begin("session", "old", "hello", None, None, None, None)
                .unwrap()
        );
    }
    #[test]
    fn deduplication_and_restart_never_infer_success() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("state.db");
        let mut store = Store::open(&path).unwrap();
        let p = store.add_project(tmp.path()).unwrap();
        assert_eq!(p.id, store.add_project(tmp.path()).unwrap().id);
        let s = store.create_session(p.id, "claude".into()).unwrap();
        assert!(
            store
                .begin(&s.id, "one", "hello", None, None, None, None)
                .unwrap()
        );
        assert!(
            !store
                .begin(&s.id, "one", "hello", None, None, None, None)
                .unwrap()
        );
        assert!(
            store
                .begin(&s.id, "one", "different", None, None, None, None)
                .is_err()
        );
        assert!(
            store
                .begin(&s.id, "two", "hello", None, None, None, None)
                .is_err()
        );
        drop(store);
        let store = Store::open(&path).unwrap();
        assert_eq!(store.session(&s.id).unwrap().status, Status::Unknown);
        let (events, _) = store.events(&s.id, 0.0).unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(store.events(&s.id, events[0].seq).unwrap().0.len(), 2);
    }

    #[test]
    fn effort_persists_and_request_identity_includes_effort() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("effort.db");
        let mut store = Store::open(&path).unwrap();
        let p = store.add_project(tmp.path()).unwrap();
        let s = store.create_session(p.id, "claude".into()).unwrap();
        assert!(
            store
                .begin(
                    &s.id,
                    "effort",
                    "hello",
                    None,
                    Some(Effort::High),
                    None,
                    None
                )
                .unwrap()
        );
        assert!(
            !store
                .begin(
                    &s.id,
                    "effort",
                    "hello",
                    None,
                    Some(Effort::High),
                    None,
                    None
                )
                .unwrap()
        );
        assert!(
            store
                .begin(
                    &s.id,
                    "effort",
                    "hello",
                    None,
                    Some(Effort::Low),
                    None,
                    None
                )
                .is_err()
        );
        store.state(&s.id, Status::Completed, None).unwrap();
        drop(store);
        let mut store = Store::open(&path).unwrap();
        assert_eq!(store.session(&s.id).unwrap().effort, Some(Effort::High));
        assert!(
            store
                .begin(&s.id, "automatic", "hello", None, None, None, None)
                .unwrap()
        );
        assert_eq!(store.session(&s.id).unwrap().effort, None);
    }
}

#[cfg(test)]
mod source_tests {
    use super::*;
    #[tokio::test]
    async fn last_saved_turn_is_session_scoped_and_ignores_old_undo_updates_and_pending_turns() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store.db");
        let mut store = Store::open(&path).unwrap();
        let project = store.add_project(dir.path()).unwrap();
        let session = store
            .create_session(project.id.clone(), "claude".into())
            .unwrap();
        let other = store.create_session(project.id, "claude".into()).unwrap();
        assert!(store.last_turn_changes(&session.id).unwrap().is_none());
        assert!(store.last_turn_changes("missing").is_err());
        for id in ["first", "second"] {
            store
                .begin(&session.id, id, "edit", None, None, None, None)
                .unwrap();
            let before = crate::task_changes::Snapshot::unavailable("fixture".into());
            store.save_turn_baseline(&session.id, id, &before).unwrap();
            let turn = crate::task_changes::FrozenTurn::freeze(
                id.into(),
                before,
                crate::task_changes::Snapshot::unavailable("fixture".into()),
                false,
                false,
            )
            .await
            .unwrap();
            store.finish_turn(&session.id, &turn).unwrap();
            store.state(&session.id, Status::Completed, None).unwrap();
        }
        let mut older = store.turn_changes(&session.id, "first").unwrap();
        older.changes.undo = latte_work_protocol::TurnUndoStatus::Reverted;
        store.update_turn(&session.id, &older).unwrap();
        store
            .begin(&session.id, "pending", "edit", None, None, None, None)
            .unwrap();
        store
            .save_turn_baseline(
                &session.id,
                "pending",
                &crate::task_changes::Snapshot::unavailable("fixture".into()),
            )
            .unwrap();
        assert_eq!(
            store
                .last_turn_changes(&session.id)
                .unwrap()
                .unwrap()
                .changes
                .request_id,
            "second"
        );
        assert!(store.last_turn_changes(&other.id).unwrap().is_none());
        drop(store);
        let store = Store::open(&path).unwrap();
        assert_eq!(
            store
                .last_turn_changes(&session.id)
                .unwrap()
                .unwrap()
                .changes
                .request_id,
            "second"
        );
    }
    #[tokio::test]
    async fn turn_snapshots_and_uncertain_undo_persist_but_inflight_capture_recovers_as_unknown() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("project");
        std::fs::create_dir(&root).unwrap();
        assert!(
            std::process::Command::new("git")
                .current_dir(&root)
                .args(["init", "-q"])
                .status()
                .unwrap()
                .success()
        );
        std::fs::write(root.join("a.txt"), "before\n").unwrap();
        let path = dir.path().join("store.db");
        let mut store = Store::open(&path).unwrap();
        let project = store.add_project(&root).unwrap();
        let session = store.create_session(project.id, "claude".into()).unwrap();
        store
            .begin(&session.id, "r1", "edit", None, None, None, None)
            .unwrap();
        let before = crate::task_changes::Snapshot::capture(&root).await.unwrap();
        store
            .save_turn_baseline(&session.id, "r1", &before)
            .unwrap();
        store
            .save_turn_baseline(
                &session.id,
                "r1",
                &crate::task_changes::Snapshot::unavailable("duplicate".into()),
            )
            .unwrap();
        std::fs::write(root.join("a.txt"), "after\n").unwrap();
        let (_, saved) = store.pending_turn(&session.id).unwrap().unwrap();
        let after = crate::task_changes::Snapshot::capture(&root).await.unwrap();
        let mut turn =
            crate::task_changes::FrozenTurn::freeze("r1".into(), saved, after, false, false)
                .await
                .unwrap();
        store.finish_turn(&session.id, &turn).unwrap();
        store.finish_turn(&session.id, &turn).unwrap();
        store.state(&session.id, Status::Completed, None).unwrap();
        turn.changes.undo = latte_work_protocol::TurnUndoStatus::Unknown;
        store.update_turn(&session.id, &turn).unwrap();
        drop(store);
        let mut store = Store::open(&path).unwrap();
        let saved = store.turn_changes(&session.id, "r1").unwrap();
        assert_eq!(
            saved.changes.undo,
            latte_work_protocol::TurnUndoStatus::Unknown
        );
        assert!(saved.patch("a.txt").await.unwrap().0.contains("-before"));
        assert!(store.pending_turn(&session.id).unwrap().is_none());
        store
            .begin(&session.id, "r2", "next", None, None, None, None)
            .unwrap();
        store
            .save_turn_baseline(&session.id, "r2", &before)
            .unwrap();
        drop(store);
        std::fs::write(root.join("a.txt"), "changed while offline\n").unwrap();
        let store = Store::open(&path).unwrap();
        assert_eq!(store.session(&session.id).unwrap().status, Status::Unknown);
        assert!(store.pending_turn(&session.id).unwrap().is_none());
        let events = store.events(&session.id, 0.).unwrap().0;
        let changes: Vec<_> = events
            .iter()
            .filter_map(|e| {
                if let EventKind::TurnChanges { changes } = &e.event {
                    Some(changes)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(
            changes.iter().filter(|c| c.request_id == "r1").count(),
            2,
            "one result plus one unknown undo intent"
        );
        let unknown = changes.iter().find(|c| c.request_id == "r2").unwrap();
        assert!(
            unknown.interrupted
                && unknown.summary.unavailable.is_some()
                && unknown.summary.entries.is_empty()
        );
        assert!(store.turn_changes(&session.id, "r2").is_err());
    }
    #[test]
    fn baselines_and_observed_sources_persist_without_recounting_tools() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store.db");
        let store = Store::open(&path).unwrap();
        let project = store.add_project(dir.path()).unwrap();
        let session = store
            .create_session(project.id.clone(), "claude".into())
            .unwrap();
        let baseline = crate::task_changes::Snapshot::unavailable("legacy".into());
        store.save_baseline(&session.id, &baseline).unwrap();
        store
            .save_baseline(
                &session.id,
                &crate::task_changes::Snapshot::unavailable("replacement".into()),
            )
            .unwrap();
        store
            .observe_tool(&session.id, "one", "mcp__docs__read")
            .unwrap();
        store
            .observe_tool(&session.id, "one", "mcp__docs__read")
            .unwrap();
        store
            .observe_tool(&session.id, "child:two", "mcp__docs__search")
            .unwrap();
        drop(store);
        let store = Store::open(&path).unwrap();
        assert_eq!(
            store
                .baseline(&session.id)
                .unwrap()
                .unwrap()
                .unavailable
                .as_deref(),
            Some("legacy")
        );
        let (sources, truncated) = store.sources(&session.id).unwrap();
        assert!(!truncated);
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].uses, 2);
        assert_eq!(sources[0].tools.len(), 2);
    }
}
