//! Real ConPTY in a Job-owned helper, with a separate observer for crash tests.
use super::*;
use std::{
    ffi::OsStr,
    io::Write,
    mem::size_of,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    ptr::{null, null_mut},
    sync::{Arc, Mutex, mpsc},
};
use windows_sys::Win32::{
    Foundation::{HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0},
    System::{Console::*, Diagnostics::ToolHelp::*, Pipes::CreatePipe, Threading::*},
};

const HELPER: &str = "win32::experiments::conpty::helper_worker";
const PAYLOAD: &str = "win32::experiments::conpty::console_payload_worker";
const LEAF: &str = "win32::experiments::conpty::leaf_worker";
const SUPERVISOR: &str = "win32::experiments::conpty::supervisor_worker";

struct Console(HPCON);
impl Console {
    fn open() -> (Self, File, File) {
        let (input_read, input_write) = pipe();
        let (output_read, output_write) = pipe();
        let mut console = 0;
        // SAFETY: live synchronous pipe handles, initialized output; no borrowed
        // pointer survives the call. Flags zero avoids cursor-inheritance handshake.
        let result = unsafe {
            CreatePseudoConsole(
                COORD { X: 80, Y: 24 },
                input_read.as_raw_handle(),
                output_write.as_raw_handle(),
                0,
                &mut console,
            )
        };
        assert_eq!(result, 0, "CreatePseudoConsole HRESULT={result:#x}");
        (Self(console), input_write, output_read)
    }
    fn resize(&self) {
        // SAFETY: the helper exclusively owns a live HPCON until explicit drop.
        let result = unsafe { ResizePseudoConsole(self.0, COORD { X: 103, Y: 37 }) };
        assert_eq!(result, 0);
    }
    fn spawn(&self, executable: &OsStr, arguments: &str, cwd: &Path) -> (OwnedHandle, u32) {
        let mut bytes = 0;
        // SAFETY: documented sizing call, null list and initialized length output.
        unsafe { InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut bytes) };
        assert!(bytes > 0 && bytes <= 64 * 1024);
        let mut storage = vec![0usize; bytes.div_ceil(size_of::<usize>())];
        let attributes = storage.as_mut_ptr().cast();
        // SAFETY: aligned allocation of at least the returned size, kept alive
        // through DeleteProcThreadAttributeList below.
        let initialized =
            unsafe { InitializeProcThreadAttributeList(attributes, 1, 0, &mut bytes) };
        assert_ne!(initialized, 0);
        struct Attributes(LPPROC_THREAD_ATTRIBUTE_LIST);
        impl Drop for Attributes {
            fn drop(&mut self) {
                // SAFETY: initialized list, storage outlives this guard.
                unsafe { DeleteProcThreadAttributeList(self.0) };
            }
        }
        let attributes = Attributes(attributes);
        // SAFETY: PSEUDOCONSOLE takes the HPCON value, not its address; both the
        // console and list outlive CreateProcessW.
        let updated = unsafe {
            UpdateProcThreadAttribute(
                attributes.0,
                0,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
                self.0 as *const _,
                size_of::<HPCON>(),
                null_mut(),
                null(),
            )
        };
        assert_ne!(updated, 0);
        let mut startup = STARTUPINFOEXW::default();
        startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
        startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        startup.StartupInfo.hStdInput = INVALID_HANDLE_VALUE;
        startup.StartupInfo.hStdOutput = INVALID_HANDLE_VALUE;
        startup.StartupInfo.hStdError = INVALID_HANDLE_VALUE;
        startup.lpAttributeList = attributes.0;
        let executable = wide(executable).unwrap();
        // Only the quoted executable and test-owned literal arguments are used.
        // There is no user-controlled command-line interpolation in this fixture.
        let executable_text = String::from_utf16(&executable[..executable.len() - 1]).unwrap();
        assert!(!executable_text.contains('"'));
        let mut command = wide(OsStr::new(&format!("\"{executable_text}\" {arguments}"))).unwrap();
        let cwd = wide(cwd.as_os_str()).unwrap();
        let mut process = PROCESS_INFORMATION::default();
        // SAFETY: all strings/list/structs live through the call. No handles are
        // inherited; the child inherits Job membership from this already-owned helper.
        let created = unsafe {
            CreateProcessW(
                executable.as_ptr(),
                command.as_mut_ptr(),
                null(),
                null(),
                0,
                EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT,
                null(),
                cwd.as_ptr(),
                &startup.StartupInfo,
                &mut process,
            )
        };
        assert_ne!(created, 0, "CreateProcessW: {}", io::Error::last_os_error());
        // SAFETY: successful creation transfers each handle exactly once.
        let thread = unsafe { OwnedHandle::from_raw_handle(process.hThread) };
        // SAFETY: unique process handle from successful CreateProcessW.
        let child = unsafe { OwnedHandle::from_raw_handle(process.hProcess) };
        drop(thread);
        (child, process.dwProcessId)
    }
}
impl Drop for Console {
    fn drop(&mut self) {
        // SAFETY: HPCON is uniquely owned and only closed here. This potentially
        // blocking call runs only in the disposable, Job-contained helper.
        unsafe { ClosePseudoConsole(self.0) };
    }
}
fn pipe() -> (File, File) {
    let (mut read, mut write): (HANDLE, HANDLE) = (null_mut(), null_mut());
    // SAFETY: initialized outputs; null attributes make both handles non-inheritable.
    let created = unsafe { CreatePipe(&mut read, &mut write, null(), 4096) };
    assert_ne!(created, 0);
    // SAFETY: two distinct successful pipe handles, each transferred once to File.
    unsafe { (File::from_raw_handle(read), File::from_raw_handle(write)) }
}

