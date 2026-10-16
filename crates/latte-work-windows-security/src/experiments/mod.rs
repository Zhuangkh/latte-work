//! Opt-in Windows 11 experiments. No code here is compiled into production.
//! Run serially with `scripts/test-windows-terminal-undo-experiments.ps1`.
mod conpty;
mod ntfs;

use super::{ProcessJob, wide};
use std::{
    fs::{self, File},
    io::{self, Read},
    os::windows::process::CommandExt,
    path::Path,
    process::{Child, Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};
use windows_sys::Win32::System::Threading::{CREATE_NO_WINDOW, CREATE_SUSPENDED};

const DEADLINE: Duration = Duration::from_secs(15);

/// Own both the process and Job even on assertion failure. The child never runs
/// before assignment; no fixture can leave unowned descendants on a normal error.
struct Managed {
    child: Child,
    job: ProcessJob,
}
impl Managed {
    fn spawn(command: &mut Command) -> io::Result<Self> {
        let job = ProcessJob::new()?;
        let mut child = command
            .creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED)
            .spawn()?;
        if let Err(error) = job.assign_and_resume(child.id()) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        Ok(Self { child, job })
    }
    fn wait(&mut self) -> ExitStatus {
        let until = Instant::now() + DEADLINE;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            assert!(Instant::now() < until, "fixture process timed out");
            thread::sleep(Duration::from_millis(10));
        }
    }
    fn stop(&mut self) {
        self.job.terminate().unwrap();
        self.wait();
        until(|| self.job.is_empty().unwrap(), "Job did not become empty");
    }
}
impl Drop for Managed {
    fn drop(&mut self) {
        let _ = self.job.terminate();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn until(mut predicate: impl FnMut() -> bool, message: &str) {
    let until = Instant::now() + DEADLINE;
    while !predicate() {
        assert!(Instant::now() < until, "{message}");
        thread::sleep(Duration::from_millis(10));
    }
}

fn fixture_command(test: &str, dir: &Path) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", test, "--ignored", "--nocapture"])
        .env("LATTE_NATIVE_EXPERIMENT_DIR", dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

fn fixture_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("LATTE_NATIVE_EXPERIMENT_DIR").map(Into::into)
}

fn pwsh(script: &str, dir: &Path) -> String {
    let output = tempfile::tempfile().unwrap();
    let error = tempfile::tempfile().unwrap();
    let mut command = Command::new("pwsh.exe");
    command
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            script,
        ])
        .env("LATTE_NATIVE_EXPERIMENT_DIR", dir)
        .stdin(Stdio::null())
        .stdout(Stdio::from(output.try_clone().unwrap()))
        .stderr(Stdio::from(error.try_clone().unwrap()));
    let mut child = Managed::spawn(&mut command).unwrap();
    let status = child.wait();
    let read = |mut file: File| {
        use std::io::{Seek, SeekFrom};
        file.seek(SeekFrom::Start(0)).unwrap();
        let mut bytes = String::new();
        file.take(32 * 1024).read_to_string(&mut bytes).unwrap();
        bytes
    };
    let output = read(output);
    let error = read(error);
    assert!(status.success(), "pwsh failed: {error}\n{output}");
    output.trim().to_owned()
}

fn require_ordinary_user() {
    crate::current_user_sid().expect("Windows process user SID must be readable");
}

fn mark(dir: &Path, name: &str, value: impl AsRef<[u8]>) {
    fs::write(dir.join(name), value).unwrap();
}
