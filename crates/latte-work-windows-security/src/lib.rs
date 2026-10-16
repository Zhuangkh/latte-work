//! Narrow Windows security and owned-process boundary for the local daemon.
//! Only `win32` may use unsafe; all exported operations have safe owned/borrowed inputs.
#![deny(unsafe_code)]
#![deny(unsafe_op_in_unsafe_fn)]

#[cfg(windows)]
mod path;
#[cfg(windows)]
#[allow(unsafe_code)]
mod win32;

#[cfg(windows)]
pub use win32::console::{ConsoleChild, PseudoConsole};
#[cfg(windows)]
pub use win32::project::{
    ManagedRecoveryRoot, ProjectMetadata, ProjectPath, PublishedFile, RecoveryDir, StagedFile,
    delete_file, project_metadata,
};
#[cfg(windows)]
pub use win32::session::NativeSessionFile;

#[cfg(windows)]
use std::{
    fs::File,
    io,
    os::windows::io::BorrowedHandle,
    path::{Path, PathBuf},
};

/// Pins the validated control directory and its ancestors against rename/delete.
/// The path passed to constructors is the control directory itself, not its parent.
/// Keep the guard alive throughout listening or connection authentication.
#[cfg(windows)]
#[derive(Debug)]
pub struct ControlDirectory {
    path: PathBuf,
    _handles: Vec<File>,
}

#[cfg(windows)]
impl ControlDirectory {
    /// Create only the leaf with a protected user-only DACL. Existing objects
    /// are checked without modifying their ACLs. Ancestors must already exist.
    pub fn create_or_validate(path: &Path) -> io::Result<Self> {
        Self::open(path, true)
    }

    /// Validate existing objects without creating or repairing any path.
    pub fn open_verified(path: &Path) -> io::Result<Self> {
        Self::open(path, false)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Stable identity from the pinned leaf handle, independent of path spelling.
    pub fn identity(&self) -> io::Result<DirectoryIdentity> {
        win32::directory_identity(self._handles.last().expect("validated non-root directory"))
    }

    /// Recheck the pinned leaf's current ACL before another connection/admission.
    pub fn verify(&self) -> io::Result<()> {
        let user = win32::current_identity()?;
        win32::verify_directory(
            self._handles.last().expect("validated non-root directory"),
            &user,
        )
    }

    fn open(path: &Path, create: bool) -> io::Result<Self> {
        let path = path::control_path(path)?;
        let user = win32::current_identity()?;
        let mut current = PathBuf::new();
        let mut handles = Vec::new();
        for component in path.components() {
            current.push(component);
            if matches!(component, std::path::Component::Prefix(_)) {
                continue;
            }
            let leaf = current == path;
            if leaf && create {
                win32::create_directory(&current, &user)?;
            }
            // Every component is opened without following reparse points and
            // held without share-delete before traversing to the next one.
            let file = win32::open_directory(&current)?;
            if leaf {
                win32::verify_directory(&file, &user)?;
            }
            handles.push(file);
        }
        Ok(Self {
            path,
            _handles: handles,
        })
    }
}

/// Windows directory identity queried from an already validated, pinned handle.
#[cfg(windows)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DirectoryIdentity {
    pub volume: u64,
    pub file_id: [u8; 16],
}

/// Validate a native local disk spelling without resolving reparse points.
#[cfg(windows)]
pub fn local_path(path: &Path) -> io::Result<PathBuf> {
    path::control_path(path)
}

