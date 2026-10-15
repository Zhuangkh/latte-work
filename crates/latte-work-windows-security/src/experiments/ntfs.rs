//! Compare Windows publication APIs against independently read .NET ACLs/attributes.
use super::*;
use crate::win32::LocalMemory;
use std::{
    ffi::c_void,
    mem::{offset_of, size_of},
    os::windows::{ffi::OsStrExt, fs::OpenOptionsExt, io::AsRawHandle},
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::ERROR_SUCCESS,
    Security::{
        Authorization::{GetSecurityInfo, SE_FILE_OBJECT, SetSecurityInfo},
        DACL_SECURITY_INFORMATION, GROUP_SECURITY_INFORMATION, GetSecurityDescriptorControl,
        GetSecurityDescriptorDacl, GetSecurityDescriptorGroup, GetSecurityDescriptorOwner,
        OWNER_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, SE_DACL_PROTECTED,
        UNPROTECTED_DACL_SECURITY_INFORMATION,
    },
    Storage::FileSystem::*,
};

struct Security(LocalMemory);
impl Security {
    fn capture(file: &File) -> Self {
        let mut descriptor = null_mut();
        // SAFETY: live file with READ_CONTROL, initialized output. GetSecurityInfo
        // allocates the descriptor; LocalMemory releases it with LocalFree.
        let result = unsafe {
            GetSecurityInfo(
                file.as_raw_handle(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                null_mut(),
                null_mut(),
                &mut descriptor,
            )
        };
        assert_eq!(result, ERROR_SUCCESS);
        Self(LocalMemory::new(descriptor).unwrap())
    }
    fn apply(&self, file: &File) -> io::Result<()> {
        let (mut owner, mut group, mut dacl) = (null_mut(), null_mut(), null_mut());
        let (mut defaulted, mut present, mut control, mut revision) = (0, 0, 0, 0);
        let descriptor = self.0.0.as_ptr();
        // SAFETY: descriptor is an unmodified Windows allocation. Returned pointers
        // borrow that allocation, which remains live through SetSecurityInfo.
        unsafe {
            assert_ne!(
                GetSecurityDescriptorOwner(descriptor, &mut owner, &mut defaulted),
                0
            );
            assert_ne!(
                GetSecurityDescriptorGroup(descriptor, &mut group, &mut defaulted),
                0
            );
            assert_ne!(
                GetSecurityDescriptorDacl(descriptor, &mut present, &mut dacl, &mut defaulted),
                0
            );
            assert_ne!(
                GetSecurityDescriptorControl(descriptor, &mut control, &mut revision),
                0
            );
        }
        assert_ne!(present, 0);
        let flags = if control & SE_DACL_PROTECTED != 0 {
            PROTECTED_DACL_SECURITY_INFORMATION
        } else {
            UNPROTECTED_DACL_SECURITY_INFORMATION
        };
        // SAFETY: all component pointers borrow the live descriptor; file is held
        // with WRITE_DAC/WRITE_OWNER by this fixture. No SACL or privilege changes.
        let result = unsafe {
            SetSecurityInfo(
                file.as_raw_handle(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION
                    | GROUP_SECURITY_INFORMATION
                    | DACL_SECURITY_INFORMATION
                    | flags,
                owner,
                group,
                dacl,
                null(),
            )
        };
        if result == 0 {
            Ok(())
        } else {
            Err(io::Error::from_raw_os_error(result as i32))
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Publish {
    Move,
    Replace,
    Handle,
}
fn publish(kind: Publish, source: &Path, target: &Path, replace: bool) -> io::Result<()> {
    let source_wide = wide(source.as_os_str()).unwrap();
    let target_wide = wide(target.as_os_str()).unwrap();
    let ok = match kind {
        Publish::Move => {
            // SAFETY: bounded NUL-terminated strings remain live throughout call.
            unsafe {
                MoveFileExW(
                    source_wide.as_ptr(),
                    target_wide.as_ptr(),
                    if replace {
                        MOVEFILE_REPLACE_EXISTING
                    } else {
                        0
                    },
                )
            }
        }
        Publish::Replace => {
            assert!(replace);
            // SAFETY: two distinct fixture files; no backup or optional pointers.
            unsafe {
                ReplaceFileW(
                    target_wide.as_ptr(),
                    source_wide.as_ptr(),
                    null(),
                    0,
                    null(),
                    null(),
                )
            }
        }
        Publish::Handle => {
            let source = fs::OpenOptions::new()
                .access_mode(DELETE | SYNCHRONIZE)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
                .open(source)?;
            let name: Vec<u16> = target.as_os_str().encode_wide().collect();
            let bytes = offset_of!(FILE_RENAME_INFO, FileName) + name.len() * 2;
            assert!(bytes < 64 * 1024);
            let mut storage = vec![
                0usize;
                bytes
                    .max(size_of::<FILE_RENAME_INFO>())
                    .div_ceil(size_of::<usize>())
            ];
            let info = storage.as_mut_ptr().cast::<FILE_RENAME_INFO>();
            // SAFETY: aligned, zeroed buffer large enough for header and full UTF-16
            // name; lengths exclude NUL per FILE_RENAME_INFO contract. No pointer escapes.
            unsafe {
                (*info).Anonymous.ReplaceIfExists = replace;
                (*info).RootDirectory = null_mut();
                (*info).FileNameLength = (name.len() * 2) as u32;
                std::ptr::copy_nonoverlapping(
                    name.as_ptr(),
                    (*info).FileName.as_mut_ptr(),
                    name.len(),
                );
                SetFileInformationByHandle(
                    source.as_raw_handle(),
                    FileRenameInfo,
                    info.cast::<c_void>(),
                    bytes as u32,
                )
            }
        }
    };
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn metadata_oracle(path: &Path) -> String {
    let dir = path.parent().unwrap();
    let name = path.file_name().unwrap().to_str().unwrap();
    assert!(!name.contains('\''));
    pwsh(
        &format!(
            r#"$ErrorActionPreference='Stop'
$p = Join-Path $env:LATTE_NATIVE_EXPERIMENT_DIR '{name}'
$a = Get-Acl -LiteralPath $p
$a.GetSecurityDescriptorSddlForm([Security.AccessControl.AccessControlSections]'Owner,Group,Access')
[int][IO.File]::GetAttributes($p)
"#
        ),
        dir,
    )
}

fn prepare(dir: &Path, inherited: bool) -> (std::path::PathBuf, std::path::PathBuf) {
    let script = r#"$ErrorActionPreference='Stop'
$d = $env:LATTE_NATIVE_EXPERIMENT_DIR
$drive = [IO.DriveInfo]::new([IO.Path]::GetPathRoot($d))
if ($drive.DriveFormat -ne 'NTFS') { throw "NTFS required: $($drive.DriveFormat)" }
$u = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
foreach ($name in @('source','target')) {
  $p = Join-Path $d $name
  [IO.File]::WriteAllText($p, $name)
  $acl = Get-Acl -LiteralPath $p
  $sddl = "D:P(A;;FA;;;$u)"
  if ($name -eq 'target') { $sddl += '(A;;FR;;;WD)' }
  $acl.SetSecurityDescriptorSddlForm($sddl, [Security.AccessControl.AccessControlSections]::Access)
  Set-Acl -LiteralPath $p -AclObject $acl
}
[IO.File]::SetAttributes((Join-Path $d 'source'), [IO.FileAttributes]'Hidden,System,Archive')
[IO.File]::SetAttributes((Join-Path $d 'target'), [IO.FileAttributes]::Archive)
"#;
    pwsh(script, dir);
    if inherited {
        pwsh(
            r#"$ErrorActionPreference='Stop'
$p = Join-Path $env:LATTE_NATIVE_EXPERIMENT_DIR 'source'
$f = [IO.FileInfo]::new($p)
$a = [IO.FileSystemAclExtensions]::GetAccessControl($f, [Security.AccessControl.AccessControlSections]::Access)
$a.SetAccessRuleProtection($false, $true)
[IO.FileSystemAclExtensions]::SetAccessControl($f, $a)
"#,
            dir,
        );
    }
    (dir.join("source"), dir.join("target"))
}
fn attributes(path: &Path, value: u32) {
    let path = wide(path.as_os_str()).unwrap();
    // SAFETY: NUL-terminated path points only to our temporary fixture file.
    assert_ne!(unsafe { SetFileAttributesW(path.as_ptr(), value) }, 0);
}

#[test]
#[ignore = "opt-in Windows 11 NTFS experiment"]
fn publication_metadata_matrix() {
    require_ordinary_user();
    for inherited in [false, true] {
        for kind in [Publish::Move, Publish::Replace, Publish::Handle] {
            let dir = tempfile::tempdir().unwrap();
            let parent_before = pwsh(
                "(Get-Acl -LiteralPath $env:LATTE_NATIVE_EXPERIMENT_DIR).Sddl",
                dir.path(),
            );
            let (source, target) = prepare(dir.path(), inherited);
            let before = metadata_oracle(&source);
            let after = metadata_oracle(&target);
            let source_id = crate::file_identity(&File::open(&source).unwrap()).unwrap();
            publish(kind, &source, &target, true).unwrap();
            assert_eq!(fs::read(&target).unwrap(), b"source");
            assert!(!source.exists());
            let restored = metadata_oracle(&target);
            let target_id = crate::file_identity(&File::open(&target).unwrap()).unwrap();
            assert_eq!(
                source_id, target_id,
                "publication must use replacement file identity"
            );
            match kind {
                Publish::Move | Publish::Handle => assert_eq!(restored, before),
                // ReplaceFileW merges destination security; record attributes too.
                Publish::Replace => assert_eq!(restored.lines().next(), after.lines().next()),
            }
            let parent_after = pwsh(
                "(Get-Acl -LiteralPath $env:LATTE_NATIVE_EXPERIMENT_DIR).Sddl",
                dir.path(),
            );
            assert_eq!(parent_before, parent_after);
            println!(
                "{kind:?} inherited={inherited}: source_acl={}, destination_acl={}, source_attributes={}, destination_attributes={}, result_attributes={}",
                restored.lines().next() == before.lines().next(),
                restored.lines().next() == after.lines().next(),
                before.lines().nth(1).unwrap(),
                after.lines().nth(1).unwrap(),
                restored.lines().nth(1).unwrap()
            );
        }
    }
}

#[test]
#[ignore = "opt-in Windows 11 NTFS experiment"]
fn captured_security_can_be_reconstructed_before_publication() {
    require_ordinary_user();
    for inherited in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let (before, target) = prepare(dir.path(), inherited);
        let expected = metadata_oracle(&before);
        let snapshot = Security::capture(&File::open(&before).unwrap());
        let staged = dir.path().join("staged");
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .access_mode(FILE_GENERIC_READ | FILE_GENERIC_WRITE | WRITE_DAC | WRITE_OWNER)
            .open(&staged)
            .unwrap();
        // No sensitive content is written until the reconstructed descriptor is set.
        snapshot.apply(&file).unwrap();
        use std::io::Write;
        file.write_all(b"before\r\n\0binary").unwrap();
        file.sync_all().unwrap();
        attributes(
            &staged,
            FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM | FILE_ATTRIBUTE_ARCHIVE,
        );
        drop(file);
        assert_eq!(metadata_oracle(&staged), expected);
        publish(Publish::Handle, &staged, &target, true).unwrap();
        assert_eq!(metadata_oracle(&target), expected);
        assert_eq!(fs::read(&target).unwrap(), b"before\r\n\0binary");
        println!("owner/group/DACL/protection + H/S/A reconstructed, inherited={inherited}");
    }
}

#[test]
#[ignore = "opt-in Windows 11 NTFS experiment"]
fn occupied_readonly_and_noclobber_fail_without_target_mutation() {
    require_ordinary_user();
    for kind in [Publish::Move, Publish::Replace, Publish::Handle] {
        for restriction in ["occupied", "readonly"] {
            let dir = tempfile::tempdir().unwrap();
            let (source, target) = prepare(dir.path(), false);
            let held = if restriction == "occupied" {
                Some(
                    fs::OpenOptions::new()
                        .read(true)
                        .share_mode(FILE_SHARE_READ)
                        .open(&target)
                        .unwrap(),
                )
            } else {
                attributes(&target, FILE_ATTRIBUTE_READONLY | FILE_ATTRIBUTE_ARCHIVE);
                None
            };
            let target_before = metadata_oracle(&target);
            let source_before = metadata_oracle(&source);
            let result = publish(kind, &source, &target, true);
            let error = result.expect_err("restricted destination unexpectedly replaced");
            assert_eq!(fs::read(&target).unwrap(), b"target");
            assert_eq!(fs::read(&source).unwrap(), b"source");
            assert_eq!(metadata_oracle(&target), target_before);
            assert_eq!(metadata_oracle(&source), source_before);
            drop(held);
            attributes(&target, FILE_ATTRIBUTE_NORMAL);
            println!(
                "{kind:?} {restriction}: error={:?}, source/target unchanged",
                error.raw_os_error()
            );
        }
    }
    for kind in [Publish::Move, Publish::Handle] {
        let dir = tempfile::tempdir().unwrap();
        let (source, target) = prepare(dir.path(), false);
        let error = publish(kind, &source, &target, false).unwrap_err();
        assert_eq!(fs::read(&target).unwrap(), b"target");
        assert_eq!(fs::read(&source).unwrap(), b"source");
        fs::remove_file(&target).unwrap();
        attributes(
            &source,
            FILE_ATTRIBUTE_READONLY | FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_ARCHIVE,
        );
        let expected = metadata_oracle(&source);
        publish(kind, &source, &target, false).unwrap();
        assert_eq!(metadata_oracle(&target), expected);
        attributes(&target, FILE_ATTRIBUTE_NORMAL);
        println!(
            "{kind:?} no-clobber collision={:?}, absent target + readonly source succeeds",
            error.raw_os_error()
        );
    }
}

#[test]
#[ignore = "opt-in Windows 11 NTFS experiment"]
fn publication_is_per_file_and_has_no_destination_identity_compare() {
    require_ordinary_user();
    let dir = tempfile::tempdir().unwrap();
    let (source, target) = prepare(dir.path(), false);
    let verified_id = crate::file_identity(&File::open(&target).unwrap()).unwrap();
    // Deterministically model another writer replacing the path after validation.
    fs::rename(&target, dir.path().join("original-target")).unwrap();
    fs::write(&target, b"external replacement").unwrap();
    assert_ne!(
        crate::file_identity(&File::open(&target).unwrap()).unwrap(),
        verified_id
    );
    publish(Publish::Handle, &source, &target, true).unwrap();
    assert_eq!(fs::read(&target).unwrap(), b"source");
    assert_eq!(
        fs::read(dir.path().join("original-target")).unwrap(),
        b"target"
    );

    let second_source = dir.path().join("second-source");
    let second_target = dir.path().join("second-target");
    fs::write(&second_source, b"before second").unwrap();
    fs::write(&second_target, b"after second").unwrap();
    let held = fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(&second_target)
        .unwrap();
    assert!(publish(Publish::Handle, &second_source, &second_target, true).is_err());
    assert_eq!(fs::read(&target).unwrap(), b"source");
    assert_eq!(fs::read(&second_target).unwrap(), b"after second");
    drop(held);
    println!(
        "rename replaces whichever destination exists at call time; second-file failure does not roll back first-file success"
    );
}

#[test]
#[ignore = "opt-in Windows 11 NTFS experiment"]
fn pinned_parent_and_target_share_modes_expose_replacement_tradeoff() {
    require_ordinary_user();
    let dir = tempfile::tempdir().unwrap();
    let (source, target) = prepare(dir.path(), false);
    let directory = fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(dir.path())
        .unwrap();
    assert!(fs::rename(dir.path(), dir.path().with_extension("renamed")).is_err());
    let target_guard = fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(&target)
        .unwrap();
    assert!(publish(Publish::Handle, &source, &target, true).is_err());
    drop(target_guard);
    for share in [
        FILE_SHARE_READ | FILE_SHARE_DELETE,
        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
    ] {
        let target_guard = fs::OpenOptions::new()
            .read(true)
            .share_mode(share)
            .open(&target)
            .unwrap();
        let old_id = crate::file_identity(&target_guard).unwrap();
        let result = publish(Publish::Handle, &source, &target, true);
        println!("held target share={share}: rename={result:?}");
        assert!(
            result.is_err(),
            "update recorded Windows behavior if open-target replacement becomes supported"
        );
        assert_eq!(fs::read(&target).unwrap(), b"target");
        assert_eq!(crate::file_identity(&target_guard).unwrap(), old_id);
    }
    publish(Publish::Handle, &source, &target, true).unwrap();
    assert_eq!(fs::read(&target).unwrap(), b"source");
    drop(directory);
    println!(
        "parent pinned; legacy FileRenameInfo requires closing the target guard even with share-delete, leaving a destination validation/publication gap"
    );
}
