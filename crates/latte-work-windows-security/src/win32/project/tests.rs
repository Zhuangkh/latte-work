use super::*;
use std::io::{Read, Seek, Write};
use windows_sys::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_WRITE, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile, PAGE_READWRITE,
    UnmapViewOfFile,
};

const ATTEMPT: &str = "1c3b2850-5b7a-4714-98b0-7d4fc359f4d5";
const ENTRY: &str = "756566a1-dfce-483a-b69e-d8392c82c8ac.original";

#[test]
fn held_source_moves_to_recovery_and_foreign_gap_creation_wins() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join(".git")).unwrap();
    let target = root.path().join("file.bin");
    std::fs::write(&target, b"original afterimage").unwrap();
    let guard = ProjectPath::open(root.path(), Path::new("file.bin")).unwrap();
    let source = guard.open_file(true).unwrap();
    let metadata = project_metadata(&source).unwrap();
    let staged = guard.stage(".latte-undo-test", &metadata).unwrap();
    staged.file().write_all(b"beforeimage").unwrap();
    let recovery = RecoveryDir::create(&root.path().join(".git"), ATTEMPT).unwrap();
    assert!(RecoveryDir::create(&root.path().join(".git"), ATTEMPT).is_err());
    let mut displaced = recovery.displace(source, ENTRY).unwrap();
    assert_eq!(directory_identity(&displaced).unwrap(), metadata.identity);
    displaced.rewind().unwrap();
    let mut bytes = Vec::new();
    displaced.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"original afterimage");
    std::fs::write(&target, b"foreign creation").unwrap();
    assert!(staged.publish().is_err());
    assert_eq!(std::fs::read(&target).unwrap(), b"foreign creation");
    assert_eq!(
        std::fs::read(recovery.path().join(ENTRY)).unwrap(),
        b"original afterimage"
    );
    assert!(std::fs::remove_file(recovery.path().join(ENTRY)).is_err());
    drop(displaced);
    drop(recovery);
    assert_eq!(
        std::fs::read(
            root.path()
                .join(".git/latte-work-undo")
                .join(ATTEMPT)
                .join(ENTRY)
        )
        .unwrap(),
        b"original afterimage"
    );
}

struct WritableView(MEMORY_MAPPED_VIEW_ADDRESS);
impl Drop for WritableView {
    fn drop(&mut self) {
        // SAFETY: this guard uniquely owns the successful MapViewOfFile view.
        unsafe {
            UnmapViewOfFile(self.0);
        }
    }
}

#[test]
fn retained_writable_view_rejects_validation_after_file_and_mapping_handles_close() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("mapped.bin");
    std::fs::write(&path, b"mapped bytes").unwrap();
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    // SAFETY: live readable/writable file, no inherited handle, zero size maps the
    // existing nonempty file; Windows returns one uniquely owned mapping handle.
    let raw =
        unsafe { CreateFileMappingW(file.as_raw_handle(), null(), PAGE_READWRITE, 0, 0, null()) };
    assert!(!raw.is_null(), "{}", io::Error::last_os_error());
    // SAFETY: successful CreateFileMappingW transfers ownership of this handle.
    let mapping = unsafe { OwnedHandle::from_raw_handle(raw) };
    // SAFETY: live mapping, zero offset and size request the complete existing file.
    let view = unsafe { MapViewOfFile(mapping.as_raw_handle(), FILE_MAP_WRITE, 0, 0, 0) };
    assert!(!view.Value.is_null(), "{}", io::Error::last_os_error());
    let view = WritableView(view);
    drop(mapping);
    drop(file);
    let target = ProjectPath::open(root.path(), Path::new("mapped.bin")).unwrap();
    assert!(
        target.open_file(true).is_err(),
        "writable view must block omitted FILE_SHARE_WRITE"
    );
    // SAFETY: the retained writable view still maps the nonempty file.
    unsafe {
        view.0.Value.cast::<u8>().write(b'M');
    }
    drop(view);
    assert_eq!(std::fs::read(&path).unwrap(), b"Mapped bytes");
    assert!(target.open_file(true).is_ok());
}

