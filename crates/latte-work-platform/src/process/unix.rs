use nix::{
    errno::Errno,
    sys::signal::{Signal, killpg},
    unistd::Pid,
};
use std::{fmt, io, time::Duration};
use tokio::{
    process::{Child, Command},
    time::Instant,
};

#[derive(Debug)]
pub(super) struct Tree {
    pgid: Option<Pid>,
    #[cfg(test)]
    kill_error: Option<Errno>,
}

#[derive(Debug)]
struct SignalError {
    signal: Signal,
    pgid: Pid,
    source: Errno,
}
impl fmt::Display for SignalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "killpg({}, PGID={}) failed: errno {}: {}",
            self.signal, self.pgid, self.source as i32, self.source
        )
    }
}
impl std::error::Error for SignalError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

#[derive(Debug)]
struct CleanupDeadlineError(io::Error);
impl fmt::Display for CleanupDeadlineError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}; process tree cleanup deadline exceeded",
            self.0
        )
    }
}
impl std::error::Error for CleanupDeadlineError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

fn retryable_kill(error: &io::Error) -> bool {
    error
        .get_ref()
        .and_then(|error| error.downcast_ref::<SignalError>())
        .is_some_and(|error| error.signal == Signal::SIGKILL && error.source == Errno::EPERM)
}

fn signal_result(signal: Signal, pgid: Pid, result: Result<(), Errno>) -> io::Result<()> {
    match result {
        Ok(()) | Err(Errno::ESRCH) => Ok(()),
        Err(source) => Err(io::Error::new(
            io::Error::from_raw_os_error(source as i32).kind(),
            SignalError {
                signal,
                pgid,
                source,
            },
        )),
    }
}

async fn kill_with_retry(
    deadline: Instant,
    mut kill: impl FnMut() -> io::Result<()>,
    mut reap_root: impl FnMut() -> io::Result<()>,
    retry_permission_denied: bool,
) -> io::Result<()> {
    loop {
        let error = match kill() {
            Ok(()) => return Ok(()),
            Err(error) if retry_permission_denied && retryable_kill(&error) => error,
            Err(error) => return Err(error),
        };
        // Darwin can report EPERM while the group contains exiting processes.
        // Reap our root if ready, but only a later successful group signal can
        // establish signaling success. Root exit is never sufficient by itself.
        reap_root()?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        if !remaining.is_zero() {
            tokio::time::sleep(remaining.min(Duration::from_millis(10))).await;
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(error.kind(), CleanupDeadlineError(error)));
        }
    }
}

pub(super) fn spawn(command: &mut Command) -> io::Result<(Child, Tree)> {
    let child = command.process_group(0).kill_on_drop(true).spawn()?;
    let pid = child
        .id()
        .ok_or_else(|| io::Error::other("child PID missing"))?;
    Ok((
        child,
        Tree {
            pgid: Some(Pid::from_raw(pid as i32)),
            #[cfg(test)]
            kill_error: None,
        },
    ))
}
impl Tree {
    fn signal(&self, signal: Signal) -> io::Result<()> {
        let Some(pid) = self.pgid else {
            return Ok(());
        };
        signal_result(signal, pid, killpg(pid, signal))
    }
    pub(super) fn request_termination(&self) -> io::Result<bool> {
        self.signal(Signal::SIGTERM)?;
        Ok(true)
    }
    pub(super) fn kill(&self) -> io::Result<()> {
        self.signal(Signal::SIGKILL)
    }
    pub(super) async fn kill_for_close(
        &self,
        child: &mut Child,
        deadline: Instant,
    ) -> io::Result<()> {
        kill_with_retry(
            deadline,
            || {
                #[cfg(test)]
                if let (Some(pgid), Some(error)) = (self.pgid, self.kill_error) {
                    return signal_result(Signal::SIGKILL, pgid, Err(error));
                }
                self.kill()
            },
            || child.try_wait().map(|_| ()),
            cfg!(target_os = "macos"),
        )
        .await
    }
    pub(super) async fn wait_empty(&self) -> io::Result<()> {
        // Descendants are reaped by their own parents/init, not this process.
        // Preserve Unix group-kill semantics; killpg(0) also counts zombies and
        // cannot establish when a non-child was reaped.
        Ok(())
    }
    pub(super) fn disarm(&mut self) {
        self.pgid = None;
    }
    #[cfg(test)]
    pub(super) fn fail_kill(&mut self, error: Option<Errno>) {
        self.kill_error = error;
    }
}

#[cfg(test)]
#[path = "unix_tests.rs"]
mod tests;

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
