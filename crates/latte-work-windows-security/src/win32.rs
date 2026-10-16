//! The sole FFI module. See ../SAFETY.md for ownership and trust boundaries.
#[path = "win32/console.rs"]
pub(crate) mod console;
#[cfg(test)]
#[path = "experiments/mod.rs"]
mod experiments;
#[path = "win32/project.rs"]
pub(crate) mod project;
#[path = "win32/session.rs"]
pub(crate) mod session;
use std::{
    ffi::{OsStr, c_void},
    fs::File,
    io,
    mem::{offset_of, size_of},
    os::windows::{
        ffi::OsStrExt,
        io::{AsHandle, AsRawHandle, BorrowedHandle, FromRawHandle, OwnedHandle},
    },
    path::Path,
    ptr::{NonNull, null, null_mut},
};
use windows_sys::Win32::{
    Foundation::{ERROR_INSUFFICIENT_BUFFER, HANDLE, INVALID_HANDLE_VALUE, LocalFree},
    Security::{
        ACCESS_ALLOWED_ACE, ACE_HEADER, ACL,
        Authorization::{
            ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
            GetSecurityInfo, SDDL_REVISION_1, SE_FILE_OBJECT,
        },
        CONTAINER_INHERIT_ACE, DACL_SECURITY_INFORMATION, GetAce, GetSecurityDescriptorControl,
        GetTokenInformation, IsValidAcl, IsValidSid, OBJECT_INHERIT_ACE,
        OWNER_SECURITY_INFORMATION, PSID, RevertToSelf, SE_DACL_PRESENT, SE_DACL_PROTECTED,
        SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser,
    },
    Storage::FileSystem::{
        CreateDirectoryW, CreateFileW, FILE_ALL_ACCESS, FILE_ATTRIBUTE_DIRECTORY,
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_ID_INFO, FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES,
        FILE_SHARE_READ, FILE_SHARE_WRITE, FileAttributeTagInfo, FileIdInfo,
        GetFileInformationByHandleEx, OPEN_EXISTING, READ_CONTROL,
    },
    System::{
        Pipes::{
            GetNamedPipeClientProcessId, GetNamedPipeInfo, GetNamedPipeServerProcessId,
            ImpersonateNamedPipeClient, PIPE_SERVER_END,
        },
        SystemServices::ACCESS_ALLOWED_ACE_TYPE,
        Threading::{GetCurrentProcess, GetCurrentThread, OpenProcessToken, OpenThreadToken},
    },
};

/// Owns an unnamed kernel job. No raw handle escapes this module.
#[derive(Debug)]
pub(crate) struct ProcessJob(OwnedHandle);