#[test]
fn recovery_names_collisions_and_pinned_chain_never_modify_sources() {
    let root = tempfile::tempdir().unwrap();
    let git = root.path().join("git-control");
    std::fs::create_dir(&git).unwrap();
    let path = root.path().join("source.bin");
    std::fs::write(&path, b"preserve source").unwrap();
    for attempt in ["", ".", "..", "../outside", "a:stream", "not-a-uuid"] {
        assert!(RecoveryDir::create(&git, attempt).is_err());
    }
    for bad in [
        Path::new("relative"),
        Path::new(r"C:\temp\..\control"),
        Path::new(r"\\server\share\control"),
    ] {
        assert!(RecoveryDir::create(bad, ATTEMPT).is_err());
    }
    let recovery = RecoveryDir::create(&git, ATTEMPT).unwrap();
    assert!(std::fs::rename(&git, root.path().join("moved-git")).is_err());
    assert!(std::fs::rename(recovery.path(), root.path().join("moved-attempt")).is_err());
    std::fs::write(recovery.path().join(ENTRY), b"foreign slot").unwrap();
    let target = ProjectPath::open(root.path(), Path::new("source.bin")).unwrap();
    let source = target.open_file(true).unwrap();
    let expected = directory_identity(&source).unwrap();
    assert!(recovery.displace(source, ENTRY).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"preserve source");
    assert_eq!(
        directory_identity(&File::open(&path).unwrap()).unwrap(),
        expected
    );
    assert_eq!(
        std::fs::read(recovery.path().join(ENTRY)).unwrap(),
        b"foreign slot"
    );
    let source = target.open_file(true).unwrap();
    assert!(recovery.displace(source, "../../outside.original").is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"preserve source");
}

#[test]
fn cross_volume_recovery_is_rejected_before_renaming_the_source() {
    let source_dir = tempfile::tempdir().unwrap();
    let recovery_parent = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
    let path = source_dir.path().join("source.bin");
    std::fs::write(&path, b"cross-volume original").unwrap();
    let target = ProjectPath::open(source_dir.path(), Path::new("source.bin")).unwrap();
    let source = target.open_file(true).unwrap();
    let original_id = directory_identity(&source).unwrap();
    let recovery = RecoveryDir::create(recovery_parent.path(), ATTEMPT).unwrap();
    if original_id.volume == recovery.identity().unwrap().volume {
        println!("cross-volume fixture unavailable: temp and checkout share one volume");
        return;
    }
    assert!(recovery.require_same_volume(&source).is_err());
    assert!(recovery.displace(source, ENTRY).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"cross-volume original");
    assert_eq!(
        directory_identity(&File::open(path).unwrap()).unwrap(),
        original_id
    );
    assert!(!recovery.path().join(ENTRY).exists());
    println!("cross-volume recovery rejected before mutation");
}

#[test]
fn exact_destination_and_no_replace_hold_at_every_utf16_buffer_alignment() {
    let root = tempfile::tempdir().unwrap();
    for alignment in 0..size_of::<usize>() / 2 {
        let source_name = format!("source-{alignment}");
        std::fs::write(root.path().join(&source_name), b"owned bytes").unwrap();
        let guard = ProjectPath::open(root.path(), Path::new(&source_name)).unwrap();
        let source = guard.open_file(true).unwrap();
        let expected = directory_identity(&source).unwrap();
        let mut target = crate::path::control_path(&root.path().join("目标-🦀")).unwrap();
        while (offset_of!(FILE_RENAME_INFO, FileName)
            + target.as_os_str().encode_wide().count() * 2)
            % size_of::<usize>()
            != alignment * 2
        {
            target.as_mut_os_string().push("x");
        }
        // Alignment zero previously filled the entire word allocation without a
        // NUL, allowing Win32 conversion to append unrelated UTF-16 heap data.
        rename_no_replace(&source, &target).unwrap();
        verify_rename_destination(&source, &target).unwrap();
        assert!(!root.path().join(&source_name).exists());
        assert_eq!(std::fs::read(&target).unwrap(), b"owned bytes");
        assert_eq!(
            directory_identity(&File::open(&target).unwrap()).unwrap(),
            expected
        );
        drop(source);
        let other_name = format!("other-{alignment}");
        std::fs::write(root.path().join(&other_name), b"keep other").unwrap();
        let other = ProjectPath::open(root.path(), Path::new(&other_name))
            .unwrap()
            .open_file(true)
            .unwrap();
        assert!(rename_no_replace(&other, &target).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"owned bytes");
        assert_eq!(
            std::fs::read(root.path().join(other_name)).unwrap(),
            b"keep other"
        );
    }
    assert_eq!(
        std::fs::read_dir(root.path()).unwrap().count(),
        size_of::<usize>()
    );
}
