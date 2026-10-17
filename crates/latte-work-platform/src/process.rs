//! Owned process trees, independent daemon launch, and host shutdown notifications.
use std::{
    io,
    process::{ExitStatus, Stdio},
    time::Duration,
};
use tokio::{
    io::AsyncReadExt,
    process::{Child, ChildStderr, ChildStdin, ChildStdout, Command},
};

#[cfg(unix)]
#[path = "process/unix.rs"]
mod backend;
#[cfg(windows)]
#[path = "process/windows.rs"]
mod backend;

pub use backend::Shutdown;

/// Owns a child and its cleanup boundary, including descendants after root exit.
/// Dropping or cancelling its owner force-stops the tree; explicit close also
/// waits for cleanup and reports errors. Never use this for the host daemon.
#[derive(Debug)]
pub struct ManagedChild {
    child: Child,
    tree: backend::Tree,
    closed: bool,
    pub stdin: Option<ChildStdin>,
    pub stdout: Option<ChildStdout>,
    pub stderr: Option<ChildStderr>,
}

impl ManagedChild {
    /// Platform setup overrides process-group/creation flags and kill-on-drop.
    /// All other command arguments, environment and stdio settings are preserved.
    pub fn spawn(command: &mut Command) -> io::Result<Self> {
        let (mut child, tree) = backend::spawn(command)?;
        Ok(Self {
            stdin: child.stdin.take(),
            stdout: child.stdout.take(),
            stderr: child.stderr.take(),
            child,
            tree,
            closed: false,
        })
    }

    /// Wait for the root only. Descendants remain owned until close/drop.
    pub async fn wait(&mut self) -> io::Result<ExitStatus> {
        self.child.wait().await
    }

    /// Unix sends TERM for `grace`, then KILL; Windows terminates the whole Job.
    /// Cleanup has a separate three-second deadline, and is safe to cancel/retry.
    /// Success on Windows requires kernel accounting to report zero active processes.
    pub async fn close(&mut self, grace: Duration) -> io::Result<()> {
        if self.closed {
            return Ok(());
        }
        self.stdin.take();
        if !grace.is_zero() && self.tree.request_termination()? {
            let _ = tokio::time::timeout(grace, self.child.wait()).await;
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        self.tree.kill_for_close(&mut self.child, deadline).await?;
        tokio::time::timeout_at(deadline, async {
            self.child.wait().await?;
            self.tree.wait_empty().await
        })
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "process tree cleanup timed out"))??;
        self.tree.disarm();
        self.closed = true;
        Ok(())
    }
}
impl Drop for ManagedChild {
    fn drop(&mut self) {
        if !self.closed {
            let _ = self.tree.kill();
            let _ = self.child.start_kill();
        }
    }
}

/// Start an independent daemon with null stdio. Dropping the returned root
/// handle does not terminate it. External service-manager/job policy still applies.
pub fn spawn_daemon(command: &mut Command) -> io::Result<Child> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(false);
    backend::spawn_daemon(command)
}

/// Bounded metadata output with the same tree supervision as agent sessions.
/// Stderr is discarded; successful root exit still cleans remaining descendants.
pub async fn output(
    command: &mut Command,
    timeout: Duration,
    limit: usize,
) -> io::Result<(ExitStatus, Vec<u8>)> {
    let read_limit = u64::try_from(limit)
        .ok()
        .and_then(|n| n.checked_add(1))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid output limit"))?;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = ManagedChild::spawn(command)?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("child stdout missing"))?
        .take(read_limit);
    let result = tokio::time::timeout(timeout, async {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).await?;
        if bytes.len() > limit {
            return Err(io::Error::other("child output exceeds limit"));
        }
        Ok((child.wait().await?, bytes))
    })
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "process output timed out"))
    .and_then(|result| result);
    child.close(Duration::ZERO).await?;
    result
}

#[cfg(test)]
mod tests;
