use super::{Identity, WindowsIdentity};
use anyhow::{Result, bail};
use std::{
    fs::{File, Metadata},
    path::Path,
};
pub(super) fn identity(path: &Path) -> Result<Identity> {
    let (volume, file_id) = latte_work_platform::fs::directory_identity(path)?;
    Ok(Identity::Windows {
        windows_v1: WindowsIdentity { volume, file_id },
    })
}
pub(super) fn mode(_: &Metadata) -> Option<u32> {
    None
}
pub(super) fn restore_mode(_: &File, _: u32) -> Result<()> {
    require_undo()
}
pub(super) fn require_undo() -> Result<()> {
    bail!("此 Windows Host 尚不支持 Undo；未更改任何文件")
}
