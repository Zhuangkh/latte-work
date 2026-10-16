//! NTFS project files. Unlike private App storage, retain the user's ACL policy.
#[path = "project/managed.rs"]
mod managed;
#[cfg(test)]
#[path = "project/tests.rs"]
mod tests;
use super::*;
pub use managed::ManagedRecoveryRoot;
use std::{
    fs::OpenOptions,
    os::windows::fs::OpenOptionsExt,
    path::{Component, PathBuf},
};
use windows_sys::Win32::{
    Security::{
        Authorization::{ConvertSecurityDescriptorToStringSecurityDescriptorW, SetSecurityInfo},
        GROUP_SECURITY_INFORMATION, GetSecurityDescriptorDacl, GetSecurityDescriptorGroup,
        GetSecurityDescriptorOwner, PROTECTED_DACL_SECURITY_INFORMATION,
        UNPROTECTED_DACL_SECURITY_INFORMATION,
    },
    Storage::FileSystem::*,
    System::WindowsProgramming::{DRIVE_FIXED, DRIVE_REMOVABLE},
};
const ATTRIBUTES: u32 = FILE_ATTRIBUTE_READONLY
    | FILE_ATTRIBUTE_HIDDEN
    | FILE_ATTRIBUTE_SYSTEM
    | FILE_ATTRIBUTE_ARCHIVE
    | FILE_ATTRIBUTE_NORMAL;