fn exited(process: &OwnedHandle) -> bool {
    // SAFETY: the held process handle has SYNCHRONIZE access; zero is nonblocking.
    unsafe { WaitForSingleObject(process.as_raw_handle(), 0) == WAIT_OBJECT_0 }
}
fn process_handle(pid: u32) -> OwnedHandle {
    // SAFETY: query/synchronize only, no inheritance; the returned handle pins identity.
    let raw = unsafe {
        OpenProcess(
            PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
            0,
            pid,
        )
    };
    assert!(
        !raw.is_null(),
        "OpenProcess({pid}): {}",
        io::Error::last_os_error()
    );
    // SAFETY: successful OpenProcess transfers a unique handle.
    unsafe { OwnedHandle::from_raw_handle(raw) }
}

fn descendants(root: u32) -> Vec<(u32, String)> {
    // SAFETY: snapshot call has no pointer arguments; successful snapshot is owned.
    let raw = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    assert_ne!(raw, INVALID_HANDLE_VALUE);
    // SAFETY: successful snapshot owns one kernel handle.
    let snapshot = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut all = Vec::new();
    // SAFETY: live snapshot and correctly sized writable output.
    let mut found = unsafe { Process32FirstW(snapshot.as_raw_handle(), &mut entry) };
    while found != 0 {
        assert!(all.len() < 65_536);
        let end = entry.szExeFile.iter().position(|c| *c == 0).unwrap();
        all.push((
            entry.th32ProcessID,
            entry.th32ParentProcessID,
            String::from_utf16_lossy(&entry.szExeFile[..end]),
        ));
        // SAFETY: same live snapshot and output as first call.
        found = unsafe { Process32NextW(snapshot.as_raw_handle(), &mut entry) };
    }
    let mut pids = vec![root];
    let mut result = Vec::new();
    loop {
        let before = pids.len();
        for (pid, parent, name) in &all {
            if pids.contains(parent) && !pids.contains(pid) {
                pids.push(*pid);
                result.push((*pid, name.clone()));
            }
        }
        if before == pids.len() {
            return result;
        }
    }
}

#[test]
#[ignore = "internal subprocess fixture"]
fn leaf_worker() {
    let Some(dir) = fixture_dir() else { return };
    mark(&dir, "leaf-ready", std::process::id().to_string());
    until(|| dir.join("release-leaf").exists(), "leaf safety timeout");
}