impl ProcessJob {
    pub(crate) fn new() -> io::Result<Self> {
        use windows_sys::Win32::System::JobObjects::*;
        // SAFETY: null attributes/name create a non-inheritable, unnamed Job.
        let raw = unsafe { CreateJobObjectW(null(), null()) };
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: successful CreateJobObjectW transfers one owned handle.
        let job = Self(unsafe { OwnedHandle::from_raw_handle(raw) });
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: live Job and aligned, initialized input of the documented size.
        if unsafe {
            SetInformationJobObject(
                job.0.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(job)
    }

    pub(crate) fn assign_and_resume(&self, child_id: u32) -> io::Result<()> {
        use windows_sys::Win32::System::{JobObjects::AssignProcessToJobObject, Threading::*};
        // SAFETY: OpenProcess validates the ID/access; no handle is inherited.
        let raw = unsafe {
            OpenProcess(
                PROCESS_SET_QUOTA | PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION,
                0,
                child_id,
            )
        };
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: successful OpenProcess transfers one owned handle, pinning identity.
        let child = unsafe { OwnedHandle::from_raw_handle(raw) };
        // SAFETY: both handles are live and have the required rights. Assignment
        // fails explicitly for incompatible inherited jobs; never resume on failure.
        if unsafe { AssignProcessToJobObject(self.0.as_raw_handle(), child.as_raw_handle()) } == 0 {
            return Err(io::Error::last_os_error());
        }
        resume_initial_threads(child_id)
    }

    pub(crate) fn terminate(&self) -> io::Result<()> {
        use windows_sys::Win32::System::JobObjects::TerminateJobObject;
        // SAFETY: the owned Job remains live through the call; no borrowed memory.
        if unsafe { TerminateJobObject(self.0.as_raw_handle(), 1) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    pub(crate) fn is_empty(&self) -> io::Result<bool> {
        use windows_sys::Win32::System::JobObjects::*;
        let mut info = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
        // SAFETY: live Job, aligned output and exact documented structure size.
        if unsafe {
            QueryInformationJobObject(
                self.0.as_raw_handle(),
                JobObjectBasicAccountingInformation,
                (&mut info as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(info.ActiveProcesses == 0)
    }
}

fn resume_initial_threads(child_id: u32) -> io::Result<()> {
    use windows_sys::Win32::{
        Foundation::ERROR_NO_MORE_FILES,
        System::{Diagnostics::ToolHelp::*, Threading::*},
    };
    // SAFETY: snapshot flags have no pointer arguments; the snapshot is immutable.
    let raw = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if raw == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful snapshot creation transfers an owned kernel handle.
    let snapshot = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut entry = THREADENTRY32 {
        dwSize: size_of::<THREADENTRY32>() as u32,
        ..Default::default()
    };
    let mut threads = Vec::new();
    // SAFETY: writable, correctly sized THREADENTRY32 and live snapshot handle.
    let mut found = unsafe { Thread32First(snapshot.as_raw_handle(), &mut entry) };
    let mut count = 0;
    while found != 0 {
        count += 1;
        if count > 65_536 {
            return Err(io::Error::other("thread snapshot exceeds limit"));
        }
        if entry.th32OwnerProcessID == child_id {
            // SAFETY: OpenThread validates the snapshot ID; inheritance is disabled.
            let raw = unsafe {
                OpenThread(
                    THREAD_SUSPEND_RESUME | THREAD_QUERY_LIMITED_INFORMATION,
                    0,
                    entry.th32ThreadID,
                )
            };
            if raw.is_null() {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: successful OpenThread transfers a unique live handle.
            let thread = unsafe { OwnedHandle::from_raw_handle(raw) };
            // SAFETY: live thread with query rights. Recheck identity after opening.
            if unsafe { GetProcessIdOfThread(thread.as_raw_handle()) } != child_id {
                return Err(io::Error::other("child thread identity changed"));
            }
            threads.push(thread);
        }
        entry.dwSize = size_of::<THREADENTRY32>() as u32;
        // SAFETY: same initialized output and live snapshot as Thread32First.
        found = unsafe { Thread32Next(snapshot.as_raw_handle(), &mut entry) };
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() != Some(ERROR_NO_MORE_FILES as i32) {
        return Err(error);
    }
    if threads.is_empty() {
        return Err(io::Error::other("suspended child has no threads"));
    }
    for thread in threads {
        // SAFETY: owned thread with suspend/resume access, already assigned to Job.
        let previous = unsafe { ResumeThread(thread.as_raw_handle()) };
        if previous == u32::MAX {
            return Err(io::Error::last_os_error());
        }
        if previous != 1 {
            return Err(io::Error::other("unexpected child suspension count"));
        }
    }
    Ok(())
}

fn denied(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, message)
}

fn wide(value: &OsStr) -> io::Result<Vec<u16>> {
    let mut text: Vec<_> = value.encode_wide().collect();
    if text.contains(&0) || text.len() >= 32_767 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid Windows path",
        ));
    }
    text.push(0);
    Ok(text)
}

/// Owns only successful LocalAlloc-backed API results; never cloned or exported.
struct LocalMemory(NonNull<c_void>);
impl LocalMemory {
    fn new(pointer: *mut c_void) -> io::Result<Self> {
        NonNull::new(pointer)
            .map(Self)
            .ok_or_else(|| io::Error::other("Windows returned no descriptor"))
    }
}
impl Drop for LocalMemory {
    fn drop(&mut self) {
        // SAFETY: all constructors below receive LocalAlloc-owned results. This
        // allocation has one owner and no references survive its destruction.
        unsafe {
            LocalFree(self.0.as_ptr());
        }
    }
}

pub(crate) struct Identity {
    pub(crate) sid: String,
}

fn operation_error(operation: &str, error: io::Error) -> io::Error {
    io::Error::new(error.kind(), format!("{operation}: {error}"))
}

pub(crate) fn current_identity() -> io::Result<Identity> {
    // SAFETY: GetCurrentProcess returns a borrowed pseudo-handle, valid for this call.
    let process = unsafe { GetCurrentProcess() };
    identity_from_process(process)
        .map_err(|error| operation_error("current process identity", error))
}

// Only accepts the current process pseudo-handle or an owned process token below.
fn identity_from_process(process: HANDLE) -> io::Result<Identity> {
    let mut raw = null_mut();
    // SAFETY: process is live, raw is writable; no handle inheritance or mutation requested.
    if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut raw) } == 0 {
        return Err(operation_error(
            "OpenProcessToken(TOKEN_QUERY)",
            io::Error::last_os_error(),
        ));
    }
    // SAFETY: successful OpenProcessToken transfers a unique valid token handle.
    let token = unsafe { OwnedHandle::from_raw_handle(raw) };
    identity_from_token(token.as_raw_handle())
}

fn identity_from_token(token: HANDLE) -> io::Result<Identity> {
    let mut needed = 0;
    // SAFETY: documented size query writes only needed with a null, zero-length buffer.
    let first = unsafe { GetTokenInformation(token, TokenUser, null_mut(), 0, &mut needed) };
    let error = io::Error::last_os_error();
    if first != 0 || error.raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER as i32) {
        return Err(io::Error::other(format!("cannot size TokenUser: {error}")));
    }
    if !(size_of::<TOKEN_USER>()..=16_384).contains(&(needed as usize)) {
        return Err(io::Error::other("invalid TokenUser buffer size"));
    }
    let capacity = needed;
    let mut user = vec![0usize; (capacity as usize).div_ceil(size_of::<usize>())];
    // SAFETY: usize storage provides TOKEN_USER alignment and at least capacity bytes.
    // The allocation stays fixed until the embedded SID has been converted to text.
    if unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            user.as_mut_ptr().cast(),
            capacity,
            &mut needed,
        )
    } == 0
    {
        return Err(operation_error(
            "GetTokenInformation(TokenUser)",
            io::Error::last_os_error(),
        ));
    }
    if needed > capacity || (needed as usize) < size_of::<TOKEN_USER>() {
        return Err(io::Error::other("invalid TokenUser result size"));
    }
    // SAFETY: successful query initialized the aligned TOKEN_USER prefix bounded above.
    let sid = unsafe { (*user.as_ptr().cast::<TOKEN_USER>()).User.Sid };
    let offset = (sid as usize)
        .checked_sub(user.as_ptr() as usize)
        .filter(|offset| *offset <= needed as usize)
        .ok_or_else(|| io::Error::other("TokenUser SID outside buffer"))?;
    // SAFETY: the SID address lies in user storage and available bounds its remainder.
    let sid = unsafe { bounded_sid_text(sid, needed as usize - offset)? };