/// Ordinary state directories inherit their parent's ACL. Traverse and pin each
/// component while creating missing directories; never follow reparse points.
#[cfg(windows)]
pub fn prepare_state_directory(path: &Path) -> io::Result<PathBuf> {
    let path = path::control_path(path)?;
    let mut current = PathBuf::new();
    let mut guards = Vec::new();
    for part in path.components() {
        current.push(part);
        if matches!(part, std::path::Component::Prefix(_)) {
            continue;
        }
        match if matches!(part, std::path::Component::RootDir) {
            Ok(())
        } else {
            std::fs::create_dir(&current)
        } {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
        guards.push(win32::open_directory(&current)?);
    }
    Ok(path)
}

/// Read the full identity of an open file/directory; no ACL policy is implied.
#[cfg(windows)]
pub fn file_identity(file: &File) -> io::Result<DirectoryIdentity> {
    win32::directory_identity(file)
}

/// Validate an existing regular private file without changing its descriptor.
/// Accepts exactly one current-user full-control ACE, explicit or inherited.
#[cfg(windows)]
pub fn verify_private_file(file: &File) -> io::Result<()> {
    let user = win32::current_identity()?;
    win32::verify_private_file(file, &user)
}

/// Open an existing private file for reading without traversing reparse points.
/// Ancestors are pinned until the leaf is open; the returned handle denies writes
/// and replacement and is verified before any contents are read.
#[cfg(windows)]
pub fn open_private_file(path: &Path) -> io::Result<File> {
    use std::{fs::OpenOptions, os::windows::fs::OpenOptionsExt};
    use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ};

    let path = path::control_path(path)?;
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("private file needs a parent"))?;
    let mut current = PathBuf::new();
    let mut guards = Vec::new();
    for component in parent.components() {
        current.push(component);
        if !matches!(component, std::path::Component::Prefix(_)) {
            guards.push(win32::open_directory(&current)?);
        }
    }
    let file = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    verify_private_file(&file)?;
    Ok(file)
}

/// Create a new private file with an explicit user owner and protected user-only
/// DACL before any content can be written. Parent must be a verified private
/// directory; existing files are never opened, truncated or repaired.
#[cfg(windows)]
pub fn create_private_file(path: &Path) -> io::Result<File> {
    let path = path::control_path(path)?;
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("private file needs a parent"))?;
    let _guard = ControlDirectory::open_verified(parent)?;
    win32::create_private_file(&path, &win32::current_identity()?)
}

/// SID of the current process user, for the separate pipe DACL.
/// Elevation does not change the same-user policy.
#[cfg(windows)]
pub fn current_user_sid() -> io::Result<String> {
    Ok(win32::current_identity()?.sid)
}

/// Authenticate the server of an opened local named pipe before protocol traffic.
/// Its actual handle must have the current user as owner and an exact user-only
/// full-control DACL. No peer process/token query or protocol exchange is needed.
#[cfg(windows)]
pub fn verify_pipe_server(pipe: BorrowedHandle<'_>) -> io::Result<()> {
    win32::verify_pipe_server(pipe)
}

/// Authenticate the client from the server end of a connected local named pipe.
/// The peer PID and impersonated token come from the kernel, never from caller
/// protocol data. Both process tokens must belong to the same user. Windows
/// access checks still apply; no privilege or integrity-level bypass is attempted.
#[cfg(windows)]
pub fn verify_pipe_peer(pipe: BorrowedHandle<'_>) -> io::Result<u32> {
    win32::verify_pipe_peer(pipe)
}

#[cfg(windows)]
#[derive(Debug)]
/// An unnamed, non-inheritable, kill-on-close Windows Job for owned children.
/// Assignment must happen before allowing the child to execute user code.
pub struct ProcessJob(win32::ProcessJob);

#[cfg(windows)]
impl ProcessJob {
    pub fn new() -> io::Result<Self> {
        win32::ProcessJob::new().map(Self)
    }

    /// Assign a newly CREATE_SUSPENDED child and resume its initial threads.
    /// The caller must retain the child until this call returns. On error it must
    /// kill/reap the child; dropping this Job also kills any assigned processes.
    pub fn assign_and_resume(&self, child_id: u32) -> io::Result<()> {
        self.0.assign_and_resume(child_id)
    }

    pub fn terminate(&self) -> io::Result<()> {
        self.0.terminate()
    }

    /// Query current kernel accounting; zero confirms all Job processes exited.
    pub fn is_empty(&self) -> io::Result<bool> {
        self.0.is_empty()
    }
}

#[cfg(all(test, windows))]
mod tests;