#[test]
#[ignore = "internal console subprocess fixture"]
#[allow(
    clippy::zombie_processes,
    reason = "the root-exit experiment deliberately leaves a live descendant in the observer-owned Job"
)]
fn console_payload_worker() {
    let Some(dir) = fixture_dir() else { return };
    let case = std::env::var("LATTE_CONPTY_CASE").unwrap();
    let mut child = fixture_command(LEAF, &dir).spawn().unwrap();
    mark(&dir, "payload-ready", std::process::id().to_string());
    if case == "root-exit" {
        until(
            || dir.join("release-root").exists(),
            "root exit command timeout",
        );
        return;
    }
    if case == "blocked-output" {
        until(|| dir.join("start-flood").exists(), "flood command timeout");
        mark(&dir, "output-start", b"");
        let buffer = vec![b'x'; 64 * 1024];
        for index in 0..512 {
            if io::stdout().write_all(&buffer).is_err() {
                return;
            }
            mark(&dir, "output-progress", index.to_string());
        }
        mark(&dir, "output-done", b"");
    }
    until(
        || dir.join("payload-stop").exists(),
        "payload safety timeout",
    );
    mark(&dir, "release-leaf", b"");
    child.wait().unwrap();
}

#[test]
#[ignore = "internal ConPTY helper fixture"]
fn helper_worker() {
    let Some(dir) = fixture_dir() else { return };
    let case = std::env::var("LATTE_CONPTY_CASE").unwrap();
    let (console, mut input, mut output) = Console::open();
    console.resize();
    let collected = Arc::new(Mutex::new(Vec::new()));
    let (read_done, drained) = mpsc::channel();
    {
        let collected = collected.clone();
        let dir = dir.clone();
        thread::spawn(move || {
            let mut buffer = [0; 8192];
            loop {
                if dir.join("pause-output").exists() {
                    mark(&dir, "reader-paused", b"");
                    until(
                        || dir.join("resume-output").exists(),
                        "paused reader safety timeout",
                    );
                }
                match output.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let mut bytes = collected.lock().unwrap();
                        let room = (64 * 1024usize).saturating_sub(bytes.len());
                        bytes.extend_from_slice(&buffer[..n.min(room)]);
                    }
                }
            }
            let _ = read_done.send(());
        });
    }
    let (_child, pid) = if case == "pwsh" {
        let shell = std::env::var_os("LATTE_EXPERIMENT_PWSH").expect("pwsh path");
        console.spawn(&shell, "-NoLogo -NoProfile -NoExit", &dir)
    } else {
        let exe = std::env::current_exe().unwrap();
        console.spawn(
            exe.as_os_str(),
            &format!("--exact {PAYLOAD} --ignored --nocapture"),
            &dir,
        )
    };
    mark(&dir, "console-pid", pid.to_string());
    let (write_done, written) = mpsc::channel();
    let mut retained_input = None;
    if case == "blocked-input" {
        let dir = dir.clone();
        thread::spawn(move || {
            mark(&dir, "input-start", b"");
            let result = input.write_all(&vec![b'x'; 32 * 1024 * 1024]);
            mark(&dir, "input-done", format!("{result:?}"));
            let _ = write_done.send(());
        });
    } else {
        if case == "pwsh" {
            input.write_all("Write-Output ('LATTE_'+'中文🙂'); [Console]::WriteLine('SIZE_'+$Host.UI.RawUI.WindowSize.Width+'x'+$Host.UI.RawUI.WindowSize.Height); [Console]::WriteLine('CWD_'+(Get-Location).Path); [Console]::WriteLine('CP_'+[Console]::OutputEncoding.CodePage); [Console]::WriteLine('DIRECT_'+'中文🙂'); [Console]::OutputEncoding=[Text.UTF8Encoding]::new($false); [Console]::WriteLine('UTF8_'+'中文🙂'); exit 7\r".as_bytes()).unwrap();
        }
        retained_input = Some(input);
        let _ = write_done.send(());
    }
    mark(&dir, "helper-ready", b"");
    until(|| dir.join("close").exists(), "helper command timeout");
    let started = Instant::now();
    drop(retained_input);
    drop(console);
    mark(&dir, "close-ms", started.elapsed().as_millis().to_string());
    drained
        .recv_timeout(Duration::from_secs(3))
        .expect("ConPTY reader did not finish");
    written
        .recv_timeout(Duration::from_secs(3))
        .expect("ConPTY writer did not finish");
    mark(&dir, "output", &*collected.lock().unwrap());
    mark(&dir, "helper-done", b"");
}