    Ok(Identity { sid })
}

/// # Safety
/// `sid` is live, readable for `available` bytes and retained for this call.
unsafe fn bounded_sid_text(sid: PSID, available: usize) -> io::Result<String> {
    if sid.is_null() || available < 8 || !(sid as usize).is_multiple_of(4) {
        return Err(denied("invalid SID extent or alignment"));
    }
    // SAFETY: fixed eight-byte SID header is bounded above.
    let count = unsafe { *sid.cast::<u8>().add(1) } as usize;
    if count > 15 || 8 + count * 4 > available {
        return Err(denied("SID exceeds its backing buffer"));
    }
    // SAFETY: complete SID (including subauthorities) fits live storage.
    unsafe { sid_text(sid) }
}

/// # Safety
/// `sid` points to a complete, aligned SID in live token/descriptor storage.
unsafe fn sid_text(sid: PSID) -> io::Result<String> {
    // SAFETY: caller guarantees a complete readable SID; this checks revision and count.
    if sid.is_null() || unsafe { IsValidSid(sid) } == 0 {
        return Err(denied("invalid SID"));
    }
    let mut text = null_mut();
    // SAFETY: valid SID and writable out-pointer; result is a NUL-terminated LocalAlloc string.
    if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let _allocation = LocalMemory::new(text.cast())?;
    let mut units = Vec::new();
    for offset in 0..256 {
        // SAFETY: API returns a terminated SID string; SID textual representations are
        // shorter than 256 UTF-16 units. Iteration stops at the first terminator.
        let unit = unsafe { *text.add(offset) };
        if unit == 0 {
            return String::from_utf16(&units)
                .map_err(|_| io::Error::other("invalid SID encoding"));
        }
        units.push(unit);
    }
    Err(io::Error::other("SID string exceeds limit"))
}

