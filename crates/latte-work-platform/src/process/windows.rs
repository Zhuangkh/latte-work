use latte_work_windows_security::ProcessJob;
use std::{io, time::Duration};
use tokio::process::{Child, Command};
use windows_sys::Win32::System::Threading::{
    CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW, CREATE_SUSPENDED, DETACHED_PROCESS,
};

#[derive(Debug)]
pub(super) struct Tree(ProcessJob);

pub(super) fn spawn(command: &mut Command) -> io::Result<(Child, Tree)> {
    let program = std::path::Path::new(command.as_std().get_program());
    if program
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            ["cmd", "bat", "ps1"]
                .iter()
                .any(|script| ext.eq_ignore_ascii_case(script))
        })
    {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "managed Windows processes require a native executable, not a shell script",
        ));
    }
    let job = ProcessJob::new()?;
    command
        .creation_flags(CREATE_SUSPENDED | CREATE_NO_WINDOW)
        .kill_on_drop(true);
    let mut child = command.spawn()?;
    let result = child
        .id()
        .ok_or_else(|| io::Error::other("child PID missing"))
        .and_then(|pid| job.assign_and_resume(pid));
    if let Err(error) = result {
        // Still suspended if assignment failed; assigned children are also covered
        // by kill-on-job-close. Tokio's kill-on-drop reaper owns the root handle.
        let _ = child.start_kill();
        return Err(error);
    }
    Ok((child, Tree(job)))
}
impl Tree {
    pub(super) fn request_termination(&self) -> io::Result<bool> {
        Ok(false)
    }
    pub(super) fn kill(&self) -> io::Result<()> {
        self.0.terminate()
    }
    pub(super) async fn kill_for_close(
        &self,
        _child: &mut Child,
        _deadline: tokio::time::Instant,
    ) -> io::Result<()> {
        self.kill()
    }
    pub(super) async fn wait_empty(&self) -> io::Result<()> {
        while !self.0.is_empty()? {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        Ok(())
    }
    pub(super) fn disarm(&mut self) {}
}

pub(super) fn spawn_daemon(command: &mut Command) -> io::Result<Child> {
    // No kill-on-close Job and no bridge console. Do not silently break out of
    // a caller's external Job: incompatible supervisor policy must remain visible.
    command
        .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
        .spawn()
}

/// Register once, before daemon initialization. Detached daemons normally shut
/// down through authenticated IPC; foreground console Ctrl+C/Break/Close also notify.
pub struct Shutdown {
    interrupt: tokio::signal::windows::CtrlC,
    terminate: tokio::signal::windows::CtrlBreak,
    close: tokio::signal::windows::CtrlClose,
}
impl Shutdown {
    pub fn new() -> io::Result<Self> {
        Ok(Self {
            interrupt: tokio::signal::windows::ctrl_c()?,
            terminate: tokio::signal::windows::ctrl_break()?,
            close: tokio::signal::windows::ctrl_close()?,
        })
    }
    pub async fn recv(&mut self) {
        tokio::select! { _ = self.interrupt.recv() => {}, _ = self.terminate.recv() => {}, _ = self.close.recv() => {} }
    }
}