fn launch(dir: &Path, case: &str) -> Managed {
    let mut command = fixture_command(HELPER, dir);
    command
        .env("LATTE_CONPTY_CASE", case)
        .stdout(Stdio::from(
            File::create(dir.join("helper.stdout")).unwrap(),
        ))
        .stderr(Stdio::from(
            File::create(dir.join("helper.stderr")).unwrap(),
        ));
    if case == "pwsh" {
        let shell = pwsh("(Get-Command pwsh.exe).Source", dir);
        command.env("LATTE_EXPERIMENT_PWSH", shell);
    }
    Managed::spawn(&mut command).unwrap()
}
fn ready(dir: &Path) {
    until(|| dir.join("helper-ready").exists(), "helper not ready");
}
fn observe(root: u32) -> Vec<(String, OwnedHandle)> {
    let children = descendants(root);
    assert!(
        children
            .iter()
            .any(|(_, name)| name.eq_ignore_ascii_case("conhost.exe")),
        "no conhost in {children:?}"
    );
    println!("observed descendants: {children:?}");
    children
        .into_iter()
        .map(|(pid, name)| (format!("{name}:{pid}"), process_handle(pid)))
        .collect()
}
fn assert_gone(observed: &[(String, OwnedHandle)]) {
    for (name, handle) in observed {
        until(|| exited(handle), &format!("still alive: {name}"));
    }
}

#[test]
#[ignore = "opt-in Windows 11 native experiment"]
fn pwsh_unicode_cwd_resize_and_graceful_close() {
    require_ordinary_user();
    let parent = tempfile::tempdir().unwrap();
    let dir = parent.path().join("中文 项目");
    fs::create_dir(&dir).unwrap();
    let mut helper = launch(&dir, "pwsh");
    ready(&dir);
    let pid: u32 = fs::read_to_string(dir.join("console-pid"))
        .unwrap()
        .parse()
        .unwrap();
    let shell = process_handle(pid);
    until(|| exited(&shell), "pwsh did not exit");
    mark(&dir, "close", b"");
    assert!(helper.wait().success());
    until(|| helper.job.is_empty().unwrap(), "graceful Job not empty");
    let output = fs::read_to_string(dir.join("output")).unwrap();
    assert!(output.contains("LATTE_中文🙂"), "{output:?}");
    assert!(output.contains("SIZE_103x37"), "{output:?}");
    assert!(
        output.contains(&format!("CWD_{}", dir.display())),
        "{output:?}"
    );
    assert!(output.contains("UTF8_中文🙂"), "{output:?}");
    let encoding = output
        .split("\r\nCP_")
        .nth(1)
        .unwrap()
        .lines()
        .next()
        .unwrap();
    println!(
        "initial .NET encoding={encoding}; default direct Console.WriteLine emoji intact={}",
        output.contains("DIRECT_中文🙂")
    );
    let mut code = 0;
    // SAFETY: held process handle is live with query rights; initialized DWORD output.
    let queried = unsafe { GetExitCodeProcess(shell.as_raw_handle(), &mut code) };
    assert_ne!(queried, 0);
    assert_eq!(code, 7);
    println!(
        "pwsh UTF-8/cwd/103x37/exit=7; ClosePseudoConsole={}ms",
        fs::read_to_string(dir.join("close-ms")).unwrap()
    );
}