const SECURITY_LIMIT: usize = 32 * 1024;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectMetadata {
    pub identity: crate::DirectoryIdentity,
    pub security: String,
    pub attributes: u32,
    pub write_time: i64,
    pub change_time: i64,
}
/// Holds every directory component without share-delete; never follows reparse points.
pub struct ProjectPath {
    target: PathBuf,
    directories: Vec<File>,
    root_index: usize,
}
impl ProjectPath {
    pub fn open(root: &Path, relative: &Path) -> io::Result<Self> {
        if relative.as_os_str().is_empty()
            || relative
                .components()
                .any(|p| !matches!(p, Component::Normal(_)))
        {
            return Err(denied("project file requires a relative path"));
        }
        let root = crate::path::control_path(root)?;
        let drive: PathBuf = root.components().take(2).collect();
        let drive = wide(drive.as_os_str())?;
        // SAFETY: validated local-drive root, NUL-terminated UTF-16 input.
        let drive_type = unsafe { GetDriveTypeW(drive.as_ptr()) };
        if !matches!(drive_type, DRIVE_FIXED | DRIVE_REMOVABLE) {
            return Err(denied(
                "Windows Undo requires a local disk, not a mapped network drive",
            ));
        }
        let target = crate::path::control_path(&root.join(relative))?;
        let mut directories = Vec::new();
        let mut path = PathBuf::new();
        let mut root_index = None;
        for part in target
            .parent()
            .ok_or_else(|| denied("file parent missing"))?
            .components()
        {
            path.push(part);
            if matches!(part, Component::Prefix(_)) {
                continue;
            }
            reject_alias(&path)?;
            let file = open_directory(&path)?;
            if path == root {
                require_ntfs(&file)?;
                root_index = Some(directories.len());
            }
            directories.push(file);
        }
        match reject_alias(&target) {
            Ok(()) => (),
            Err(error) if error.kind() == io::ErrorKind::NotFound => (),
            Err(error) => return Err(error),
        }
        Ok(Self {
            target,
            directories,
            root_index: root_index.ok_or_else(|| denied("project root missing"))?,
        })
    }
    pub fn root_identity(&self) -> io::Result<crate::DirectoryIdentity> {
        directory_identity(&self.directories[self.root_index])
    }
    pub fn path(&self) -> &Path {
        &self.target
    }
    /// Read and optionally delete this exact object; disallow concurrent writers/deleters.
    pub fn open_file(&self, delete: bool) -> io::Result<File> {
        let file = OpenOptions::new()
            .access_mode(FILE_GENERIC_READ | if delete { DELETE } else { 0 })
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&self.target)?;
        reject_alias(&self.target)?;
        let attributes = file.metadata()?;
        if !attributes.is_file() {
            return Err(denied("project entry is not an ordinary file"));
        }
        Ok(file)
    }
    pub fn stage(self, name: &str, metadata: &ProjectMetadata) -> io::Result<StagedFile> {
        if name.is_empty()
            || Path::new(name).components().count() != 1
            || !matches!(
                Path::new(name).components().next(),
                Some(Component::Normal(_))
            )
        {
            return Err(denied("invalid staging file name"));
        }
        let path = crate::path::control_path(
            &self
                .target
                .parent()
                .ok_or_else(|| denied("file parent missing"))?
                .join(name),
        )?;
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .access_mode(FILE_GENERIC_READ | FILE_GENERIC_WRITE | WRITE_DAC | WRITE_OWNER | DELETE)
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&path)?;
        let staged = StagedFile {
            file,
            target: Some(self),
            published: false,
        };
        apply_security(&staged.file, &metadata.security)?;
        // Inherited ACEs can be recomputed from a changed parent. Confirm the exact
        // descriptor before returning a writer that may receive snapshot contents.
        if security_text(&staged.file)? != metadata.security {
            return Err(denied(
                "project security inheritance changed; cannot stage contents",
            ));
        }
        Ok(staged)
    }
}
/// A new, same-directory file. Unpublished files are deleted through their owned handle.
pub struct StagedFile {
    file: File,
    target: Option<ProjectPath>,
    published: bool,
}
impl StagedFile {
    pub fn file(&self) -> &File {
        &self.file
    }
    pub fn target(&self) -> &ProjectPath {
        self.target.as_ref().expect("owned staging target")
    }
    pub fn set_attributes(&self, attributes: u32) -> io::Result<()> {
        set_attributes(&self.file, attributes)
    }
    pub fn publish(mut self) -> io::Result<PublishedFile> {
        let file = self.file.try_clone()?;
        rename_no_replace(&self.file, self.target().path())?;
        self.published = true;
        verify_rename_destination(&self.file, self.target().path())?;
        Ok(PublishedFile {
            file,
            target: self.target.take().expect("owned staging target"),
        })
    }
}
/// Retains the published file and pinned parents through the caller's durable commit.
pub struct PublishedFile {
    file: File,
    target: ProjectPath,
}
impl PublishedFile {
    pub fn file(&self) -> &File {
        &self.file
    }
    pub fn target(&self) -> &ProjectPath {
        &self.target
    }
}
fn rename_no_replace(file: &File, target: &Path) -> io::Result<()> {
    let name = wide(target.as_os_str())?;
    let bytes = offset_of!(FILE_RENAME_INFO, FileName) + name.len() * 2;
    if bytes > 64 * 1024 {
        return Err(denied("rename path too long"));
    }
    let mut storage = vec![
        0usize;
        bytes
            .max(size_of::<FILE_RENAME_INFO>())
            .div_ceil(size_of::<usize>())
    ];
    let info = storage.as_mut_ptr().cast::<FILE_RENAME_INFO>();
    // SAFETY: aligned initialized buffer sized for header plus bounded UTF-16;
    // FileName is explicitly NUL-terminated for Win32 path conversion; the name
    // length excludes NUL. The held source file has DELETE access.
    let result = unsafe {
        (*info).Anonymous.ReplaceIfExists = false;
        (*info).FileNameLength = ((name.len() - 1) * 2) as u32;
        std::ptr::copy_nonoverlapping(name.as_ptr(), (*info).FileName.as_mut_ptr(), name.len());
        SetFileInformationByHandle(
            file.as_raw_handle(),
            FileRenameInfo,
            info.cast(),
            bytes as u32,
        )
    };
    if result == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
fn verify_rename_destination(file: &File, target: &Path) -> io::Result<()> {
    // Read the actual destination name as well as the retained object. This read
    // shares DELETE with our existing handle but requests no mutation rights.
    let destination = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(target)?;
    if directory_identity(file)? != directory_identity(&destination)? {
        return Err(denied(
            "Undo rename destination identity does not match the held file",
        ));
    }
    Ok(())
}
impl Drop for StagedFile {
    fn drop(&mut self) {
        if !self.published {
            // Only our unpublished temporary object may have readonly cleared.
            let _ = set_attributes(&self.file, FILE_ATTRIBUTE_NORMAL);
            let _ = delete_file(&self.file);
        }
    }
}
/// Dedicated capability for originals displaced by Undo. Git internals remain
/// forbidden to ProjectPath; this capability only creates its narrow namespace.
pub struct RecoveryDir {
    path: PathBuf,
    directories: Vec<File>,
    git_index: usize,
}
impl RecoveryDir {
    pub fn create(git_dir: &Path, attempt: &str) -> io::Result<Self> {
        require_uuid(attempt)?;
        let git_dir = crate::path::control_path(git_dir)?;
        require_local_disk(&git_dir)?;
        let mut directories = Vec::new();
        let mut path = PathBuf::new();
        for part in git_dir.components() {
            path.push(part);
            if matches!(part, Component::Prefix(_)) {
                continue;
            }
            reject_short_alias(&path)?;
            directories.push(open_directory(&path)?);
        }
        let git_index = directories.len() - 1;
        require_ntfs(&directories[git_index])?;
        path.push("latte-work-undo");
        match std::fs::create_dir(&path) {
            Ok(()) => (),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => (),
            Err(error) => return Err(error),
        }
        reject_short_alias(&path)?;
        directories.push(open_directory(&path)?);
        path.push(attempt);
        // An existing attempt is never adopted, repaired or cleaned up.
        std::fs::create_dir(&path)?;
        reject_short_alias(&path)?;
        directories.push(open_directory(&path)?);
        Ok(Self {
            path,
            directories,
            git_index,
        })
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn git_identity(&self) -> io::Result<crate::DirectoryIdentity> {
        directory_identity(&self.directories[self.git_index])
    }
    pub fn identity(&self) -> io::Result<crate::DirectoryIdentity> {
        directory_identity(self.directories.last().expect("owned recovery directory"))
    }
    pub fn require_same_volume(&self, source: &File) -> io::Result<()> {
        if directory_identity(source)?.volume != self.identity()?.volume {
            return Err(denied(
                "Undo recovery and project files must be on the same NTFS volume",
            ));
        }
        Ok(())
    }
    /// Move this exact held object without replacing any existing recovery object.
    /// The returned File only closes on Drop; the original is never deleted.
    pub fn displace(&self, source: File, name: &str) -> io::Result<File> {
        let id = name
            .strip_suffix(".original")
            .ok_or_else(|| denied("invalid recovery name"))?;
        require_uuid(id)?;
        self.require_same_volume(&source)?;
        let target = self.path.join(name);
        rename_no_replace(&source, &target)?;
        verify_rename_destination(&source, &target)?;
        Ok(source)
    }
}
fn require_uuid(value: &str) -> io::Result<()> {
    if value.len() != 36
        || !value.bytes().enumerate().all(|(index, byte)| {
            if [8, 13, 18, 23].contains(&index) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
    {
        return Err(denied("recovery name requires a generated UUID"));
    }
    Ok(())
}
fn require_local_disk(path: &Path) -> io::Result<()> {
    let drive: PathBuf = path.components().take(2).collect();
    let drive = wide(drive.as_os_str())?;
    // SAFETY: validated absolute local path and NUL-terminated UTF-16 drive root.
    let drive_type = unsafe { GetDriveTypeW(drive.as_ptr()) };
    if !matches!(drive_type, DRIVE_FIXED | DRIVE_REMOVABLE) {
        return Err(denied("Windows Undo requires a local disk"));
    }
    Ok(())
}
fn require_ntfs(file: &File) -> io::Result<()> {
    let mut name = [0u16; 32];
    // SAFETY: live volume-associated handle; optional outputs null, bounded writable buffer.
    if unsafe {
        GetVolumeInformationByHandleW(
            file.as_raw_handle(),
            null_mut(),
            0,
            null_mut(),
            null_mut(),
            null_mut(),
            name.as_mut_ptr(),
            name.len() as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let length = name.iter().position(|c| *c == 0).unwrap_or(name.len());
    if String::from_utf16_lossy(&name[..length]) != "NTFS" {
        return Err(denied("Windows Undo requires local NTFS"));
    }
    Ok(())
}
fn reject_alias(path: &Path) -> io::Result<()> {
    if path
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case(".git"))
    {
        return Err(denied("cannot restore Git internal files"));
    }
    reject_short_alias(path)
}
fn reject_short_alias(path: &Path) -> io::Result<()> {
    let original = wide(path.as_os_str())?;
    let mut expanded = vec![0u16; 32_768];
    // SAFETY: valid UTF-16 input and bounded writable output. No canonicalization through links.
    let length = unsafe {
        GetLongPathNameW(
            original.as_ptr(),
            expanded.as_mut_ptr(),
            expanded.len() as u32,
        )
    } as usize;
    if length == 0 {
        return Err(io::Error::last_os_error());
    }
    if length >= expanded.len() {
        return Err(denied("expanded path too long"));
    }
    use std::os::windows::ffi::OsStringExt;
    let expanded = PathBuf::from(std::ffi::OsString::from_wide(&expanded[..length]));
    if let Some(name) = expanded.file_name()
        && path
            .file_name()
            .is_some_and(|original| !original.eq_ignore_ascii_case(name))
    {
        return Err(denied("short-name file aliases are unsupported for Undo"));
    }
    Ok(())
}
pub fn project_metadata(file: &File) -> io::Result<ProjectMetadata> {
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: live file and correctly sized initialized output.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    if info.dwFileAttributes & !ATTRIBUTES != 0 || info.nNumberOfLinks != 1 {
        return Err(denied(
            "Undo requires an ordinary file without reparse, hard links, compression or encryption",
        ));
    }
    let mut streams = vec![0usize; (64 * 1024) / size_of::<usize>()];
    // SAFETY: aligned bounded output; inspect only the documented first header below.
    if unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileStreamInfo,
            streams.as_mut_ptr().cast(),
            64 * 1024,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let stream = streams.as_ptr().cast::<FILE_STREAM_INFO>();
    // SAFETY: successful FileStreamInfo fills a header in the allocation; all name
    // lengths/offsets are bounded before creating a slice. Reject extra streams.
    let ordinary = unsafe {
        let len = (*stream).StreamNameLength as usize;
        (*stream).NextEntryOffset == 0
            && len == 14
            && offset_of!(FILE_STREAM_INFO, StreamName) + len <= 64 * 1024
            && std::slice::from_raw_parts((*stream).StreamName.as_ptr(), len / 2)
                == "::$DATA".encode_utf16().collect::<Vec<_>>()
    };
    if !ordinary {
        return Err(denied("named data streams are unsupported for Undo"));
    }
    let mut basic = FILE_BASIC_INFO::default();
    // SAFETY: correctly sized aligned output and live handle.
    if unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileBasicInfo,
            (&mut basic as *mut FILE_BASIC_INFO).cast(),
            size_of::<FILE_BASIC_INFO>() as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(ProjectMetadata {
        identity: directory_identity(file)?,
        security: security_text(file)?,
        attributes: info.dwFileAttributes,
        write_time: basic.LastWriteTime,
        change_time: basic.ChangeTime,
    })
}
fn security_text(file: &File) -> io::Result<String> {
    let mut descriptor = null_mut();
    let flags = OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION;
    // SAFETY: live READ_CONTROL handle, Windows allocates a self-contained descriptor.
    let result = unsafe {
        GetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            flags,
            null_mut(),
            null_mut(),
            null_mut(),
            null_mut(),
            &mut descriptor,
        )
    };
    if result != 0 {
        return Err(io::Error::from_raw_os_error(result as i32));
    }
    let descriptor = LocalMemory::new(descriptor)?;
    let (mut text, mut length) = (null_mut(), 0);
    // SAFETY: Windows descriptor remains live; conversion returns LocalAlloc UTF-16.
    if unsafe {
        ConvertSecurityDescriptorToStringSecurityDescriptorW(
            descriptor.0.as_ptr(),
            SDDL_REVISION_1,
            flags,
            &mut text,
            &mut length,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let allocation = LocalMemory::new(text.cast())?;
    if length == 0 || length as usize > SECURITY_LIMIT {
        return Err(denied("file security descriptor exceeds limit"));
    }
    let mut units = Vec::new();
    for index in 0..length as usize {
        // SAFETY: Windows returns a NUL-terminated LocalAlloc string. Read only
        // through its first terminator, bounded by the returned size and our cap.
        let unit = unsafe { *allocation.0.as_ptr().cast::<u16>().add(index) };
        if unit == 0 {
            return String::from_utf16(&units).map_err(|_| denied("invalid security encoding"));
        }
        units.push(unit);
    }
    Err(denied("unterminated security descriptor"))
}
fn apply_security(file: &File, security: &str) -> io::Result<()> {
    let text = wide(OsStr::new(security))?;
    if text.len() > SECURITY_LIMIT {
        return Err(denied("file security descriptor exceeds limit"));
    }
    let mut descriptor = null_mut();
    // SAFETY: bounded NUL-terminated SDDL; Windows validates it and owns output allocation.
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            text.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let descriptor = LocalMemory::new(descriptor)?;
    let raw = descriptor.0.as_ptr();
    let (mut owner, mut group, mut dacl) = (null_mut(), null_mut(), null_mut());
    let (mut defaulted, mut present, mut control, mut revision) = (0, 0, 0, 0);
    // SAFETY: descriptor comes from Windows validation. Component pointers borrow
    // its live allocation, retained until SetSecurityInfo completes.
    let valid = unsafe {
        GetSecurityDescriptorOwner(raw, &mut owner, &mut defaulted) != 0
            && GetSecurityDescriptorGroup(raw, &mut group, &mut defaulted) != 0
            && GetSecurityDescriptorDacl(raw, &mut present, &mut dacl, &mut defaulted) != 0
            && GetSecurityDescriptorControl(raw, &mut control, &mut revision) != 0
    };
    if !valid || owner.is_null() || group.is_null() || present == 0 || dacl.is_null() {
        return Err(denied("incomplete project security descriptor"));
    }
    let protection = if control & SE_DACL_PROTECTED != 0 {
        PROTECTED_DACL_SECURITY_INFORMATION
    } else {
        UNPROTECTED_DACL_SECURITY_INFORMATION
    };
    // SAFETY: valid component pointers and owned staging file with WRITE_DAC/WRITE_OWNER.
    // Only temp files are modified; no privilege adjustment or SACL restoration.
    let result = unsafe {
        SetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION
                | GROUP_SECURITY_INFORMATION
                | DACL_SECURITY_INFORMATION
                | protection,
            owner,
            group,
            dacl,
            null(),
        )
    };
    if result != 0 {
        return Err(io::Error::from_raw_os_error(result as i32));
    }
    Ok(())
}
fn set_attributes(file: &File, attributes: u32) -> io::Result<()> {
    if attributes & !ATTRIBUTES != 0 {
        return Err(denied("unsupported file attributes"));
    }
    let basic = FILE_BASIC_INFO {
        FileAttributes: if attributes == 0 {
            FILE_ATTRIBUTE_NORMAL
        } else {
            attributes
        },
        ..Default::default()
    };
    // SAFETY: initialized struct, zero timestamps preserve times; held file is a staging file.
    if unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle(),
            FileBasicInfo,
            (&basic as *const FILE_BASIC_INFO).cast(),
            size_of::<FILE_BASIC_INFO>() as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
/// Delete an already-open, validated file through its handle; caller needs DELETE access.
pub fn delete_file(file: &File) -> io::Result<()> {
    let info = FILE_DISPOSITION_INFO { DeleteFile: true };
    // SAFETY: initialized structure and live file handle; no path lookup or ACL change.
    if unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle(),
            FileDispositionInfo,
            (&info as *const FILE_DISPOSITION_INFO).cast(),
            size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
