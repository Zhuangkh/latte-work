use super::*;
use nix::{
    sys::signal::{Signal, killpg},
    unistd::Pid,
};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::{
    io::{Read, Write},
    time::Instant,
};
struct Control {
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    reaped: bool,
}
impl Control {
    fn foreground(&self) {
        let pid = self.child.process_id().map(|p| p as i32);
        if let Some(group) = self
            .master
            .process_group_leader()
            .filter(|p| *p > 1 && Some(*p) != pid)
        {
            let _ = killpg(Pid::from_raw(group), Signal::SIGKILL);
        }
    }
    fn signal(&self, signal: Signal) {
        if self.reaped {
            return;
        }
        if let Some(pid) = self.child.process_id().filter(|p| *p > 1) {
            let _ = killpg(Pid::from_raw(pid as i32), signal);
        }
    }
}
impl Drop for Control {
    fn drop(&mut self) {
        self.foreground();
        self.signal(Signal::SIGKILL);
    }
}
fn dimensions(cols: u16, rows: u16) -> PtySize {
    PtySize {
        cols,
        rows,
        pixel_width: 0,
        pixel_height: 0,
    }
}
pub(super) fn helper() -> Result<()> {
    bail!("此 Host 不使用 ConPTY 辅助进程")
}
pub(super) fn spawn(
    path: &Path,
    cols: u16,
    rows: u16,
    output: Arc<Mutex<Output>>,
) -> Result<Backend> {
    let pair = native_pty_system().openpty(dimensions(cols, rows))?;
    let mut command = CommandBuilder::new_default_prog();
    command.cwd(path);
    command.env("TERM", "xterm-256color");
    command.env("COLORTERM", "truecolor");
    let fd = pair.master.as_raw_fd().context("Host 不支持 Unix PTY")?;
    let mut reader = filedescriptor::FileDescriptor::dup(&fd)?;
    reader.set_non_blocking(true)?;
    let writer = reader.try_clone()?;
    let child = pair
        .slave
        .spawn_command(command)
        .context("无法启动终端 Shell")?;
    drop(pair.slave);
    let control = Control {
        master: pair.master,
        child,
        reaped: false,
    };
    let (commands, receive) = mpsc::channel(8);
    let (closing, close) = watch::channel(false);
    let (state, current) = watch::channel(State {
        ready: true,
        ..State::default()
    });
    tokio::spawn(supervise(
        control, reader, writer, receive, close, state, output,
    ));
    Ok(Backend {
        title: std::env::var("SHELL")
            .ok()
            .and_then(|s| {
                Path::new(&s)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| "终端".into()),
        commands,
        closing,
        state: current,
    })
}
async fn input(
    writer: &mut filedescriptor::FileDescriptor,
    data: &[u8],
    close: &watch::Receiver<bool>,
) -> Result<()> {
    let deadline = Instant::now() + INPUT_TIMEOUT;
    let mut remaining = data;
    while !remaining.is_empty() {
        if *close.borrow() || Instant::now() >= deadline {
            bail!("终端已关闭或输入超时");
        }
        match writer.write(remaining) {
            Ok(0) => bail!("终端输入已关闭"),
            Ok(n) => remaining = &remaining[n..],
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => (),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                tokio::time::sleep(Duration::from_millis(10)).await
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}
async fn supervise(
    mut control: Control,
    mut reader: filedescriptor::FileDescriptor,
    writer: filedescriptor::FileDescriptor,
    mut commands: mpsc::Receiver<Command>,
    mut close: watch::Receiver<bool>,
    state: watch::Sender<State>,
    output: Arc<Mutex<Output>>,
) {
    let mut pending: Option<tokio::task::JoinHandle<Result<(), String>>> = None;
    let mut closing: Option<Instant> = None;
    let mut exited: Option<Instant> = None;
    let mut current = state.borrow().clone();
    let mut interval = tokio::time::interval(Duration::from_millis(10));
    loop {
        tokio::select! {
            biased;
            changed = close.changed(), if closing.is_none() => {
                let _ = changed;
                closing = Some(Instant::now());
            }
            _ = interval.tick() => {
                let mut buffer = [0; 8192];
                for _ in 0..8 {
                    match reader.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(n) => { if let Ok(mut out) = output.lock() { out.append(&buffer[..n]); } }
                        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock || error.raw_os_error() == Some(nix::libc::EIO) => break,
                        Err(error) => { current.error = Some(format!("终端读取失败：{error}")); closing.get_or_insert_with(Instant::now); break; }
                    }
                }
                if let Some(at) = closing {
                    control.foreground();
                    if at.elapsed() >= Duration::from_millis(100) {
                        control.signal(if at.elapsed() < Duration::from_millis(500) { Signal::SIGHUP } else { Signal::SIGKILL });
                    }
                }
                if exited.is_none() {
                    match control.child.try_wait() {
                        Ok(Some(status)) => { control.reaped = true; current.exit_code = Some(status.exit_code()); current.exited = true; exited = Some(Instant::now()); control.foreground(); }
                        Ok(None) => (),
                        Err(error) => { current.error = Some(format!("终端等待失败，清理未确认：{error}")); closing.get_or_insert_with(Instant::now); state.send_replace(current.clone()); }
                    }
                }
                if exited.is_some_and(|at| at.elapsed() >= Duration::from_millis(100)) {
                    if let Some(task) = pending.take() { task.abort(); let _ = task.await; }
                    drop(control);
                    drop(reader);
                    drop(writer);
                    if let Ok(mut out) = output.lock() { out.eof = true; }
                    current.cleaned = true;
                    state.send_replace(current);
                    return;
                }
                if closing.is_some_and(|at| at.elapsed() >= Duration::from_secs(3)) && exited.is_none() {
                    current.error = Some("终端清理未确认，请重试关闭".into());
                    state.send_replace(current.clone());
                    // Continue supervising; a late reap can still confirm cleanup.
                }
            }
            result = async { pending.as_mut().expect("pending write").await }, if pending.is_some() => {
                pending.take();
                if !matches!(result, Ok(Ok(()))) {
                    current.error = Some(format!("终端输入失败：{result:?}"));
                    state.send_replace(current.clone());
                    closing.get_or_insert_with(Instant::now);
                }
            }
            command = commands.recv(), if closing.is_none() && exited.is_none() && pending.is_none() => {
                let Some(command) = command else { closing = Some(Instant::now()); continue };
                match command.operation {
                    Operation::Write(data) => {
                        match writer.try_clone() {
                            Ok(mut writer) => {
                                let close = close.clone();
                                // A full input queue must not pause output draining:
                                // the child may need stdout space before it can read.
                                pending = Some(tokio::spawn(async move {
                                    let result = input(&mut writer, &data, &close).await.map_err(|e| e.to_string());
                                    let _ = command.reply.send(result.clone());
                                    result
                                }));
                            }
                            Err(error) => {
                                let _ = command.reply.send(Err(error.to_string()));
                                closing = Some(Instant::now());
                            }
                        }
                    }
                    Operation::Resize(cols, rows) => {
                        let result = control.master.resize(dimensions(cols, rows)).map_err(|e| e.to_string());
                        if result.is_err() { closing = Some(Instant::now()); }
                        let _ = command.reply.send(result);
                    }
                }
            }
        }
    }
}