#[test]
#[ignore = "opt-in Windows 11 native experiment"]
fn blocked_pipes_and_root_exit_have_bounded_job_cleanup() {
    require_ordinary_user();
    for (case, resume_output) in [
        ("idle", false),
        ("blocked-input", false),
        ("blocked-output", false),
        ("blocked-output", true),
        ("root-exit", false),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let mut helper = launch(dir.path(), case);
        ready(dir.path());
        until(|| dir.path().join("leaf-ready").exists(), "leaf missing");
        if case == "blocked-input" {
            until(|| dir.path().join("input-start").exists(), "writer missing");
            thread::sleep(Duration::from_millis(300));
            assert!(
                !dir.path().join("input-done").exists(),
                "input did not block"
            );
        }
        if case == "blocked-output" {
            mark(dir.path(), "pause-output", b"");
            mark(dir.path(), "start-flood", b"");
            until(
                || {
                    dir.path().join("output-start").exists()
                        && dir.path().join("reader-paused").exists()
                },
                "flood did not reach paused reader",
            );
            thread::sleep(Duration::from_millis(300));
            assert!(
                !dir.path().join("output-done").exists(),
                "output did not block"
            );
        }
        let observed = observe(helper.child.id());
        if case == "root-exit" {
            let pid: u32 = fs::read_to_string(dir.path().join("console-pid"))
                .unwrap()
                .parse()
                .unwrap();
            let root = process_handle(pid);
            mark(dir.path(), "release-root", b"");
            until(|| exited(&root), "root did not exit");
            assert!(
                !helper.job.is_empty().unwrap(),
                "descendants must still need cleanup"
            );
        }
        mark(dir.path(), "close", b"");
        if resume_output {
            mark(dir.path(), "resume-output", b"");
        }
        let started = Instant::now();
        let graceful_until = started + Duration::from_millis(750);
        while Instant::now() < graceful_until && !dir.path().join("helper-done").exists() {
            thread::sleep(Duration::from_millis(10));
        }
        let graceful = dir.path().join("helper-done").exists();
        if resume_output {
            assert!(graceful, "resumed reader did not finish closing");
        }
        helper.stop();
        assert_gone(&observed);
        assert!(started.elapsed() < Duration::from_secs(3));
        println!(
            "{case} resume_output={resume_output}: helper_completed={graceful}, cleanup={}ms, close_ms={:?}",
            started.elapsed().as_millis(),
            fs::read_to_string(dir.path().join("close-ms")).ok()
        );
    }
}

fn handle_count() -> u32 {
    let mut count = 0;
    // SAFETY: current-process pseudo-handle is borrowed, never closed; count is writable.
    let queried = unsafe { GetProcessHandleCount(GetCurrentProcess(), &mut count) };
    assert_ne!(queried, 0);
    count
}

#[test]
#[ignore = "opt-in Windows 11 native experiment"]
fn repeated_helpers_release_processes_and_observer_handles() {
    require_ordinary_user();
    let mut counts = Vec::new();
    for _ in 0..9 {
        {
            let dir = tempfile::tempdir().unwrap();
            let mut helper = launch(dir.path(), "idle");
            ready(dir.path());
            until(|| dir.path().join("leaf-ready").exists(), "leaf missing");
            let observed = observe(helper.child.id());
            mark(dir.path(), "close", b"");
            assert!(helper.wait().success());
            helper.stop();
            assert_gone(&observed);
        }
        counts.push(handle_count());
    }
    // First cycle warms the test harness. Repeated runs must not grow handles.
    let low = *counts[1..].iter().min().unwrap();
    let high = *counts[1..].iter().max().unwrap();
    assert!(high - low <= 2, "observer handle growth: {counts:?}");
    println!("handles after nine fully reaped cycles: {counts:?}");
}

#[test]
#[ignore = "internal supervisor fixture"]
fn supervisor_worker() {
    let Some(dir) = fixture_dir() else { return };
    let helper = launch(&dir, "blocked-input");
    ready(&dir);
    mark(&dir, "supervisor-ready", helper.child.id().to_string());
    until(
        || dir.join("supervisor-stop").exists(),
        "supervisor safety timeout",
    );
}

#[test]
#[ignore = "opt-in Windows 11 native experiment"]
fn supervisor_crash_releases_helper_shell_descendants_and_conhost() {
    require_ordinary_user();
    let dir = tempfile::tempdir().unwrap();
    // Outer Job is test cleanup insurance only. Kill *just* the supervisor to
    // verify its private inner Job closes; the outer Job remains live throughout.
    let mut supervisor = Managed::spawn(&mut fixture_command(SUPERVISOR, dir.path())).unwrap();
    until(
        || dir.path().join("supervisor-ready").exists() && dir.path().join("leaf-ready").exists(),
        "supervisor tree missing",
    );
    let observed = observe(supervisor.child.id());
    let started = Instant::now();
    supervisor.child.kill().unwrap();
    supervisor.wait();
    assert_gone(&observed);
    assert!(supervisor.job.is_empty().unwrap());
    assert!(started.elapsed() < Duration::from_secs(3));
    println!(
        "supervisor crash: all observed processes exited in {}ms",
        started.elapsed().as_millis()
    );
}
