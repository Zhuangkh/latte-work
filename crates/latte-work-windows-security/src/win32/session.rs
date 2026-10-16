//! Existing Agent transcripts: preserve ACLs while pinning their native identity.
use super::*;
use std::{fs::OpenOptions, os::windows::fs::OpenOptionsExt, path::PathBuf};
use windows_sys::Win32::Storage::FileSystem::{FILE_APPEND_DATA, FILE_GENERIC_READ};

/// Pins every ordinary ancestor and an existing same-user, non-reparse file.
/// Writers may append concurrently; replacement is denied until this guard drops.
pub struct NativeSessionFile {
    file: File,
    _directories: Vec<File>,
}

impl NativeSessionFile {
    /// Never creates, truncates, repairs ACLs or grants general write access.
    pub fn open(path: &Path, append: bool) -> io::Result<Self> {
        let path = crate::path::control_path(path)?;
        let parent = path
            .parent()
            .ok_or_else(|| denied("session parent missing"))?;
        let mut current = PathBuf::new();
        let mut directories = Vec::new();
        for component in parent.components() {
            current.push(component);
            if !matches!(component, std::path::Component::Prefix(_)) {
                directories.push(open_directory(&current)?);
            }
        }
        let file = OpenOptions::new()
            .access_mode(FILE_GENERIC_READ | if append { FILE_APPEND_DATA } else { 0 })
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&path)?;
        verify_owner(&file, &current_identity()?)?;
        // Fail before any I/O if the filesystem cannot supply a full stable ID.
        directory_identity(&file)?;
        Ok(Self {
            file,
            _directories: directories,
        })
    }

    pub fn file(&self) -> &File {
        &self.file
    }

    pub fn file_mut(&mut self) -> &mut File {
        &mut self.file
    }

    pub fn identity(&self) -> io::Result<crate::DirectoryIdentity> {
        directory_identity(&self.file)
    }
}

fn verify_owner(file: &File, user: &Identity) -> io::Result<()> {
    use std::os::windows::fs::MetadataExt;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(denied(
            "native session must be an ordinary non-reparse file",
        ));
    }
    let (mut owner, mut descriptor) = (null_mut(), null_mut());
    // SAFETY: live READ_CONTROL handle; returned owner borrows the LocalAlloc
    // descriptor, owned immediately below. This never changes the source ACL.
    let status = unsafe {
        GetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            null_mut(),
            null_mut(),
            &mut descriptor,
        )
    };
    if status != 0 {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    let _descriptor = LocalMemory::new(descriptor)?;
    if owner.is_null() {
        return Err(denied("native session owner missing"));
    }
    // SAFETY: owner is the OS-returned SID inside the live descriptor above.
    if unsafe { sid_text(owner)? } != user.sid {
        return Err(denied("native session belongs to another user"));
    }
    Ok(())
}

#[cfg(test)]
#[path = "session/tests.rs"]
mod tests;
