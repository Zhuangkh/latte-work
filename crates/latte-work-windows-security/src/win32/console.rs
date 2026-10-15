//! Owned ConPTY primitives. Call only inside a Job-contained terminal helper:
//! synchronous pipes and ClosePseudoConsole can block; the supervisor owns deadlines.
use super::*;
use std::ffi::OsString;
use windows_sys::Win32::{
    Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT},
    System::{Console::*, Pipes::CreatePipe, Threading::*},
};

/// Unique pseudo console. Drop closes it; keep draining its output during close.
pub struct PseudoConsole(HPCON);
/// Owned root process handle; the surrounding helper's Job owns all descendants.
pub struct ConsoleChild(OwnedHandle);

fn dimensions(cols: u16, rows: u16) -> io::Result<COORD> {
    if !(2..=500).contains(&cols) || !(1..=300).contains(&rows) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid console size",
        ));
    }
    Ok(COORD {
        X: cols as i16,
        Y: rows as i16,
    })
}
fn pipe() -> io::Result<(File, File)> {
    let (mut read, mut write) = (null_mut(), null_mut());
    // SAFETY: initialized outputs; null attributes create non-inheritable handles.
    if unsafe { CreatePipe(&mut read, &mut write, null(), 4096) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful call transfers two distinct owned handles exactly once.
    Ok(unsafe { (File::from_raw_handle(read), File::from_raw_handle(write)) })
}
fn hresult(result: i32) -> io::Result<()> {
    if result < 0 {
        Err(io::Error::other(format!("ConPTY HRESULT {result:#x}")))
    } else {
        Ok(())
    }
}
impl PseudoConsole {
    /// Returns console, input writer and output reader. Start draining before spawn.
    pub fn open(cols: u16, rows: u16) -> io::Result<(Self, File, File)> {
        let size = dimensions(cols, rows)?;
        let (input_read, input_write) = pipe()?;
        let (output_read, output_write) = pipe()?;
        let mut console = 0;
        // SAFETY: live synchronous pipes, initialized HPCON output; flags zero
        // avoids the cursor inheritance handshake. ConPTY retains its endpoints.
        hresult(unsafe {
            CreatePseudoConsole(
                size,
                input_read.as_raw_handle(),
                output_write.as_raw_handle(),
                0,
                &mut console,
            )
        })?;
        Ok((Self(console), input_write, output_read))
    }
    pub fn resize(&mut self, cols: u16, rows: u16) -> io::Result<()> {
        let size = dimensions(cols, rows)?;
        // SAFETY: uniquely owned live HPCON, size validated above.
        hresult(unsafe { ResizePseudoConsole(self.0, size) })
    }
    /// Launch a native absolute executable, inheriting the helper's environment/Job.
    /// This API does not create a Job: its caller must already be supervised.
    pub fn spawn(
        &mut self,
        executable: &Path,
        args: &[OsString],
        cwd: &Path,
    ) -> io::Result<ConsoleChild> {
        if !executable.is_absolute() || !cwd.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "console executable/cwd must be absolute",
            ));
        }
        let exe = wide(executable.as_os_str())?;
        let cwd = wide(cwd.as_os_str())?;
        let mut command = command_line(executable.as_os_str(), args)?;
        let mut bytes = 0;
        // SAFETY: documented sizing call with initialized size output.
        unsafe { InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut bytes) };
        if bytes == 0 || bytes > 64 * 1024 {
            return Err(io::Error::other("invalid attribute list size"));
        }
        let mut storage = vec![0usize; bytes.div_ceil(size_of::<usize>())];
        let list = storage.as_mut_ptr().cast();
        // SAFETY: aligned buffer of sufficient size, kept until the list is deleted.
        if unsafe { InitializeProcThreadAttributeList(list, 1, 0, &mut bytes) } == 0 {
            return Err(io::Error::last_os_error());
        }
        struct Attributes(LPPROC_THREAD_ATTRIBUTE_LIST);
        impl Drop for Attributes {
            fn drop(&mut self) {
                // SAFETY: initialized list; its backing storage outlives this guard.
                unsafe { DeleteProcThreadAttributeList(self.0) };
            }
        }
        let list = Attributes(list);
        // SAFETY: pseudoconsole attribute takes the HPCON value, not its address.
        // Both console and attribute storage remain live through CreateProcessW.
        if unsafe {
            UpdateProcThreadAttribute(
                list.0,
                0,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
                self.0 as *const _,
                size_of::<HPCON>(),
                null_mut(),
                null(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        let mut startup = STARTUPINFOEXW::default();
        startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
        startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        startup.StartupInfo.hStdInput = INVALID_HANDLE_VALUE;
        startup.StartupInfo.hStdOutput = INVALID_HANDLE_VALUE;
        startup.StartupInfo.hStdError = INVALID_HANDLE_VALUE;
        startup.lpAttributeList = list.0;
        let mut process = PROCESS_INFORMATION::default();
        // SAFETY: helper-only process setting. A detached daemon may inherit an
        // ignored Ctrl+C flag; clear it before spawning the interactive child.
        // Custom handlers are not inherited, but this ignore flag is.
        if unsafe { SetConsoleCtrlHandler(None, 0) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: valid live input buffers; command line is writable, environment
        // inherited, no stdio/Job handles inherited. OS inherits helper Job membership.
        if unsafe {
            CreateProcessW(
                exe.as_ptr(),
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
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: successful call returns two uniquely owned handles.
        let thread = unsafe { OwnedHandle::from_raw_handle(process.hThread) };
        // SAFETY: process handle transferred once; thread is independently owned.
        let child = unsafe { OwnedHandle::from_raw_handle(process.hProcess) };
        drop(thread);
        Ok(ConsoleChild(child))
    }
}
impl Drop for PseudoConsole {
    fn drop(&mut self) {
        // SAFETY: unique HPCON closed exactly once. May block, so helper-only.
        unsafe { ClosePseudoConsole(self.0) };
    }
}
impl ConsoleChild {
    pub fn try_wait(&self) -> io::Result<Option<u32>> {
        // SAFETY: live process handle with SYNCHRONIZE; zero timeout never blocks.
        match unsafe { WaitForSingleObject(self.0.as_raw_handle(), 0) } {
            WAIT_TIMEOUT => Ok(None),
            WAIT_OBJECT_0 => {
                let mut code = 0;
                // SAFETY: live query handle, initialized DWORD output. Waiting first
                // distinguishes real exit code 259 from STILL_ACTIVE.
                if unsafe { GetExitCodeProcess(self.0.as_raw_handle(), &mut code) } == 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(Some(code))
            }
            _ => Err(io::Error::last_os_error()),
        }
    }
}
fn command_line(executable: &OsStr, args: &[OsString]) -> io::Result<Vec<u16>> {
    let mut out = Vec::new();
    for value in std::iter::once(executable).chain(args.iter().map(OsString::as_os_str)) {
        if !out.is_empty() {
            out.push(b' ' as u16);
        }
        out.push(b'"' as u16);
        let mut slashes = 0;
        for unit in value.encode_wide() {
            if out.len() + slashes * 2 + 3 >= 32_767 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "console command line too long",
                ));
            }
            if unit == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "NUL in console argument",
                ));
            }
            if unit == b'\\' as u16 {
                slashes += 1;
                continue;
            }
            out.extend(std::iter::repeat_n(
                b'\\' as u16,
                slashes * if unit == b'"' as u16 { 2 } else { 1 },
            ));
            slashes = 0;
            if unit == b'"' as u16 {
                out.push(b'\\' as u16);
            }
            out.push(unit);
        }
        out.extend(std::iter::repeat_n(b'\\' as u16, slashes * 2));
        out.push(b'"' as u16);
        if out.len() >= 32_767 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "console command line too long",
            ));
        }
    }
    out.push(0);
    Ok(out)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_arguments_quote_spaces_quotes_and_trailing_slashes() {
        let line = command_line(
            OsStr::new(r"C:\Program Files\pwsh.exe"),
            &["".into(), "a\"b".into(), "tail\\".into()],
        )
        .unwrap();
        assert_eq!(
            String::from_utf16(&line[..line.len() - 1]).unwrap(),
            "\"C:\\Program Files\\pwsh.exe\" \"\" \"a\\\"b\" \"tail\\\\\""
        );
        assert!(command_line(OsStr::new("a"), &["\0".into()]).is_err());
        assert!(command_line(OsStr::new("a"), &["\\".repeat(40_000).into()]).is_err());
    }
}
