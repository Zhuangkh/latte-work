use super::*;
use futures_util::StreamExt;
use latte_work_platform::process::ManagedChild;
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, process::Stdio};
use tokio::{io::AsyncWriteExt, process::Command as ProcessCommand, time::Instant};
use tokio_util::codec::{FramedRead, LinesCodec};
#[path = "windows/helper.rs"]
mod helper;
pub(super) fn helper() -> Result<()> {
    helper::run()
}
const FRAME_LIMIT: usize = 128 * 1024;
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Message {
    Start {
        shell: PathBuf,
        cwd: PathBuf,
        cols: u16,
        rows: u16,
        no_profile: bool,
    },
    Write {
        id: u64,
        data: Vec<u8>,
    },
    Resize {
        id: u64,
        cols: u16,
        rows: u16,
    },
    Close,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Event {
    Ready,
    Output { data: Vec<u8> },
    Ack { id: u64 },
    Exited { code: Option<u32> },
    Error { message: String },
}
fn launch_path(path: &Path) -> PathBuf {
    use std::path::{Component, Prefix};
    if let Some(Component::Prefix(prefix)) = path.components().next()
        && let Prefix::VerbatimDisk(drive) = prefix.kind()
    {
        // PowerShell treats a verbatim cwd/argv[0] as an UNC provider path.
        // Preserve native components, changing only the local-drive spelling.
        let mut result = PathBuf::from(format!("{}:", drive as char));
        result.extend(path.components().skip(1).map(|c| c.as_os_str()));
        result
    } else {
        path.to_owned()
    }
}
fn shell(project: &Path) -> Result<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = std::env::var_os("PATH") {
        candidates.extend(
            std::env::split_paths(&path)
                .filter(|p| p.is_absolute())
                .map(|p| p.join("pwsh.exe")),
        );
    }
    if let Some(root) = std::env::var_os("ProgramFiles") {
        candidates.push(PathBuf::from(root).join("PowerShell/7/pwsh.exe"));
    }
    find_shell(candidates, project)
}
fn find_shell(candidates: Vec<PathBuf>, project: &Path) -> Result<PathBuf> {
    for candidate in candidates {
        if !candidate.is_file() {
            continue;
        }
        let resolved = candidate.canonicalize()?;
        if let Some(parent) = resolved.parent()
            && latte_work_platform::fs::directory_identity(parent)?
                != latte_work_platform::fs::directory_identity(project)?
        {
            return Ok(launch_path(&resolved));
        }
    }
    bail!("未找到 pwsh.exe，请安装 PowerShell 7 并将其加入 PATH；此 Host 首版仅支持 pwsh")
}
pub(super) fn spawn(
    path: &Path,
    cols: u16,
    rows: u16,
    output: Arc<Mutex<Output>>,
) -> Result<Backend> {
    let shell = shell(path)?;
    let mut command = ProcessCommand::new(std::env::current_exe()?);
    command
        .arg("__terminal")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .env("TERM", "xterm-256color")
        .env("COLORTERM", "truecolor");
    let child = ManagedChild::spawn(&mut command).context("无法启动终端辅助进程")?;
    let (commands, receive) = mpsc::channel(8);
    let (closing, close) = watch::channel(false);
    let (state, current) = watch::channel(State::default());
    let no_profile = cfg!(feature = "test-fixture")
        && std::env::var_os("LATTE_WORK_TEST_TERMINAL_NO_PROFILE").is_some();
    let start = Message::Start {
        shell,
        cwd: launch_path(path),
        cols,
        rows,
        no_profile,
    };
    tokio::spawn(supervise(child, start, receive, close, state, output));
    Ok(Backend {
        title: "pwsh".into(),
        commands,
        closing,
        state: current,
    })
}
async fn send(input: &mut tokio::process::ChildStdin, message: &Message) -> Result<()> {
    let mut bytes = serde_json::to_vec(message)?;
    if bytes.len() >= FRAME_LIMIT {
        bail!("终端控制帧过大");
    }
    bytes.push(b'\n');
    tokio::time::timeout(Duration::from_millis(500), input.write_all(&bytes))
        .await
        .context("终端控制管道写入超时")??;
    Ok(())
}
fn record(event: &Event, output: &Arc<Mutex<Output>>) -> Result<()> {
    if let Event::Output { data } = event {
        if data.is_empty() || data.len() > 8192 {
            bail!("终端输出帧超出限制");
        }
        output
            .lock()
            .map_err(|_| anyhow::anyhow!("终端输出锁异常"))?
            .append(data);
    }
    Ok(())
}
async fn communicate(
    child: &mut ManagedChild,
    start: Message,
    commands: &mut mpsc::Receiver<Command>,
    close: &mut watch::Receiver<bool>,
    state: &watch::Sender<State>,
    output: &Arc<Mutex<Output>>,
) -> Result<Option<u32>> {
    let mut input = child.stdin.take().context("终端输入管道缺失")?;
    let stdout = child.stdout.take().context("终端输出管道缺失")?;
    let mut lines = FramedRead::new(stdout, LinesCodec::new_with_max_length(FRAME_LIMIT));
    send(&mut input, &start).await?;
    let mut ready = false;
    let mut sequence = 0u64;
    let mut pending: Option<(u64, oneshot::Sender<Result<(), String>>)> = None;
    let mut deadline = Instant::now() + Duration::from_secs(5);
    loop {
        tokio::select! {
            biased;
            _ = close.changed() => {
                let _ = send(&mut input, &Message::Close).await;
                // Keep reading final output. The Job is the deadline fallback even
                // when the helper is blocked writing console input or closing HPCON.
                let end = Instant::now() + Duration::from_millis(500);
                while let Ok(Some(line)) = tokio::time::timeout_at(end, lines.next()).await {
                    let event: Event = serde_json::from_str(&line?)?;
                    record(&event, output)?;
                    match event {
                        Event::Exited { code } => return Ok(code),
                        Event::Error { message } => bail!("{message}"),
                        _ => (),
                    }
                }
                return Ok(None);
            }
            _ = tokio::time::sleep_until(deadline), if !ready || pending.is_some() => {
                bail!("终端启动或输入确认超时；输入可能部分送达，请勿自动重试");
            }
            line = lines.next() => {
                let line = line.context("终端辅助进程意外结束")??;
                let event: Event = serde_json::from_str(&line)?;
                record(&event, output)?;
                match event {
                    Event::Ready if !ready => { ready = true; state.send_modify(|s| s.ready = true); }
                    Event::Output { .. } => (),
                    Event::Ack { id } => {
                        let (expected, reply) = pending.take().context("意外终端确认")?;
                        if id != expected { bail!("终端输入确认不匹配"); }
                        let _ = reply.send(Ok(()));
                    }
                    Event::Exited { code } => return Ok(code),
                    Event::Error { message } => bail!("{message}"),
                    _ => bail!("重复终端启动确认"),
                }
            }
            command = commands.recv(), if ready && pending.is_none() => {
                let Some(command) = command else { return Ok(None) };
                sequence += 1;
                let message = match command.operation {
                    Operation::Write(data) => Message::Write { id: sequence, data },
                    Operation::Resize(cols, rows) => Message::Resize { id: sequence, cols, rows },
                };
                pending = Some((sequence, command.reply));
                deadline = Instant::now() + INPUT_TIMEOUT;
                send(&mut input, &message).await?;
            }
        }
    }
}
async fn supervise(
    mut child: ManagedChild,
    start: Message,
    mut commands: mpsc::Receiver<Command>,
    mut close: watch::Receiver<bool>,
    state: watch::Sender<State>,
    output: Arc<Mutex<Output>>,
) {
    let result = communicate(
        &mut child,
        start,
        &mut commands,
        &mut close,
        &state,
        &output,
    )
    .await;
    let mut final_state = state.borrow().clone();
    match result {
        Ok(code) => {
            final_state.exited = true;
            final_state.exit_code = code;
        }
        Err(error) => final_state.error = Some(format!("终端失败：{error:#}")),
    }
    // Retain the child/Job and ownership on cleanup failure. A later explicit close
    // retries cleanup only, never startup or input.
    loop {
        match child.close(Duration::ZERO).await {
            Ok(()) => {
                if let Ok(mut out) = output.lock() {
                    out.eof = true;
                }
                final_state.cleaned = true;
                state.send_replace(final_state);
                return;
            }
            Err(error) => {
                final_state.error = Some(format!("终端清理未确认，请重试关闭：{error}"));
                state.send_replace(final_state.clone());
                if close.changed().await.is_err() {
                    return;
                }
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_pwsh_is_explicit_and_project_local_candidates_are_excluded() {
        let root = tempfile::tempdir().unwrap();
        let cwd = root.path().canonicalize().unwrap();
        let candidate = cwd.join("pwsh.exe");
        assert!(
            find_shell(vec![candidate.clone()], &cwd)
                .unwrap_err()
                .to_string()
                .contains("PowerShell 7")
        );
        std::fs::write(&candidate, b"project-controlled file").unwrap();
        assert!(find_shell(vec![candidate], &cwd).is_err());
        assert_eq!(
            launch_path(Path::new(r"\\?\C:\中文 空格\file")),
            PathBuf::from(r"C:\中文 空格\file")
        );
    }
}
