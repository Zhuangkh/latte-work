# SSH host setup

The remote host needs OpenSSH, the matching `latte-work-server` executable and an
installed/authenticated Claude Code CLI. No browser runtime, Node frontend,
public service port, or Tauri dependency is needed for the server.

Build on the target Unix OS/architecture from this source checkout (Rust 1.97 and a C
compiler for bundled SQLite):

```sh
cargo build --release -p latte-work-server --locked
install -m 755 target/release/latte-work-server "$HOME/.local/bin/latte-work-server"
```

Create `~/.local/bin` first if needed. Verify `claude --version` and complete
`claude` login on that host. The service checks `LATTE_WORK_CLAUDE`, then
`~/.local/bin/claude`, then PATH. If PATH differs in noninteractive SSH, start the
service with an explicit absolute CLI path:

```sh
LATTE_WORK_CLAUDE=/absolute/path/to/claude \
  ~/.local/bin/latte-work-server serve
```

Normally no manual service start is needed: the Desktop's `connect` bridge
starts it on demand, detached from that SSH connection. To diagnose startup,
run `serve` in a terminal. To stop an existing daemon for an upgrade, send
SIGTERM to its verified PID after stopping/finishing tasks and closing terminals; SIGTERM cancels
remaining tasks and closes the service. Do not delete its runtime database.

First establish the normal SSH connection in your terminal so its host key is
verified:

```sh
ssh devbox
ssh devbox /absolute/path/to/latte-work-server --help
```

In Latte Work, open **设置 → SSH 连接 → 添加**. Enter a display name, SSH host,
and optional port. Select the remote OS (old records default to Linux / macOS).
For Unix, leave the Server path empty to find an executable at
`~/.local/bin/latte-work-server` or on the remote noninteractive SSH `PATH`;
enter an absolute path if installed elsewhere. Detection does not install or
upgrade the remote binary. Choose one authentication
mode: **默认身份验证** uses existing OpenSSH config/agent, **身份文件** selects a local
private key, and **密码** asks for a password. Save verifies the connection. You
can also reach this page from **创建项目 → 源文件夹 → 添加远程**. Then select the
saved host in the project form. Use **添加** to browse its folders, or enter an
absolute path, then create the project. Local and remote projects share the
sidebar; selecting a project routes files, Git and Claude to its host.

This build uses protocol v1 (including model selection, Provider configuration and
directory browsing). Build the desktop and remote server from the same source revision;
protocol version mismatches fail the handshake. Stop an old daemon only after finishing
its tasks and closing its terminals.

A macOS binary cannot be copied to Linux. V0.1 does not automatically build,
upload or update remote binaries. Shared state is per operating-system user,
not across users. The desktop saves the host, port, authentication mode, identity
file path, remote platform and optional binary path. Passwords are held only in desktop process memory,
never in `hosts.json` or the macOS Keychain. If a password host needs to
reconnect after an app restart, the desktop prompts for its password once and
reuses it for later reconnects during that run. Canceling the prompt leaves
that host disconnected; use its reconnect action to try again. Host key checking
stays enabled; unknown keys must be verified in a terminal first.

Provider definitions and per-host Agent associations belong to the App configuration,
not the local Server. Management works offline. For both local and SSH execution,
the native App reads the latest selection and contacts only the selected host:
model requests carry metadata, and sends carry a fresh snapshot or explicit CLI mode.
A failed local Server cannot block remote model loading or sending. With no Provider
selected, each host uses only its own Agent configuration and model labels.

Password, identity-file and custom-port connections share this path. Running turns
keep their original snapshot and survive bridge disconnects. The remote Server never
writes snapshots to its Provider catalog or request ledger; temporary CLI settings can
contain credentials until a turn finishes. App startup migrates the legacy local
Provider file once, directly from disk, preserving the source. Legacy target-side
copies remain untouched and do not override explicit CLI mode. See [Provider](providers.md)
for paths, migration and error behavior. Existing per-turn protocol support is required.

## Windows OpenSSH targets

Windows targets use an explicit **Windows** remote OS selection and a drive-qualified
native executable path, for example `C:\Tools\Latte Work\latte-work-server.exe`.
`C:/Tools/latte-work-server.exe` also works. Empty paths, drive-relative paths,
UNC/device paths, alternate data streams and script files are rejected. This first
implementation does not automatically detect the remote OS or discover Windows binaries.
The remote command is independent of the desktop OS; Unix clients can select Windows.

Install a matching Windows Server binary and authenticate a native `claude.exe` for
the same Windows account used by SSH. Ordinary and elevated tokens follow the
same user-SID policy; the Server no longer explicitly rejects elevation. Windows OpenSSH must
allow remote commands, with `cmd.exe`, Windows PowerShell or PowerShell 7 as its
standard default shell. `powershell.exe` must be available on the noninteractive
PATH; arbitrary shell wrappers and custom `DefaultShellCommandOption` settings are
not covered. Verify the host key in a terminal before connecting, as for Unix.

Start Server **on the remote machine's local terminal, outside SSH**, as that account:

```powershell
& 'C:\Tools\Latte Work\latte-work-server.exe' serve
```

