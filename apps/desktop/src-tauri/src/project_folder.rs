//! Native folder-picker result. Paths and names stay separate at the UI boundary.
use serde::Serialize;
use std::path::PathBuf;

#[derive(Debug, Serialize)]
pub struct ProjectFolder {
    pub path: String,
    pub name: String,
}

impl TryFrom<PathBuf> for ProjectFolder {
    type Error = String;

    fn try_from(path: PathBuf) -> Result<Self, Self::Error> {
        let name = path
            .file_name()
            .unwrap_or_else(|| path.as_os_str())
            .to_str()
            .ok_or("目录名称不是 UTF-8")?
            .to_owned();
        let path = path.to_str().ok_or("目录路径不是 UTF-8")?.to_owned();
        Ok(Self { path, name })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picker_preserves_native_path_and_extracts_only_the_directory_name() {
        let path = std::env::temp_dir()
            .join("long parent directory ".repeat(8))
            .join("中文 project");
        let folder = ProjectFolder::try_from(path.clone()).unwrap();
        assert_eq!(folder.path, path.to_str().unwrap());
        assert_eq!(folder.name, "中文 project");
        let root = path.ancestors().last().unwrap();
        let folder = ProjectFolder::try_from(root.to_owned()).unwrap();
        assert_eq!(folder.name, root.to_str().unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn backslash_is_part_of_a_unix_directory_name() {
        let folder = ProjectFolder::try_from(PathBuf::from(r"/tmp/foo\bar")).unwrap();
        assert_eq!(folder.name, r"foo\bar");
    }

    #[cfg(windows)]
    #[test]
    fn windows_picker_handles_verbatim_paths_and_drive_roots() {
        for path in [r"C:\parent\项目", r"\\?\C:\parent\项目\"] {
            assert_eq!(
                ProjectFolder::try_from(PathBuf::from(path)).unwrap().name,
                "项目"
            );
        }
        for path in [r"C:\", r"\\?\C:\"] {
            assert_eq!(
                ProjectFolder::try_from(PathBuf::from(path)).unwrap().name,
                path
            );
        }
    }
}
