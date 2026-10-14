use nix::{
    errno::Errno,
    sys::signal::{Signal, killpg},
    unistd::Pid,
};
use std::io;
use tokio::process::{Child, Command};

#[derive(Debug)]
pub(super) struct Tree(Option<Pid>);

pub(super) fn spawn(command: &mut Command) -> io::Result<(Child, Tree)> {
    let child = command.process_group(0).kill_on_drop(true).spawn()?;
    let pid = child
        .id()
        .ok_or_else(|| io::Error::other("child PID missing"))?;
    Ok((child, Tree(Some(Pid::from_raw(pid as i32)))))
}
impl Tree {
    fn signal(&self, signal: Signal) -> io::Result<()> {
        let Some(pid) = self.0 else {
            return Ok(());
        };
        match killpg(pid, signal) {
            Ok(()) | Err(Errno::ESRCH) => Ok(()),
            Err(error) => Err(io::Error::from_raw_os_error(error as i32)),
        }
    }
    pub(super) fn request_termination(&self) -> io::Result<bool> {
        self.signal(Signal::SIGTERM)?;
        Ok(true)
    }
    pub(super) fn kill(&self) -> io::Result<()> {
        self.signal(Signal::SIGKILL)
    }
    pub(super) async fn wait_empty(&self) -> io::Result<()> {
        // Descendants are reaped by their own parents/init, not this process.
        // Preserve Unix group-kill semantics; killpg(0) also counts zombies and
        // cannot establish when a non-child was reaped.
        Ok(())
    }
    pub(super) fn disarm(&mut self) {
        self.0 = None;
    }
}

pub(super) fn spawn_daemon(command: &mut Command) -> io::Result<Child> {
    command.process_group(0).spawn()
}

/// Register before startup work so SIGTERM/Ctrl+C use the owned cleanup path.
pub struct Shutdown {
    interrupt: tokio::signal::unix::Signal,
    terminate: tokio::signal::unix::Signal,
}
impl Shutdown {
    pub fn new() -> io::Result<Self> {
        use tokio::signal::unix::{SignalKind, signal};
        Ok(Self {
            interrupt: signal(SignalKind::interrupt())?,
            terminate: signal(SignalKind::terminate())?,
        })
    }
    pub async fn recv(&mut self) {
        tokio::select! { _ = self.interrupt.recv() => {}, _ = self.terminate.recv() => {} }
    }
}
