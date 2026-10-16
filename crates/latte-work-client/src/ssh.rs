//! Remote command construction follows the target OS, never the client's OS.
use anyhow::{Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SshPlatform {
    #[default]
    Unix,
    Windows,
}

pub(super) fn remote_command(binary: &str, platform: SshPlatform) -> Result<String> {
    if binary.len() > 4096 || binary.chars().any(char::is_control) {
        bail!("远程 Server 路径过长或包含控制字符");
    }
    match platform {
        SshPlatform::Unix => {
            if !binary.is_empty() && !binary.starts_with('/') {
                bail!("远程 Server 必须是 Unix 绝对路径");
            }
            if !binary.is_empty() {
                return Ok(format!("exec {} connect", super::shell_quote(binary)));
            }
            // Noninteractive SSH often omits ~/.local/bin from PATH.
            let discover = r#"if [ -x "$HOME/.local/bin/latte-work-server" ]; then
  exec "$HOME/.local/bin/latte-work-server" connect
fi
candidate=$(command -v latte-work-server 2>/dev/null || true)
if [ -n "$candidate" ] && [ -x "$candidate" ]; then
  exec "$candidate" connect
fi
printf '%s\n' '{"kind":"error","code":"server_not_found","message":"远程未找到 latte-work-server；请先安装或填写绝对路径"}'"#;
            Ok(format!("exec /bin/sh -c {}", super::shell_quote(discover)))
        }
        SshPlatform::Windows => {
            validate_windows_binary(binary)?;
            // Encode path data separately from script syntax. UTF-16LE is the
            // Windows PowerShell EncodedCommand contract.
            let script = windows_script(binary);
            let bytes: Vec<_> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
            let command = format!(
                "powershell.exe -NoLogo -NoProfile -NonInteractive -EncodedCommand {}",
                STANDARD.encode(bytes)
            );
            // cmd.exe (the default Windows OpenSSH shell) limits command lines.
            if command.len() > 7500 {
                bail!("Windows 远程 Server 路径超出 SSH 启动命令长度限制");
            }
            Ok(command)
        }
    }
}

fn validate_windows_binary(binary: &str) -> Result<()> {
    let bytes = binary.as_bytes();
    if bytes.len() < 4
        || !bytes[0].is_ascii_alphabetic()
        || bytes[1] != b':'
        || !matches!(bytes[2], b'\\' | b'/')
        || !binary.to_ascii_lowercase().ends_with(".exe")
        || binary[2..].contains([':', '"', '<', '>', '|', '?', '*'])
        || binary[3..].split(['\\', '/']).any(|part| {
            part.is_empty() || part == "." || part == ".." || part.ends_with([' ', '.'])
        })
    {
        bail!(
            "Windows 远程 Server 必须是带盘符的 .exe 绝对路径，例如 C:\\Tools\\latte-work-server.exe"
        );
    }
    Ok(())
}

fn windows_script(binary: &str) -> String {
    // Process.Start with no redirection inherits raw standard handles. Invoking a
    // native executable through a PowerShell pipeline can consume/re-encode stdin.
    // The daemon must already run outside the SSH session's process Job.
    // Encoding the path as data also avoids PowerShell's Unicode smart-quote
    // delimiters, which ordinary ASCII single-quote escaping would miss.
    let binary = STANDARD.encode(binary.as_bytes());
    format!(
        r#"$ErrorActionPreference='Stop'
$ProgressPreference='SilentlyContinue'
try {{
    $s=New-Object System.Diagnostics.ProcessStartInfo
    $s.FileName=[Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('{binary}'))
    $s.Arguments='connect-existing'
    $s.UseShellExecute=$false
    $p=[System.Diagnostics.Process]::Start($s)
    $p.WaitForExit()
    exit $p.ExitCode
}} catch {{
    [Console]::Out.WriteLine('{{"kind":"error","code":"server_start_failed","message":"Windows Server launch failed; check the executable path and permissions"}}')
    exit 1
}}"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_paths_are_validated_independently_of_the_local_os() {
        for path in [r"C:\Tools\latte-work-server.exe", "d:/工具 bin/server.EXE"] {
            assert!(remote_command(path, SshPlatform::Windows).is_ok(), "{path}");
            assert!(remote_command(path, SshPlatform::Unix).is_err());
        }
        for path in [
            "",
            "server.exe",
            r"C:server.exe",
            r"\server.exe",
            r"\\host\share\server.exe",
            r"\\?\C:\server.exe",
            "/usr/bin/server",
            r"C:\server.cmd",
            r"C:\x:server.exe",
            "C:\\bad\nserver.exe",
            "C:\\bad\0server.exe",
            r"C:\a\..\server.exe",
            r"C:\a.\server.exe",
            "C:\\a\\\"server.exe",
        ] {
            assert!(
                remote_command(path, SshPlatform::Windows).is_err(),
                "{path:?}"
            );
        }
        assert!(remote_command("/usr/bin/server", SshPlatform::Unix).is_ok());
        assert!(
            remote_command(
                &format!("C:/{}.exe", "a".repeat(4000)),
                SshPlatform::Windows
            )
            .is_err()
        );
    }

    #[test]
    fn windows_shell_receives_only_a_fixed_command_and_encoded_data() {
        let binary = r"C:\工具 & %PATH% ! ^ $() ` ' ; ‘ ’ “ ”\server.exe";
        let command = remote_command(binary, SshPlatform::Windows).unwrap();
        let (prefix, encoded) = command.rsplit_once(' ').unwrap();
        assert_eq!(
            prefix,
            "powershell.exe -NoLogo -NoProfile -NonInteractive -EncodedCommand"
        );
        assert!(
            encoded
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"+/=".contains(&b))
        );
        let bytes = STANDARD.decode(encoded).unwrap();
        let words: Vec<_> = bytes
            .chunks_exact(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect();
        let script = String::from_utf16(&words).unwrap();
        assert!(script.contains(&format!("FromBase64String('{}')", STANDARD.encode(binary))));
        assert!(!script.contains(binary));
        assert!(script.contains("$s.Arguments='connect-existing'"));
        assert!(!script.contains("RedirectStandard"));
    }
}