fn private_descriptor(user: &Identity, directory: bool) -> io::Result<LocalMemory> {
    let flags = if directory { "OICI" } else { "" };
    let text = wide(OsStr::new(&format!(
        "O:{}D:P(A;{flags};FA;;;{})",
        user.sid, user.sid
    )))?;
    let mut raw = null_mut();
    // SAFETY: text is terminated and live; API allocates one self-relative descriptor.
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            text.as_ptr(),
            SDDL_REVISION_1,
            &mut raw,
            null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let descriptor = LocalMemory::new(raw)?;
    require_protected(&descriptor)?;
    Ok(descriptor)
}

fn require_protected(descriptor: &LocalMemory) -> io::Result<()> {
    let mut control = 0;
    let mut revision = 0;
    // SAFETY: descriptor came from the SDDL parser or GetSecurityInfo and stays live.
    if unsafe { GetSecurityDescriptorControl(descriptor.0.as_ptr(), &mut control, &mut revision) }
        == 0
    {
        return Err(io::Error::last_os_error());
    }
    if control & (SE_DACL_PRESENT | SE_DACL_PROTECTED) != SE_DACL_PRESENT | SE_DACL_PROTECTED {
        return Err(denied(
            "control directory DACL must be present and protected",
        ));
    }
    Ok(())
}

pub(crate) fn create_directory(path: &Path, user: &Identity) -> io::Result<()> {
    let descriptor = private_descriptor(user, true)?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0.as_ptr(),
        bInheritHandle: 0,
    };
    let path = wide(path.as_os_str())?;
    // SAFETY: path, attributes and descriptor are live for this atomic creation.
    // AlreadyExists never changes an existing object's permissions.
    if unsafe { CreateDirectoryW(path.as_ptr(), &attributes) } == 0 {
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::AlreadyExists {
            return Err(error);
        }
    }
    Ok(())
}

pub(crate) fn create_private_file(path: &Path, user: &Identity) -> io::Result<File> {
    use windows_sys::Win32::{
        Foundation::{GENERIC_READ, GENERIC_WRITE},
        Storage::FileSystem::{CREATE_NEW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_DELETE},
    };
    let descriptor = private_descriptor(user, false)?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0.as_ptr(),
        bInheritHandle: 0,
    };
    let path = wide(path.as_os_str())?;
    // SAFETY: path, explicit owner/DACL and non-inheritable attributes remain live
    // throughout atomic CREATE_NEW. Existing objects are neither opened nor changed.
    let raw = unsafe {
        CreateFileW(
            path.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            &attributes,
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    };
    if raw == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    if raw.is_null() {
        return Err(io::Error::other("CreateFileW returned a null handle"));
    }
    // SAFETY: successful CREATE_NEW transfers this unique handle exactly once.
    let file = unsafe { File::from_raw_handle(raw) };
    verify_private_file(&file, user)?;
    Ok(file)
}

