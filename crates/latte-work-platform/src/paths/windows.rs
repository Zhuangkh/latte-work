use std::{
    io,
    path::{Component, Path, Prefix},
};
pub(super) fn overlap(left: &Path, right: &Path) -> io::Result<bool> {
    let left_id = crate::fs::directory_identity(left)?;
    let right_id = crate::fs::directory_identity(right)?;
    for path in left.ancestors() {
        if crate::fs::directory_identity(path)? == right_id {
            return Ok(true);
        }
    }
    for path in right.ancestors() {
        if crate::fs::directory_identity(path)? == left_id {
            return Ok(true);
        }
    }
    Ok(false)
}
pub(super) fn component(name: &str) -> io::Result<()> {
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end_matches(' ')
        .to_uppercase();
    if name.ends_with([' ', '.'])
        || name
            .chars()
            .any(|ch| ch.is_control() || "<>:\"/\\|?*".contains(ch))
        || matches!(
            stem.as_str(),
            "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
        )
        || ["COM", "LPT"].iter().any(|prefix| {
            stem.strip_prefix(prefix).is_some_and(|tail| {
                matches!(
                    tail,
                    "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
            })
        })
    {
        return Err(super::invalid());
    }
    Ok(())
}
pub(super) fn absolute(path: &Path) -> io::Result<()> {
    if !path.is_absolute() {
        return Err(super::invalid());
    }
    let mut parts = path.components();
    if !matches!(parts.next(), Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)))
    {
        return Err(super::invalid());
    }
    for part in parts {
        match part {
            Component::Normal(name) => component(name.to_str().ok_or_else(super::invalid)?)?,
            Component::RootDir => {}
            _ => return Err(super::invalid()),
        }
    }
    Ok(())
}
pub(super) fn check_project_links(root: &Path, relative: &Path) -> io::Result<()> {
    let mut path = root.to_owned();
    for part in relative.components() {
        path.push(part);
        if crate::fs::is_link(&path.symlink_metadata()?) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "project reparse points are unsupported",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn overlap_uses_directory_identity_for_case_aliases() {
        let root = tempfile::tempdir().unwrap();
        let child = root.path().join("Child");
        let other = root.path().join("Other");
        std::fs::create_dir(&child).unwrap();
        std::fs::create_dir(&other).unwrap();
        let alias = std::path::PathBuf::from(child.to_str().unwrap().to_ascii_uppercase());
        assert!(super::overlap(root.path(), &alias).unwrap());
        assert!(super::overlap(&alias, &child).unwrap());
        assert!(!super::overlap(&child, &other).unwrap());
    }
    #[test]
    fn rejects_windows_aliases_and_device_paths() {
        for name in ["name:stream", "NUL", "COM1.txt", "a.", "a ", "x?y"] {
            assert!(super::component(name).is_err(), "{name}");
        }
        for path in [r"C:relative", r"\\server\share\file", r"\\.\pipe\file"] {
            assert!(super::absolute(std::path::Path::new(path)).is_err());
        }
        assert!(super::absolute(std::path::Path::new(r"C:\项目\file.txt")).is_ok());
    }
}
