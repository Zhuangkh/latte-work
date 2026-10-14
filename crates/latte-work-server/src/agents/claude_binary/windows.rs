pub(super) const NAME: &str = "claude.exe";
pub(super) fn validate(path: &std::path::Path) -> anyhow::Result<()> {
    if !path
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("exe"))
    {
        anyhow::bail!("Windows 首期仅支持原生 claude.exe；不支持 cmd/bat/PowerShell 启动脚本");
    }
    Ok(())
}
