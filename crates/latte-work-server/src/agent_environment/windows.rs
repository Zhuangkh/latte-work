use std::{collections::BTreeMap, ffi::OsString};

pub(super) async fn capture_default() -> anyhow::Result<BTreeMap<OsString, OsString>> {
    // Windows v1 inherits the Server environment. No PowerShell/Git Bash profiles
    // or duplicate case-sensitive PATH overlays; Command applies Windows semantics.
    Ok(BTreeMap::new())
}