pub(crate) fn open_directory(path: &Path) -> io::Result<File> {
    let path = wide(path.as_os_str())?;
    // SAFETY: terminated path; read-only OPEN_EXISTING cannot create or truncate.
    // OPEN_REPARSE_POINT exposes the leaf itself; listing access makes omission
    // of FILE_SHARE_DELETE effective for pinning the directory.
    let raw = unsafe {
        CreateFileW(
            path.as_ptr(),
            READ_CONTROL | FILE_READ_ATTRIBUTES | FILE_LIST_DIRECTORY,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    };
    if raw == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    if raw.is_null() {
        return Err(io::Error::other("CreateFileW returned a null handle"));
    }
    // SAFETY: successful CreateFileW transferred this valid handle exactly once.
    let file = unsafe { File::from_raw_handle(raw) };
    let mut info = FILE_ATTRIBUTE_TAG_INFO::default();
    // SAFETY: file is live and info is the exact initialized output structure.
    if unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileAttributeTagInfo,
            (&mut info as *mut FILE_ATTRIBUTE_TAG_INFO).cast(),
            size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    if info.FileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0
        || info.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
    {
        return Err(denied(
            "control path must contain directories without reparse points",
        ));
    }
    Ok(file)
}

pub(crate) fn directory_identity(file: &File) -> io::Result<crate::DirectoryIdentity> {
    let mut info = FILE_ID_INFO::default();
    // SAFETY: file is the live pinned directory handle; info is the exact initialized
    // FileIdInfo output structure and no pointer outlives the call.
    if unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileIdInfo,
            (&mut info as *mut FILE_ID_INFO).cast(),
            size_of::<FILE_ID_INFO>() as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(crate::DirectoryIdentity {
        volume: info.VolumeSerialNumber,
        file_id: info.FileId.Identifier,
    })
}

pub(crate) fn verify_directory(file: &File, user: &Identity) -> io::Result<()> {
    verify_storage_acl(file.as_handle(), user, true)
}

pub(crate) fn verify_private_file(file: &File, user: &Identity) -> io::Result<()> {
    let metadata = file.metadata()?;
    use std::os::windows::fs::MetadataExt;
    if !metadata.is_file() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(denied(
            "private file must be regular and not a reparse point",
        ));
    }
    verify_storage_acl(file.as_handle(), user, false)
}