Keep that terminal running. If required, set `$env:LATTE_WORK_CLAUDE` to the native
Claude executable's absolute path before starting Server. A separately supervised
process can be used only if it preserves the same user and state directory. Windows
ACL/integrity and process-token access checks still apply; the App does not change
tokens or bypass operating-system access denials. For an administrator SSH account,
run the independent Server under that account with a compatible integrity level.
A Server that still reports `control channel requires non-elevated process tokens`
is an older binary: replace the **remote Server** after finishing its tasks and
restart it; updating only the client cannot remove that old check.
No service or scheduled task is installed by the App.

In **设置 → SSH 连接 → 添加**, choose **Windows**, enter the actual Server path,
and save. Windows SSH invokes `connect-existing`, which only attaches to the named
pipe of an already running daemon. If it is unavailable, saving fails with an explicit
instruction to start it. Both binaries must include this command; older remote binaries
must be updated. Unix connections continue to use `connect` and start on demand.

Windows SSH intentionally does not start a daemon in the SSH process tree:
`DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP` does not guarantee escape from an
OpenSSH Job Object. Keeping daemon startup independent preserves running tasks when
the SSH bridge disconnects, without weakening the current process supervision policy.
This requires the independent process to remain alive; logging off or terminating
that process is not a recoverable transport disconnect.

The launcher encodes path data separately from the PowerShell script and passes the
script via UTF-16LE `EncodedCommand`. `ProcessStartInfo` disables shell execution and
inherits raw stdin/stdout; it never pipes protocol bytes through PowerShell objects.
Command size is bounded for `cmd.exe`. No password is included in the command.

Focused verification:

```sh
cargo test -p latte-work-client --lib --locked
cargo test -p latte-work-server --features test-fixture --test native ssh:: --locked
npm run test -w apps/desktop -- src/SSHSettings.test.tsx src/api.test.ts
```

The native fixture executes the actual generated command through `cmd.exe` and
Windows PowerShell, and through `pwsh.exe` when installed. It forwards to the real
Server in an isolated state directory and verifies special-character/Unicode paths,
duplex Unicode protocol messages, bridge EOF and reconnect to the same daemon.
The missing-daemon test verifies that `connect-existing` does not create a database
or acquire a daemon lease. These are local shell/Server tests, not an sshd acceptance test.

The user reports real Windows SSH acceptance passed. The local fixture remains
separate evidence; no complete client-OS, default-shell, authentication or integrity
matrix was supplied. For additional environments, verify the independent same-user
daemon, access to the registered project, task survival across SSH disconnect,
same-daemon reconnect without prompt replay, and explicit absent-daemon/path/security
errors. Do not infer cross-user attack coverage from a working SSH connection.

## Local bundled server upgrades

Local desktop connections compare the running server's executable fingerprint with
the bundled server. If they differ, the daemon permits replacement only while idle
and with no open terminal. Busy daemons remain running; finish tasks, close terminals,
and reconnect to retry. No prompt is automatically replayed. A legacy daemon lacking
the lifecycle handshake needs a one-time manual restart after checking tasks and
terminals in the old app. Closing/reopening the desktop alone does not stop it.
This behavior also applies to release bundles; it is not a debug-only workaround.
Remote servers still require explicit installation and restart as described above.

## SSH startup diagnostics

A failed initial Hello now includes the last 8 KiB of SSH/bridge stderr, when
available, instead of only reporting `Host 连接已关闭`. This distinguishes an
askpass/host-key/authentication error from a missing executable, unsupported remote
command or denied Windows IPC identity. Stderr is drained concurrently to prevent
pipe backpressure. Collection waits at most 250 ms after failure; successful Hello
clears the buffer and subsequent output is drained without retention. Diagnostics
stay in memory and are not written to a log or host configuration.

A Server-side `IPC connection rejected` followed by a client-side closed-channel
error means the named pipe was accepted but failed validation before protocol
traffic. Server diagnostics identify the failed control-directory or connected-peer validation
stage and the failing Win32 operation. Both pipe ends enforce the current user SID:
the client checks the actual pipe handle's owner/exact DACL, while the server reads
the connected client's TokenUser through identification-only impersonation and checks
its kernel PID. Neither end opens the remote process token. See [IPC](ipc.md).
Compare `whoami` in the independent Server terminal and SSH session first. Elevation
is not an authentication filter, but Windows access policy can still refuse a
connection. Preserve the exact failing operation for diagnosis; do not weaken control
ACLs or disable peer validation. Errors naming cross-process `OpenProcessToken` checks
may identify an older binary; replace the remote Server only after its work is safe.

Windows password-helper acceptance can be tested without a real user's password:

```powershell
python -m venv target/ssh-password-smoke-venv
.\target\ssh-password-smoke-venv\Scripts\python.exe -m pip install paramiko==5.0.0
.\target\ssh-password-smoke-venv\Scripts\python.exe scripts/smoke-windows-ssh-password.py
```

The optional `--askpass-exe` argument selects an installed/release desktop helper;
`--ssh` selects a specific OpenSSH executable. The test uses only an ephemeral
localhost listener, a pinned temporary host key and a public fixture password.
It uses the production password challenge and generated SSH options, with Windows
`CREATE_NO_WINDOW`, and verifies an authenticated Hello. The fixture does not execute
the remote launcher. It therefore tests password delivery, not the target machine's
account policy, shell, Server installation or IPC permissions.
