//! Explicitly registered private recovery storage for ordinary local projects.
use super::*;
use std::io::{Read, Write};

const NAMESPACE: &str = ".latte-work-undo";
const MANIFEST: &str = "owner.json";
const MANIFEST_LIMIT: usize = 64 * 1024;

#[cfg(test)]
#[path = "managed_tests.rs"]
mod tests;

/// Pins the project, private recovery root and every ancestor. A manifest alone
/// never authorizes reopening: callers must supply independently persisted IDs.
pub struct ManagedRecoveryRoot {
    path: PathBuf,
    directories: Vec<File>,
    project_index: usize,
}

impl ManagedRecoveryRoot {
    /// Atomically create a new private namespace. Never adopt an existing object,
    /// including an empty directory left by an interrupted initialization.
    pub fn create_new(
        project_root: &Path,
        expected_project_identity: crate::DirectoryIdentity,
    ) -> io::Result<Self> {
        let mut root = Self::pin_project(project_root, expected_project_identity)?;
        root.path.push(NAMESPACE);
        let user = current_identity()?;
        create_directory_new(&root.path, &user)?;
        reject_short_alias(&root.path)?;
        let directory = open_directory(&root.path)?;
        verify_directory(&directory, &user)?;
        root.directories.push(directory);
        root.require_same_volume()?;
        Ok(root)
    }

    /// Reopen only the independently registered project and store identities.
    /// No path is created, repaired or removed on failed verification.
    pub fn open_verified(
        project_root: &Path,
        expected_project_identity: crate::DirectoryIdentity,
        expected_store_identity: crate::DirectoryIdentity,
    ) -> io::Result<Self> {
        let mut root = Self::pin_project(project_root, expected_project_identity)?;
        root.path.push(NAMESPACE);
        reject_short_alias(&root.path)?;
        let directory = open_directory(&root.path)?;
        verify_directory(&directory, &current_identity()?)?;
        if directory_identity(&directory)? != expected_store_identity {
            return Err(denied("managed recovery directory identity changed"));
        }
        root.directories.push(directory);
        root.require_same_volume()?;
        Ok(root)
    }

    fn pin_project(project_root: &Path, expected: crate::DirectoryIdentity) -> io::Result<Self> {
        let project_root = crate::path::control_path(project_root)?;
        require_local_disk(&project_root)?;
        let mut path = PathBuf::new();
        let mut directories = Vec::new();
        for part in project_root.components() {
            path.push(part);
            if matches!(part, Component::Prefix(_)) {
                continue;
            }
            reject_alias(&path)?;
            directories.push(open_directory(&path)?);
        }
        let project_index = directories.len() - 1;
        require_ntfs(&directories[project_index])?;
        if directory_identity(&directories[project_index])? != expected {
            return Err(denied("managed recovery project identity changed"));
        }
        Ok(Self {
            path,
            directories,
            project_index,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn identity(&self) -> io::Result<crate::DirectoryIdentity> {
        directory_identity(self.directories.last().expect("pinned recovery root"))
    }

    pub fn project_identity(&self) -> io::Result<crate::DirectoryIdentity> {
        directory_identity(&self.directories[self.project_index])
    }

    fn require_same_volume(&self) -> io::Result<()> {
        if self.identity()?.volume != self.project_identity()?.volume {
            return Err(denied("managed recovery requires the project NTFS volume"));
        }
        Ok(())
    }

    fn verify(&self) -> io::Result<()> {
        verify_directory(
            self.directories.last().expect("pinned recovery root"),
            &current_identity()?,
        )
    }

    /// Fixed-leaf, private CREATE_NEW write, bounded before creation and flushed
    /// before returning. Failed initialization is retained for inspection.
    pub fn write_owner_manifest_new(&self, bytes: &[u8], max_bytes: usize) -> io::Result<()> {
        let limit = manifest_limit(max_bytes)?;
        if bytes.len() > limit {
            return Err(denied("managed recovery manifest exceeds limit"));
        }
        self.verify()?;
        let mut file = create_private_file(&self.path.join(MANIFEST), &current_identity()?)?;
        verify_manifest(&file)?;
        file.write_all(bytes)?;
        file.sync_all()
    }

    /// Validate the private file before reading any bytes; an extra byte detects
    /// oversized input without unbounded allocation or metadata-only trust.
    pub fn read_owner_manifest(&self, max_bytes: usize) -> io::Result<Vec<u8>> {
        let limit = manifest_limit(max_bytes)?;
        self.verify()?;
        let path = self.path.join(MANIFEST);
        reject_short_alias(&path)?;
        let file = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)?;
        verify_manifest(&file)?;
        // Reject streams/hard links and unsupported project-file attributes too.
        project_metadata(&file)?;
        let mut bytes = Vec::new();
        file.take((limit + 1) as u64).read_to_end(&mut bytes)?;
        if bytes.len() > limit {
            return Err(denied("managed recovery manifest exceeds limit"));
        }
        Ok(bytes)
    }

    /// New private attempt, with its own copies of every pinned ancestor. The
    /// returned capability remains safe even if this root guard is dropped.
    pub fn create_attempt(&self, attempt: &str) -> io::Result<RecoveryDir> {
        require_uuid(attempt)?;
        self.verify()?;
        let mut directories = self
            .directories
            .iter()
            .map(File::try_clone)
            .collect::<io::Result<Vec<_>>>()?;
        let path = self.path.join(attempt);
        let user = current_identity()?;
        create_directory_new(&path, &user)?;
        reject_short_alias(&path)?;
        let directory = open_directory(&path)?;
        verify_directory(&directory, &user)?;
        if directory_identity(&directory)?.volume != self.project_identity()?.volume {
            return Err(denied("managed recovery attempt changed volume"));
        }
        directories.push(directory);
        Ok(RecoveryDir {
            path,
            directories,
            git_index: self.project_index,
        })
    }
}

fn verify_manifest(file: &File) -> io::Result<()> {
    verify_private_file(file, &current_identity()?)?;
    let mut descriptor = null_mut();
    // SAFETY: live READ_CONTROL file handle and writable descriptor output.
    // Windows allocates the descriptor, immediately owned below on success.
    let status = unsafe {
        GetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            null_mut(),
            null_mut(),
            &mut descriptor,
        )
    };
    if status != 0 {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    require_protected(&LocalMemory::new(descriptor)?)
}

fn manifest_limit(max_bytes: usize) -> io::Result<usize> {
    if max_bytes == 0 || max_bytes > MANIFEST_LIMIT {
        return Err(denied("invalid managed recovery manifest bound"));
    }
    Ok(max_bytes)
}

fn create_directory_new(path: &Path, user: &Identity) -> io::Result<()> {
    let descriptor = private_descriptor(user, true)?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0.as_ptr(),
        bInheritHandle: 0,
    };
    let path = wide(path.as_os_str())?;
    // SAFETY: terminated path and explicit user-owned protected descriptor stay
    // live through atomic creation. Every collision is returned without repair.
    if unsafe { CreateDirectoryW(path.as_ptr(), &attributes) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
