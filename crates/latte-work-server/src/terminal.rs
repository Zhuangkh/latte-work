//! Host-owned terminals. Registry, bounded output and close confirmation are shared.
#[cfg(unix)]
#[path = "terminal/unix.rs"]
mod backend;
#[cfg(windows)]
#[path = "terminal/windows.rs"]
mod backend;
#[cfg(any(unix, test))]
#[path = "terminal/drain.rs"]
mod drain;
use anyhow::{Context, Result, bail};
use latte_work_protocol::{Response, TerminalInfo};
use std::{
    collections::{HashMap, VecDeque},
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{mpsc, oneshot, watch};

pub const SUPPORTED: bool = true;
const MAX_TERMINALS: usize = 16;
const OUTPUT_LIMIT: usize = 1024 * 1024;
const READ_LIMIT: usize = 64 * 1024;
const INPUT_LIMIT: usize = 16 * 1024;
const INPUT_TIMEOUT: Duration = Duration::from_secs(2);
#[derive(Default)]
pub struct Terminals {
    entries: HashMap<String, Arc<Terminal>>,
}
pub type SharedTerminals = Arc<Mutex<Terminals>>;
struct Terminal {
    info: TerminalInfo,
    output: Arc<Mutex<Output>>,
    backend: Backend,
}
#[derive(Clone)]
struct Backend {
    title: String,
    commands: mpsc::Sender<Command>,
    closing: watch::Sender<bool>,
    state: watch::Receiver<State>,
}
#[derive(Clone, Default)]
struct State {
    ready: bool,
    exited: bool,
    exit_code: Option<u32>,
    cleaned: bool,
    error: Option<String>,
}
enum Operation {
    Write(Vec<u8>),
    Resize(u16, u16),
}
struct Command {
    operation: Operation,
    reply: oneshot::Sender<Result<(), String>>,
}
#[derive(Default)]
struct Output {
    bytes: VecDeque<u8>,
    end: u64,
    eof: bool,
}
impl Output {
    fn append(&mut self, bytes: &[u8]) {
        self.end += bytes.len() as u64;
        self.bytes.extend(bytes);
        self.bytes
            .drain(..self.bytes.len().saturating_sub(OUTPUT_LIMIT));
    }
    fn read(&self, after: f64) -> Result<(Vec<u8>, f64, bool, bool)> {
        if !after.is_finite() || after < 0.0 || after.fract() != 0.0 || after > self.end as f64 {
            bail!("无效的终端输出游标");
        }
        let start = self.end - self.bytes.len() as u64;
        let offset = (after as u64).max(start);
        let data: Vec<_> = self
            .bytes
            .iter()
            .skip((offset - start) as usize)
            .take(READ_LIMIT)
            .copied()
            .collect();
        let next = offset + data.len() as u64;
        Ok((data, next as f64, next < self.end, (after as u64) < start))
    }
}
fn size(cols: u16, rows: u16) -> Result<()> {
    if !(2..=500).contains(&cols) || !(1..=300).contains(&rows) {
        bail!("终端尺寸超出范围");
    }
    Ok(())
}
impl Terminal {
    fn snapshot(&self) -> Result<TerminalInfo> {
        let state = self.backend.state.borrow();
        let mut info = self.info.clone();
        info.exited = state.exited
            && state.cleaned
            && self
                .output
                .lock()
                .map_err(|_| anyhow::anyhow!("终端输出锁异常"))?
                .eof;
        info.exit_code = state.exit_code;
        Ok(info)
    }
}
impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self.backend.closing.send(true);
    }
}
impl Terminals {
    pub fn is_open(&self, id: &str) -> bool {
        self.entries.get(id).is_some_and(|t| {
            let s = t.backend.state.borrow();
            !s.cleaned || !s.exited
        })
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn list(&self, project_id: &str) -> Result<Response> {
        let mut terminals = self
            .entries
            .values()
            .filter(|t| t.info.project_id == project_id)
            .map(|t| t.snapshot())
            .collect::<Result<Vec<_>>>()?;
        terminals.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(Response::Terminals { terminals })
    }
    fn get(&self, id: &str) -> Result<Arc<Terminal>> {
        self.entries
            .get(id)
            .cloned()
            .context("终端已关闭或 Host 已重启，请新建终端")
    }
    pub fn read(&self, id: &str, after: f64) -> Result<Response> {
        let t = self.get(id)?;
        if let Some(error) = &t.backend.state.borrow().error {
            bail!("{error}");
        }
        let info = t.snapshot()?;
        let output = t
            .output
            .lock()
            .map_err(|_| anyhow::anyhow!("终端输出锁异常"))?;
        let (data, next, has_more, truncated) = output.read(after)?;
        Ok(Response::TerminalOutput {
            terminal: info,
            data,
            next,
            has_more,
            truncated,
        })
    }
}
pub async fn create(
    terminals: &SharedTerminals,
    project_id: String,
    path: &Path,
    id: String,
    cols: u16,
    rows: u16,
) -> Result<Response> {
    size(cols, rows)?;
    uuid::Uuid::parse_str(&id).context("无效的终端 ID")?;
    let terminal = {
        let mut registry = terminals
            .lock()
            .map_err(|_| anyhow::anyhow!("终端锁异常"))?;
        if let Some(existing) = registry.entries.get(&id) {
            if existing.info.project_id != project_id {
                bail!("终端 ID 已属于另一个项目");
            }
            if *existing.backend.closing.borrow() {
                bail!("终端正在关闭，请等待清理完成");
            }
            existing.clone()
        } else {
            if registry.entries.len() >= MAX_TERMINALS {
                bail!("此 Host 最多打开 16 个终端，请先关闭不使用的终端");
            }
            let path = path.canonicalize().context("项目目录不存在")?;
            if !path.is_dir() {
                bail!("项目路径不是目录");
            }
            let output = Arc::new(Mutex::new(Output::default()));
            let backend = backend::spawn(&path, cols, rows, output.clone())?;
            let terminal = Arc::new(Terminal {
                info: TerminalInfo {
                    id: id.clone(),
                    project_id,
                    title: backend.title.clone(),
                    exited: false,
                    exit_code: None,
                },
                output,
                backend,
            });
            registry.entries.insert(id.clone(), terminal.clone());
            terminal
        }
    };
    let mut state = terminal.backend.state.clone();
    let result = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let current = state.borrow().clone();
            if let Some(error) = current.error {
                bail!("{error}");
            }
            if current.ready {
                return terminal.snapshot();
            }
            state.changed().await.context("终端启动中断")?;
        }
    })
    .await;
    match result {
        Ok(Ok(terminal)) => Ok(Response::Terminal { terminal }),
        other => {
            let _ = terminal.backend.closing.send(true);
            // Cleanup continues in its backend even if this caller disconnects.
            let _ = close(terminals, &id).await;
            bail!("无法启动终端：{other:?}");
        }
    }
}
async fn operation(
    terminals: &SharedTerminals,
    id: &str,
    operation: Operation,
) -> Result<Response> {
    let t = terminals
        .lock()
        .map_err(|_| anyhow::anyhow!("终端锁异常"))?
        .get(id)?;
    {
        let state = t.backend.state.borrow();
        if let Some(error) = &state.error {
            bail!("{error}");
        }
        if !state.ready || state.exited || *t.backend.closing.borrow() {
            bail!("终端已退出或正在关闭");
        }
    }
    let (reply, receive) = oneshot::channel();
    t.backend
        .commands
        .try_send(Command { operation, reply })
        .map_err(|_| anyhow::anyhow!("终端输入队列已满或已关闭；输入未发送"))?;
    match tokio::time::timeout(INPUT_TIMEOUT, receive).await {
        Ok(Ok(Ok(()))) => Ok(Response::Ok),
        result => {
            let _ = t.backend.closing.send(true);
            bail!("终端操作失败或超时，已请求停止终端；输入可能部分送达，请勿自动重试：{result:?}");
        }
    }
}
pub async fn write(terminals: &SharedTerminals, id: &str, data: Vec<u8>) -> Result<Response> {
    if data.is_empty() || data.len() > INPUT_LIMIT {
        bail!("终端输入必须为 1–16384 字节");
    }
    operation(terminals, id, Operation::Write(data)).await
}
pub async fn resize(
    terminals: &SharedTerminals,
    id: &str,
    cols: u16,
    rows: u16,
) -> Result<Response> {
    size(cols, rows)?;
    operation(terminals, id, Operation::Resize(cols, rows)).await
}
pub async fn close(terminals: &SharedTerminals, id: &str) -> Result<Response> {
    let t = {
        let registry = terminals
            .lock()
            .map_err(|_| anyhow::anyhow!("终端锁异常"))?;
        let Some(t) = registry.entries.get(id) else {
            return Ok(Response::Ok);
        };
        t.clone()
    };
    let mut state = t.backend.state.clone();
    let _ = t.backend.closing.send(true);
    tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            if state.borrow().cleaned {
                return Ok::<_, anyhow::Error>(());
            }
            state
                .changed()
                .await
                .context("终端监督已结束，但未确认清理")?;
        }
    })
    .await
    .context("终端清理未确认，请重试关闭；资源归属仍保留")??;
    let mut registry = terminals
        .lock()
        .map_err(|_| anyhow::anyhow!("终端锁异常"))?;
    if registry
        .entries
        .get(id)
        .is_some_and(|entry| Arc::ptr_eq(entry, &t))
    {
        registry.entries.remove(id);
    }
    Ok(Response::Ok)
}
pub async fn close_all(terminals: &SharedTerminals) -> Result<()> {
    let ids: Vec<_> = terminals
        .lock()
        .map_err(|_| anyhow::anyhow!("终端锁异常"))?
        .entries
        .keys()
        .cloned()
        .collect();
    let results = futures_util::future::join_all(ids.iter().map(|id| close(terminals, id))).await;
    for result in results {
        result?;
    }
    Ok(())
}
/// Called before constructing Tokio; helper stdin threads end with this process.
pub fn helper() -> Result<()> {
    backend::helper()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input_fixture() -> (
        SharedTerminals,
        mpsc::Receiver<Command>,
        watch::Receiver<bool>,
    ) {
        let terminals = SharedTerminals::default();
        let (commands, receive) = mpsc::channel(8);
        let (closing, close) = watch::channel(false);
        let (_state, current) = watch::channel(State {
            ready: true,
            ..State::default()
        });
        terminals.lock().unwrap().entries.insert(
            "input".into(),
            Arc::new(Terminal {
                info: TerminalInfo {
                    id: "input".into(),
                    project_id: "project".into(),
                    title: "fixture".into(),
                    exited: false,
                    exit_code: None,
                },
                output: Arc::new(Mutex::new(Output::default())),
                backend: Backend {
                    title: "fixture".into(),
                    commands,
                    closing,
                    state: current,
                },
            }),
        );
        (terminals, receive, close)
    }

    #[tokio::test]
    async fn full_input_queue_rejects_without_requesting_close_or_sending_more_input() {
        let (terminals, mut receive, close) = input_fixture();
        let terminal = terminals.lock().unwrap().get("input").unwrap();
        for _ in 0..8 {
            let (reply, _) = oneshot::channel();
            terminal
                .backend
                .commands
                .try_send(Command {
                    operation: Operation::Write(vec![b'x']),
                    reply,
                })
                .ok()
                .unwrap();
        }
        let error = write(&terminals, "input", vec![b'y']).await.unwrap_err();
        assert!(error.to_string().contains("输入未发送"));
        assert!(!*close.borrow());
        for _ in 0..8 {
            assert!(matches!(
                receive.try_recv().unwrap().operation,
                Operation::Write(data) if data == b"x"
            ));
        }
        assert!(receive.try_recv().is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn timed_out_input_requests_close_without_replaying_the_command() {
        let (terminals, mut receive, close) = input_fixture();
        let request = tokio::spawn(async move { write(&terminals, "input", vec![b'x']).await });
        let command = receive.recv().await.unwrap();
        tokio::time::advance(INPUT_TIMEOUT).await;
        let error = request.await.unwrap().unwrap_err();
        assert!(error.to_string().contains("输入可能部分送达，请勿自动重试"));
        assert!(*close.borrow());
        assert!(command.reply.is_closed());
        assert!(receive.try_recv().is_err());
    }

    #[tokio::test]
    async fn cancelled_close_keeps_registry_until_cleanup_is_confirmed() {
        let terminals = SharedTerminals::default();
        let (commands, _commands) = mpsc::channel(8);
        let (closing, mut close_requested) = watch::channel(false);
        let (state, current) = watch::channel(State {
            ready: true,
            ..State::default()
        });
        terminals.lock().unwrap().entries.insert(
            "owned".into(),
            Arc::new(Terminal {
                info: TerminalInfo {
                    id: "owned".into(),
                    project_id: "project".into(),
                    title: "fixture".into(),
                    exited: false,
                    exit_code: None,
                },
                output: Arc::new(Mutex::new(Output::default())),
                backend: Backend {
                    title: "fixture".into(),
                    commands,
                    closing,
                    state: current,
                },
            }),
        );
        let first = terminals.clone();
        let waiting = tokio::spawn(async move { close(&first, "owned").await });
        close_requested.changed().await.unwrap();
        waiting.abort();
        let _ = waiting.await;
        assert!(!terminals.lock().unwrap().is_empty());
        state.send_modify(|s| s.error = Some("cleanup not confirmed".into()));
        assert!(terminals.lock().unwrap().is_open("owned"));
        state.send_modify(|s| s.cleaned = true);
        close(&terminals, "owned").await.unwrap();
        assert!(terminals.lock().unwrap().is_empty());
    }
    #[test]
    fn ring_is_bounded_and_cursors_reveal_loss_without_splitting_text() {
        let mut output = Output::default();
        output.append(&vec![b'x'; OUTPUT_LIMIT + 7]);
        assert_eq!(output.bytes.len(), OUTPUT_LIMIT);
        let (data, next, more, lost) = output.read(0.0).unwrap();
        assert_eq!(data.len(), READ_LIMIT);
        assert_eq!(next, (7 + READ_LIMIT) as f64);
        assert!(more && lost);
        for cursor in [-1.0, f64::NAN, 0.5, (OUTPUT_LIMIT + 8) as f64] {
            assert!(output.read(cursor).is_err());
        }
        assert!(size(0, 24).is_err());
        assert!(size(80, 301).is_err());
    }
}
