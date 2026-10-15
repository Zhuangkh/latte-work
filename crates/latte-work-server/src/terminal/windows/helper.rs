//! Disposable ConPTY process. Stdin/output threads are bounded by its parent Job.
use super::{Event, FRAME_LIMIT, Message};
use anyhow::{Context, Result, bail};
use latte_work_platform::terminal::PseudoConsole;
use std::{
    io::{self, BufRead, BufReader, Read, Write},
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};
type Writer = Arc<Mutex<io::Stdout>>;
fn emit(writer: &Writer, event: Event) -> Result<()> {
    let mut bytes = serde_json::to_vec(&event)?;
    if bytes.len() >= FRAME_LIMIT {
        bail!("终端辅助帧超出限制");
    }
    bytes.push(b'\n');
    let mut writer = writer
        .lock()
        .map_err(|_| anyhow::anyhow!("终端输出锁异常"))?;
    writer.write_all(&bytes)?;
    writer.flush()?;
    Ok(())
}
fn read(reader: &mut impl BufRead) -> Result<Option<Message>> {
    let mut bytes = Vec::new();
    (&mut *reader)
        .take((FRAME_LIMIT + 1) as u64)
        .read_until(b'\n', &mut bytes)?;
    if bytes.is_empty() {
        return Ok(None);
    }
    if bytes.len() > FRAME_LIMIT || bytes.last() != Some(&b'\n') {
        bail!("终端输入帧过大或不完整");
    }
    Ok(Some(serde_json::from_slice(&bytes)?))
}
pub(super) fn run() -> Result<()> {
    let writer = Arc::new(Mutex::new(io::stdout()));
    let result = session(&writer);
    if let Err(error) = &result {
        let _ = emit(
            &writer,
            Event::Error {
                message: format!("{error:#}"),
            },
        );
    }
    result
}
fn session(writer: &Writer) -> Result<()> {
    let mut reader = BufReader::new(io::stdin());
    let Some(Message::Start {
        shell,
        cwd,
        cols,
        rows,
        no_profile,
    }) = read(&mut reader)?
    else {
        bail!("缺少终端启动帧")
    };
    if !shell.is_absolute()
        || !cwd.is_absolute()
        || shell
            .file_name()
            .is_none_or(|s| !s.eq_ignore_ascii_case("pwsh.exe"))
    {
        bail!("无效的 pwsh 启动路径");
    }
    let (mut console, mut input, mut output) = PseudoConsole::open(cols, rows)?;
    let (finished, drained) = mpsc::sync_channel(1);
    let out = writer.clone();
    std::thread::Builder::new()
        .name("conpty-output".into())
        .spawn(move || {
            let result = (|| -> Result<()> {
                let mut buffer = [0; 8192];
                loop {
                    let count = match output.read(&mut buffer) {
                        Ok(0) => return Ok(()),
                        Ok(count) => count,
                        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => return Ok(()),
                        Err(error) => return Err(error.into()),
                    };
                    emit(
                        &out,
                        Event::Output {
                            data: buffer[..count].to_vec(),
                        },
                    )?;
                }
            })();
            let _ = finished.send(result);
        })?;
    let mut args = vec!["-NoLogo".into()];
    if no_profile {
        args.push("-NoProfile".into());
    }
    let child = console
        .spawn(&shell, &args, &cwd)
        .context("无法启动 pwsh")?;
    emit(writer, Event::Ready)?;
    let (sender, commands) = mpsc::sync_channel(8);
    std::thread::Builder::new()
        .name("conpty-control".into())
        .spawn(move || {
            loop {
                let message = read(&mut reader);
                let done = !matches!(&message, Ok(Some(_)));
                if sender.send(message).is_err() || done {
                    break;
                }
            }
        })?;
    let code = loop {
        if let Some(code) = child.try_wait()? {
            break Some(code);
        }
        match commands.recv_timeout(Duration::from_millis(20)) {
            Ok(Ok(Some(Message::Write { id, data }))) => {
                if data.is_empty() || data.len() > super::INPUT_LIMIT {
                    bail!("终端输入超出限制");
                }
                input.write_all(&data)?;
                emit(writer, Event::Ack { id })?;
            }
            Ok(Ok(Some(Message::Resize { id, cols, rows }))) => {
                console.resize(cols, rows)?;
                emit(writer, Event::Ack { id })?;
            }
            Ok(Ok(Some(Message::Close)))
            | Ok(Ok(None))
            | Err(mpsc::RecvTimeoutError::Disconnected) => break None,
            Ok(Ok(Some(Message::Start { .. }))) => bail!("重复终端启动帧"),
            Ok(Err(error)) => return Err(error),
            Err(mpsc::RecvTimeoutError::Timeout) => (),
        }
    };
    drop(input);
    drop(console);
    drained
        .recv_timeout(Duration::from_secs(1))
        .context("终端输出未排空")??;
    // Serialized stdout guarantees every output frame precedes Exited.
    emit(writer, Event::Exited { code })
}
