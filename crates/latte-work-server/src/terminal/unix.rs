//! Host-owned interactive PTYs. Output is a bounded byte ring, never persisted.
use anyhow::{Context, Result, bail};
use latte_work_protocol::{Response, TerminalInfo};
use nix::{
    sys::signal::{Signal, killpg},
    unistd::Pid,
};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::{
    collections::{HashMap, VecDeque},
    io::{Read, Write},
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, oneshot};

const MAX_TERMINALS: usize = 16;
const OUTPUT_LIMIT: usize = 1024 * 1024;
const READ_LIMIT: usize = 64 * 1024;
const INPUT_LIMIT: usize = 16 * 1024;
#[derive(Default)]
pub struct Terminals {
    entries: HashMap<String, Terminal>,
}
pub type SharedTerminals = Arc<Mutex<Terminals>>;
struct Terminal {
    info: TerminalInfo,
    output: Arc<Mutex<Output>>,
    control: Arc<Mutex<Control>>,
    input: mpsc::Sender<Input>,
    stopped: Arc<AtomicBool>,
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
        let excess = self.bytes.len().saturating_sub(OUTPUT_LIMIT);
        self.bytes.drain(..excess);
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
struct Control {
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    exited: bool,
    exit_code: Option<u32>,
    closing: Option<Instant>,
}
impl Control {
    fn stop(&mut self) {
        if self.exited {
            return;
        }
        if self.closing.is_some() {
            return;
        }
        self.closing = Some(Instant::now());
        self.stop_foreground();
    }
    fn stop_foreground(&self) {
        let pid = self.child.process_id().map(|p| p as i32);
        // Let the shell reap the foreground job before asking it to exit.
        if let Some(group) = self
            .master
            .process_group_leader()
            .filter(|p| *p > 1 && Some(*p) != pid)
        {
            let _ = killpg(Pid::from_raw(group), Signal::SIGKILL);
        }
    }
    fn advance_close(&self) {
        let Some(at) = self.closing else { return };
        // A just-started job can become foreground after close was requested.
        self.stop_foreground();
        if at.elapsed() < Duration::from_millis(100) {
            return;
        }
        if let Some(pid) = self.child.process_id().filter(|pid| *pid > 1) {
            let signal = if at.elapsed() < Duration::from_millis(500) {
                Signal::SIGHUP
            } else {
                // Enforce the whole-group deadline without blocking this supervisor
                // on portable-pty's per-child hangup grace period.
                Signal::SIGKILL
            };
            let _ = killpg(Pid::from_raw(pid as i32), signal);
        }
    }
}
struct Input {
    data: Vec<u8>,
    reply: oneshot::Sender<Result<(), String>>,
}
fn size(cols: u16, rows: u16) -> Result<PtySize> {
    if !(2..=500).contains(&cols) || !(1..=300).contains(&rows) {
        bail!("终端尺寸超出范围");
    }
    Ok(PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    })
}
impl Terminal {
    fn snapshot(&self) -> Result<TerminalInfo> {
        let control = self
            .control
            .lock()
            .map_err(|_| anyhow::anyhow!("终端锁异常"))?;
        let mut info = self.info.clone();
        info.exited = control.exited
            && self
                .output
                .lock()
                .map_err(|_| anyhow::anyhow!("终端输出锁异常"))?
                .eof;
        info.exit_code = control.exit_code;
        Ok(info)
    }
}
impl Drop for Terminal {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Relaxed);
        if let Ok(mut control) = self.control.lock() {
            control.stop();
        }
    }
}
impl Terminals {
    pub fn is_open(&self, id: &str) -> bool {
        self.entries
            .get(id)
            .is_some_and(|t| t.control.lock().is_ok_and(|c| !c.exited))
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn list(&self, project_id: &str) -> Result<Response> {
        let mut terminals = self
            .entries
            .values()
            .filter(|t| t.info.project_id == project_id)
            .map(Terminal::snapshot)
            .collect::<Result<Vec<_>>>()?;
        terminals.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(Response::Terminals { terminals })
    }
    pub fn create(
        &mut self,
        project_id: String,
        path: &Path,
        id: String,
        cols: u16,
        rows: u16,
    ) -> Result<Response> {
        let dimensions = size(cols, rows)?;
        uuid::Uuid::parse_str(&id).context("无效的终端 ID")?;
        if let Some(existing) = self.entries.get(&id) {
            if existing.info.project_id != project_id {
                bail!("终端 ID 已属于另一个项目");
            }
            return Ok(Response::Terminal {
                terminal: existing.snapshot()?,
            });
        }
        if self.entries.len() >= MAX_TERMINALS {
            bail!("此 Host 最多打开 16 个终端，请先关闭不使用的终端");
        }
        let path = path.canonicalize().context("项目目录不存在")?;
        if !path.is_dir() {
            bail!("项目路径不是目录");
        }
        let pair = native_pty_system().openpty(dimensions)?;
        let mut command = CommandBuilder::new_default_prog();
        command.cwd(path);
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");
        // Duplicates share O_NONBLOCK. Safe descriptor APIs avoid uninterruptible
        // I/O threads when a detached/background job retains the slave PTY.
        let fd = pair.master.as_raw_fd().context("Host 不支持 Unix PTY")?;
        let mut reader = filedescriptor::FileDescriptor::dup(&fd)?;
        reader.set_non_blocking(true)?;
        let mut writer = reader.try_clone()?;
        let child = pair
            .slave
            .spawn_command(command)
            .context("无法启动终端 Shell")?;
        drop(pair.slave);
        let control = Arc::new(Mutex::new(Control {
            master: pair.master,
            child,
            exited: false,
            exit_code: None,
            closing: None,
        }));
        let output = Arc::new(Mutex::new(Output::default()));
        let (input, mut receive) = mpsc::channel::<Input>(8);
        let stopped = Arc::new(AtomicBool::new(false));
        let terminal = Terminal {
            info: TerminalInfo {
                id: id.clone(),
                project_id,
                title: std::env::var("SHELL")
                    .ok()
                    .and_then(|s| {
                        Path::new(&s)
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                    })
                    .unwrap_or_else(|| "终端".into()),
                exited: false,
                exit_code: None,
            },
            control: control.clone(),
            output: output.clone(),
            input,
            stopped: stopped.clone(),
        };
        // Bounded queues and one reader/writer per PTY. Blocking PTY I/O never holds
        // the host dispatcher or the control lock, so close can unblock a writer.
        // Keep draining until the child exits. Stopping reads on removal can leave
        // a macOS shell stuck in tty teardown, even after SIGKILL was delivered.
        let read_stopped = Arc::new(AtomicBool::new(false));
        let read_done = read_stopped.clone();
        std::thread::spawn(move || {
            let mut buffer = [0; 8192];
            while !read_stopped.load(Ordering::Relaxed) {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(n) => {
                        if let Ok(mut output) = output.lock() {
                            output.append(&buffer[..n]);
                        } else {
                            break;
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10))
                    }
                    Err(_) => break, // Linux PTYs return EIO when the slave closes.
                }
            }
            if let Ok(mut output) = output.lock() {
                output.eof = true;
            }
        });
        let write_stopped = stopped.clone();
        std::thread::spawn(move || {
            while let Some(input) = receive.blocking_recv() {
                let deadline = Instant::now() + Duration::from_secs(2);
                let result = (|| -> Result<(), String> {
                    let mut remaining = input.data.as_slice();
                    while !remaining.is_empty() {
                        if write_stopped.load(Ordering::Relaxed) || Instant::now() >= deadline {
                            return Err("终端已关闭或输入超时".into());
                        }
                        match writer.write(remaining) {
                            Ok(0) => return Err("终端输入已关闭".into()),
                            Ok(n) => remaining = &remaining[n..],
                            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => (),
                            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                                std::thread::sleep(Duration::from_millis(10))
                            }
                            Err(e) => return Err(e.to_string()),
                        }
                    }
                    Ok(())
                })();
                let failed = result.is_err();
                let _ = input.reply.send(result);
                if failed {
                    break;
                }
            }
        });
        std::thread::spawn(move || {
            loop {
                if let Ok(mut c) = control.lock() {
                    c.advance_close();
                    match c.child.try_wait() {
                        Ok(Some(status)) => {
                            c.exited = true;
                            c.exit_code = Some(status.exit_code());
                            // Allow final bytes to drain, then release any inherited PTY fds.
                            drop(c);
                            std::thread::sleep(Duration::from_millis(100));
                            stopped.store(true, Ordering::Relaxed);
                            read_done.store(true, Ordering::Relaxed);
                            break;
                        }
                        Ok(None) => (),
                        Err(_) => {
                            c.stop();
                            // The timed close loop cannot continue after a wait error.
                            let _ = c.child.kill();
                            c.exited = true;
                            stopped.store(true, Ordering::Relaxed);
                            read_done.store(true, Ordering::Relaxed);
                            break;
                        }
                    }
                } else {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        });
        let info = terminal.snapshot()?;
        self.entries.insert(id, terminal);
        Ok(Response::Terminal { terminal: info })
    }
    fn get(&self, id: &str) -> Result<&Terminal> {
        self.entries
            .get(id)
            .context("终端已关闭或 Host 已重启，请新建终端")
    }
    pub fn read(&self, id: &str, after: f64) -> Result<Response> {
        let terminal = self.get(id)?;
        let info = terminal.snapshot()?;
        let output = terminal
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
    pub fn resize(&self, id: &str, cols: u16, rows: u16) -> Result<Response> {
        let size = size(cols, rows)?;
        let control = self
            .get(id)?
            .control
            .lock()
            .map_err(|_| anyhow::anyhow!("终端锁异常"))?;
        if control.exited {
            bail!("终端已退出");
        }
        control.master.resize(size)?;
        Ok(Response::Ok)
    }
    pub fn close(&mut self, id: &str) -> Response {
        self.entries.remove(id);
        Response::Ok
    }
    pub fn close_all(&mut self) {
        self.entries.clear();
    }
}
pub async fn write(terminals: &SharedTerminals, id: &str, data: Vec<u8>) -> Result<Response> {
    if data.is_empty() || data.len() > INPUT_LIMIT {
        bail!("终端输入必须为 1–16384 字节");
    }
    let (sender, control) = {
        let terminals = terminals
            .lock()
            .map_err(|_| anyhow::anyhow!("终端锁异常"))?;
        let t = terminals.get(id)?;
        if t.control
            .lock()
            .map_err(|_| anyhow::anyhow!("终端锁异常"))?
            .exited
        {
            bail!("终端已退出");
        }
        (t.input.clone(), t.control.clone())
    };
    let (reply, receive) = oneshot::channel();
    sender
        .try_send(Input { data, reply })
        .map_err(|_| anyhow::anyhow!("终端输入队列已满或已关闭；输入未发送"))?;
    match tokio::time::timeout(Duration::from_secs(2), receive).await {
        Ok(Ok(Ok(()))) => Ok(Response::Ok),
        result => {
            if let Ok(mut c) = control.lock() {
                c.stop();
            }
            bail!("终端输入失败或超时，已停止终端；输入可能部分送达，请勿自动重试：{result:?}");
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ring_is_bounded_and_cursors_reveal_loss_without_splitting_text() {
        let mut output = Output::default();
        output.append(&vec![b'x'; OUTPUT_LIMIT + 7]);
        assert_eq!(output.bytes.len(), OUTPUT_LIMIT);
        let (data, next, more, lost) = output.read(0.0).unwrap();
        assert_eq!(data.len(), READ_LIMIT);
        assert_eq!(next, (7 + READ_LIMIT) as f64);
        assert!(more && lost);
        assert!(output.read(-1.0).is_err());
        assert!(output.read(f64::NAN).is_err());
        assert!(output.read(0.5).is_err());
        assert!(output.read((OUTPUT_LIMIT + 8) as f64).is_err());
        assert!(size(0, 24).is_err());
        assert!(size(80, 301).is_err());
    }
}
