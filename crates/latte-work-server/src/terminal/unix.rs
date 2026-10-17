use super::*;
use nix::{
    sys::signal::{Signal, killpg},
    unistd::Pid,
};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::{io::Write, time::Instant};
use tokio::io::{Interest, unix::AsyncFd};
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
    let reader = AsyncFd::with_interest(reader, Interest::READABLE)?;
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
async fn drain_output(
    reader: &mut AsyncFd<filedescriptor::FileDescriptor>,
    output: &Mutex<Output>,
) -> std::io::Result<bool> {
    let mut ready = reader.readable_mut().await?;
    let result = drain::drain(ready.get_inner_mut(), |bytes| {
        if let Ok(mut out) = output.lock() {
            out.append(bytes);
        }
    });
    match result {
        Ok(drain::Progress::WouldBlock) => {
            ready.clear_ready();
            Ok(false)
        }
        Ok(drain::Progress::Eof) => Ok(true),
        // Linux PTYs report EIO after the last slave closes.
        Err(error) if error.raw_os_error() == Some(nix::libc::EIO) => Ok(true),
        Err(error) => Err(error),
        Ok(drain::Progress::BudgetExhausted) => Ok(false),
    }
}
async fn supervise(
    mut control: Control,
    mut reader: AsyncFd<filedescriptor::FileDescriptor>,
    writer: filedescriptor::FileDescriptor,
    mut commands: mpsc::Receiver<Command>,
    mut close: watch::Receiver<bool>,
    state: watch::Sender<State>,
    output: Arc<Mutex<Output>>,
) {
    let mut pending: Option<tokio::task::JoinHandle<Result<(), String>>> = None;
    let mut closing: Option<Instant> = None;
    let mut exited: Option<Instant> = None;
    let mut output_ended = false;
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
            result = drain_output(&mut reader, &output), if !output_ended => {
                match result {
                    Ok(ended) => output_ended = ended,
                    Err(error) => {
                        current.error = Some(format!("终端读取失败：{error}"));
                        closing.get_or_insert_with(Instant::now);
                        output_ended = true;
                    }
                }
                // Keep readiness on budget exhaustion, but give input and other
                // tasks a turn before immediately continuing to drain output.
                tokio::task::yield_now().await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn descriptors() -> (
        AsyncFd<filedescriptor::FileDescriptor>,
        filedescriptor::FileDescriptor,
        tokio::net::UnixStream,
    ) {
        let (master, slave) = UnixStream::pair().unwrap();
        master.set_nonblocking(true).unwrap();
        slave.set_nonblocking(true).unwrap();
        let reader = filedescriptor::FileDescriptor::dup(&master).unwrap();
        let writer = reader.try_clone().unwrap();
        (
            AsyncFd::with_interest(reader, Interest::READABLE).unwrap(),
            writer,
            tokio::net::UnixStream::from_std(slave).unwrap(),
        )
    }

    fn fill_input(writer: &mut filedescriptor::FileDescriptor) -> usize {
        let mut queued = 0;
        for _ in 0..1024 {
            match writer.write(&[b'q'; INPUT_LIMIT]) {
                Ok(n) if n > 0 => queued += n,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return queued,
                result => panic!("unexpected socket write: {result:?}"),
            }
        }
        panic!("socket input did not block within 16 MiB");
    }

    #[tokio::test]
    async fn readiness_drains_large_output_before_the_peer_accepts_remaining_input() {
        let (mut reader, mut writer, mut slave) = descriptors();
        let output = Mutex::new(Output::default());
        let (_, close) = watch::channel(false);
        let mut queued = fill_input(&mut writer);
        let peer = async {
            slave.write_all(&vec![b'v'; 4 * 1024 * 1024]).await.unwrap();
            let mut rest = [0; INPUT_LIMIT];
            while queued != 0 {
                let length = rest.len().min(queued);
                slave.read_exact(&mut rest[..length]).await.unwrap();
                assert!(rest[..length].iter().all(|byte| *byte == b'q'));
                queued -= length;
            }
            slave.read_exact(&mut rest).await.unwrap();
            assert!(rest.iter().all(|byte| *byte == b'x'));
            slave.write_all(b"DUPLEX_OK").await.unwrap();
            drop(slave);
        };
        let read = async {
            while !drain_output(&mut reader, &output).await.unwrap() {
                tokio::task::yield_now().await;
            }
        };
        let data = vec![b'x'; INPUT_LIMIT];
        tokio::time::timeout(INPUT_TIMEOUT, async {
            let (result, (), ()) = tokio::join!(input(&mut writer, &data, &close), peer, read);
            result.unwrap();
        })
        .await
        .unwrap();
        let mut output = output.lock().unwrap();
        assert_eq!(output.end, (4 * 1024 * 1024 + 9) as u64);
        assert_eq!(output.bytes.len(), OUTPUT_LIMIT);
        assert!(output.bytes.make_contiguous().ends_with(b"DUPLEX_OK"));
    }

    #[tokio::test]
    async fn drained_readiness_waits_for_more_output_and_eof_finishes_reading() {
        let (mut reader, _writer, mut slave) = descriptors();
        let output = Mutex::new(Output::default());
        slave.write_all(b"ready").await.unwrap();
        assert!(!drain_output(&mut reader, &output).await.unwrap());
        assert_eq!(output.lock().unwrap().end, 5);
        assert!(
            tokio::time::timeout(
                Duration::from_millis(25),
                drain_output(&mut reader, &output)
            )
            .await
            .is_err()
        );
        drop(slave);
        assert!(
            tokio::time::timeout(
                Duration::from_millis(100),
                drain_output(&mut reader, &output)
            )
            .await
            .unwrap()
            .unwrap()
        );
    }

    #[tokio::test]
    async fn blocked_input_observes_close_without_waiting_for_its_deadline() {
        let (_reader, mut writer, _slave) = descriptors();
        // Fill the peer's unread socket buffer before exercising input cancellation.
        fill_input(&mut writer);
        let (closing, close) = watch::channel(false);
        let write = input(&mut writer, b"unread", &close);
        let cancel = async {
            tokio::task::yield_now().await;
            closing.send(true).unwrap();
        };
        let (result, ()) = tokio::time::timeout(Duration::from_millis(100), async {
            tokio::join!(write, cancel)
        })
        .await
        .unwrap();
        assert!(result.is_err());
    }
}
