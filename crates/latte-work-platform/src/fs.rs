//! Private, application-owned storage. This does not define project-file restore permissions.
use std::{
    io,
    path::{Path, PathBuf},
};
use tempfile::NamedTempFile;

#[cfg(unix)]
#[path = "fs/unix.rs"]
mod backend;
#[cfg(windows)]
#[path = "fs/windows.rs"]
mod backend;

#[cfg(windows)]
pub use backend::directory_identity;
/// Existing same-user native Agent data; preserves the source ACL policy.
#[cfg(windows)]
pub use latte_work_windows_security::NativeSessionFile;
#[cfg(windows)]
pub use latte_work_windows_security::open_private_file;
/// Windows project restore primitives retain project permissions, distinct from PrivateDir.
#[cfg(windows)]
pub mod project {
    pub use latte_work_windows_security::{
        DirectoryIdentity as FileIdentity, ManagedRecoveryRoot, ProjectMetadata, ProjectPath,
        PublishedFile, RecoveryDir, StagedFile, delete_file, project_metadata,
    };
}
pub(crate) use backend::open_lock_file;

/// Open an existing regular file for reading without following the leaf link.
/// Never creates or truncates it. Windows rejects device/stream path spellings.
/// This does not impose an owner policy or pin ancestors; callers own the path boundary.
pub fn open_regular_file(path: &Path) -> io::Result<std::fs::File> {
    backend::open_regular_file(path)
}

/// Ordinary application state; Windows inherits ACLs, Unix retains 0700.
pub fn prepare_state_dir(path: &Path) -> io::Result<PathBuf> {
    backend::prepare_state(path)
}
/// Includes all Windows reparse points, not only symlinks.
pub fn is_link(metadata: &std::fs::Metadata) -> bool {
    backend::is_link(metadata)
}

/// A canonical application directory prepared for private storage.
///
/// The caller owns this directory's policy. Do not use this to chmod user projects.
/// As with ordinary path-based filesystem operations, callers must not concurrently
/// replace the directory or its ancestors. This is not a filesystem sandbox.
#[derive(Debug)]
pub struct PrivateDir {
    path: PathBuf,
    _guard: backend::Guard,
}

impl PrivateDir {
    /// Create missing directories and restrict the final directory before returning it.
    /// Reject a symlink at the supplied leaf, including a dangling one.
    /// Windows creates only the leaf; its parent must exist. Existing ACLs are
    /// validated without repair, and directory handles pin the path until Drop.
    pub fn prepare(path: &Path) -> io::Result<Self> {
        backend::validate_path(path)?;
        // Remove trailing separators/dots before lstat so `link/` cannot hide a leaf symlink.
        let normalized: PathBuf = path.components().collect();
        let path = normalized.as_path();
        match path.symlink_metadata() {
            Ok(metadata) if !metadata.is_dir() || is_link(&metadata) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "private directory must be a directory, not a link",
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let guard = backend::prepare_dir(path)?;
        Ok(Self {
            path: path.canonicalize()?,
            _guard: guard,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Recheck the retained directory's Windows owner/DACL before secret I/O.
    /// Unix permissions are established by `prepare`.
    pub fn verify(&self) -> io::Result<()> {
        #[cfg(windows)]
        self._guard.verify()?;
        Ok(())
    }

    /// Restrict an existing regular file on Unix; validate its owner/DACL on Windows.
    pub fn protect_file(&self, name: &str) -> io::Result<()> {
        let path = self.path.join(file_name(name)?);
        let metadata = path.symlink_metadata()?;
        if !metadata.is_file() || is_link(&metadata) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "private file must be a regular file, not a link",
            ));
        }
        backend::protect_file(&path)
    }

    /// Create a private temporary file in the same directory as its eventual destination.
    /// Permissions are established before any caller can write credentials.
    pub fn tempfile(&self, prefix: &str, suffix: &str) -> io::Result<NamedTempFile> {
        // Builder prefixes/suffixes must not redirect creation outside the private directory.
        file_name(&format!("{prefix}temporary{suffix}"))?;
        let mut builder = tempfile::Builder::new();
        builder.prefix(prefix).suffix(suffix);
        #[cfg(unix)]
        let file = builder.tempfile_in(&self.path)?;
        #[cfg(windows)]
        let file = builder.make_in(&self.path, latte_work_windows_security::create_private_file)?;
        backend::protect_open_file(file.as_file())?;
        Ok(file)
    }
}

pub fn verify_private_file(file: &std::fs::File) -> io::Result<()> {
    backend::verify_open_file(file)
}

fn file_name(name: &str) -> io::Result<&Path> {
    crate::paths::file_name(name)?;
    Ok(Path::new(name))
}