fn verify_storage_acl(
    handle: BorrowedHandle<'_>,
    user: &Identity,
    directory: bool,
) -> io::Result<()> {
    let mut owner = null_mut();
    let mut dacl: *mut ACL = null_mut();
    let mut raw_descriptor = null_mut();
    // SAFETY: live read-control handle; returned component pointers borrow the
    // single LocalAlloc descriptor, immediately owned below. No mutations occur.
    let status = unsafe {
        GetSecurityInfo(
            handle.as_raw_handle(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            &mut dacl,
            null_mut(),
            &mut raw_descriptor,
        )
    };
    if status != 0 {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    let descriptor = LocalMemory::new(raw_descriptor)?;
    if directory {
        require_protected(&descriptor)?;
    }
    if owner.is_null() || dacl.is_null() {
        return Err(denied("private storage needs an owner and non-null DACL"));
    }
    // SAFETY: GetSecurityInfo supplies complete owner SID and ACL in descriptor storage.
    let owner_text = unsafe { sid_text(owner)? };
    // SAFETY: dacl is a non-null component in the live OS-returned descriptor.
    let valid_acl = unsafe { IsValidAcl(dacl) != 0 && (*dacl).AceCount == 1 };
    if owner_text != user.sid || !valid_acl {
        return Err(denied(
            "private storage is not owned exclusively by the current user",
        ));
    }
    let mut raw_ace = null_mut();
    // SAFETY: validated ACL has exactly one ACE; result borrows descriptor storage.
    if unsafe { GetAce(dacl, 0, &mut raw_ace) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: dacl header is valid; its AclSize bounds all ACE data.
    let acl_len = unsafe { (*dacl).AclSize as usize };
    let offset = (raw_ace as usize)
        .checked_sub(dacl as usize)
        .filter(|offset| *offset >= size_of::<ACL>() && *offset <= acl_len)
        .ok_or_else(|| denied("ACE lies outside the DACL"))?;
    if acl_len - offset < size_of::<ACE_HEADER>() {
        return Err(denied("truncated ACE header"));
    }
    // SAFETY: header extent is bounded by ACL storage. Unaligned reads do not assume Rust alignment.
    let header = unsafe { raw_ace.cast::<ACE_HEADER>().read_unaligned() };
    let ace_len = header.AceSize as usize;
    if header.AceType as u32 != ACCESS_ALLOWED_ACE_TYPE
        || (directory && header.AceFlags as u32 != OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE)
        || (!directory
            && !matches!(
                header.AceFlags as u32,
                0 | windows_sys::Win32::Security::INHERITED_ACE
            ))
        || ace_len > acl_len - offset
        || ace_len < offset_of!(ACCESS_ALLOWED_ACE, SidStart) + 8
    {
        return Err(denied(
            "private storage ACE has an unexpected type or inheritance flags",
        ));
    }
    // SAFETY: the type and complete fixed prefix were checked above.
    let ace = unsafe { raw_ace.cast::<ACCESS_ALLOWED_ACE>().read_unaligned() };
    if ace.Mask != FILE_ALL_ACCESS {
        return Err(denied("private storage ACE must grant full control"));
    }
    // SAFETY: validated ACE includes its inline SID header; pointer remains in descriptor storage.
    let sid = unsafe {
        raw_ace
            .cast::<u8>()
            .add(offset_of!(ACCESS_ALLOWED_ACE, SidStart))
            .cast()
    };
    // SAFETY: remaining ACE length bounds the variable-sized SID.
    let granted_sid =
        unsafe { bounded_sid_text(sid, ace_len - offset_of!(ACCESS_ALLOWED_ACE, SidStart))? };
    if granted_sid != user.sid {
        return Err(denied("private storage grants a different user"));
    }
    Ok(())
}

fn pipe_peer_pid(pipe: BorrowedHandle<'_>) -> io::Result<u32> {
    let mut flags = 0;
    // SAFETY: borrowed handle remains valid; GetNamedPipeInfo rejects other handle types.
    if unsafe {
        GetNamedPipeInfo(
            pipe.as_raw_handle(),
            &mut flags,
            null_mut(),
            null_mut(),
            null_mut(),
        )
    } == 0
    {
        return Err(operation_error(
            "GetNamedPipeInfo",
            io::Error::last_os_error(),
        ));
    }
    let mut pid = 0;
    // SAFETY: kernel-reported pipe end selects the matching peer query. pid is writable.
    let result = unsafe {
        if flags & PIPE_SERVER_END != 0 {
            GetNamedPipeClientProcessId(pipe.as_raw_handle(), &mut pid)
        } else {
            GetNamedPipeServerProcessId(pipe.as_raw_handle(), &mut pid)
        }
    };
    if result == 0 {
        let operation = if flags & PIPE_SERVER_END != 0 {
            "GetNamedPipeClientProcessId"
        } else {
            "GetNamedPipeServerProcessId"
        };
        return Err(operation_error(operation, io::Error::last_os_error()));
    }
    if pid == 0 {
        return Err(denied("pipe peer PID unavailable"));
    }
    Ok(pid)
}

struct RevertToSelfGuard;
impl Drop for RevertToSelfGuard {
    fn drop(&mut self) {
        // SAFETY: this guard exists only after ImpersonateNamedPipeClient succeeds.
        let _ = unsafe { RevertToSelf() };
    }
}

fn pipe_client_identity(pipe: BorrowedHandle<'_>) -> io::Result<Identity> {
    // SAFETY: the handle is a live server end of a named pipe and the client
    // was admitted by its user-only pipe DACL. Identification-level SQOS keeps
    // this limited to reading the client's identity; it grants no client access.
    if unsafe { ImpersonateNamedPipeClient(pipe.as_raw_handle()) } == 0 {
        return Err(operation_error(
            "ImpersonateNamedPipeClient",
            io::Error::last_os_error(),
        ));
    }
    let _revert = RevertToSelfGuard;
    let mut raw = null_mut();
    // SAFETY: GetCurrentThread is a valid pseudo-handle; raw is writable.
    if unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, 1, &mut raw) } == 0 {
        return Err(operation_error(
            "OpenThreadToken(TOKEN_QUERY)",
            io::Error::last_os_error(),
        ));
    }
    // SAFETY: successful OpenThreadToken transfers a unique valid token handle.
    let token = unsafe { OwnedHandle::from_raw_handle(raw) };
    identity_from_token(token.as_raw_handle())
}

pub(crate) fn verify_pipe_server(pipe: BorrowedHandle<'_>) -> io::Result<()> {
    verify_pipe_server_for_user(pipe, &current_identity()?)
}

fn verify_pipe_server_for_user(pipe: BorrowedHandle<'_>, user: &Identity) -> io::Result<()> {
    // Validate the kernel handle type before querying its security descriptor.
    // The descriptor belongs to this pipe instance, not to its enumerable name.
    pipe_peer_pid(pipe)?;
    verify_storage_acl(pipe, user, false)
}

pub(crate) fn verify_pipe_peer(pipe: BorrowedHandle<'_>) -> io::Result<u32> {
    let current = current_identity()?;
    let pid = pipe_peer_pid(pipe)?;
    let peer = pipe_client_identity(pipe)?;
    if peer.sid != current.sid {
        return Err(denied("control peer belongs to another user"));
    }
    // Re-read the kernel peer PID after impersonation. The pipe handle itself is
    // the liveness boundary; no OpenProcess/OpenProcessToken cross-integrity
    // query is needed, so a medium SSH bridge can connect to a high Server.
    if pipe_peer_pid(pipe)? != pid {
        return Err(denied("control peer changed during authentication"));
    }
    Ok(pid)
}

#[cfg(test)]
mod pipe_server_tests {
    use super::*;
    use std::{fs::OpenOptions, os::windows::fs::OpenOptionsExt};
    use windows_sys::Win32::{
        Storage::FileSystem::{FILE_GENERIC_READ, FILE_GENERIC_WRITE, PIPE_ACCESS_DUPLEX},
        System::Pipes::{CreateNamedPipeW, PIPE_REJECT_REMOTE_CLIENTS},
    };

    #[test]
    fn opened_pipe_server_owner_must_match_expected_user() {
        let current = current_identity().unwrap();
        let descriptor = private_descriptor(&current, false).unwrap();
        let attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0.as_ptr(),
            bInheritHandle: 0,
        };
        let name = format!(r"\\.\pipe\latte-work-owner-test-{}", std::process::id());
        let wide_name = wide(OsStr::new(&name)).unwrap();
        // SAFETY: the terminated name and descriptor remain live throughout
        // creation; the non-inheritable instance is immediately owned below.
        let raw = unsafe {
            CreateNamedPipeW(
                wide_name.as_ptr(),
                PIPE_ACCESS_DUPLEX,
                PIPE_REJECT_REMOTE_CLIENTS,
                1,
                1024,
                1024,
                0,
                &attributes,
            )
        };
        assert_ne!(raw, INVALID_HANDLE_VALUE, "{}", io::Error::last_os_error());
        // SAFETY: CreateNamedPipeW returned one valid uniquely owned handle.
        let _server = unsafe { OwnedHandle::from_raw_handle(raw) };
        let client = OpenOptions::new()
            .read(true)
            .write(true)
            .access_mode(FILE_GENERIC_READ | FILE_GENERIC_WRITE | READ_CONTROL)
            .open(name)
            .unwrap();
        verify_pipe_server(client.as_handle()).unwrap();
        // Model a client whose expected user differs from this real pipe owner.
        // This does not require privileges to assign a foreign owner or logon.
        let foreign = Identity {
            sid: "S-1-5-21-111-222-333-444".to_owned(),
        };
        assert_ne!(foreign.sid, current.sid);
        let error = verify_pipe_server_for_user(client.as_handle(), &foreign).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert!(error.to_string().contains("owned exclusively"), "{error}");
    }
}
