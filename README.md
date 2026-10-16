# Latte Work

A desktop workspace for coding agents, with local and SSH projects.

**Tauri 2 · React · Rust**. The first integration is the installed **Claude Code
CLI**, using its structured stream-json protocol and native session history.

## Start

```sh
make setup
make dev
```

On Windows, install locked frontend dependencies and launch directly from PowerShell 7:

```powershell
npm ci
pwsh -NoProfile -File scripts/desktop.ps1 dev
```

Install and sign in to Claude on each execution host before sending a task.
Windows requires native `claude.exe`; interactive terminals require PowerShell 7
(`pwsh.exe`). Create a project, select this computer or an SSH host, choose its
folder, then create a conversation and send a prompt.

The desktop starts the bundled local server. Connections using the same host user
and state directory share one daemon, which owns projects, persistent history and
independent Agent processes. Closing the window hides it; explicit Quit cleans up
this App's owned resources. A transport disconnect does not stop accepted host tasks.
Windows SSH connects to a same-user daemon started independently outside SSH;
Unix SSH can start the daemon on demand. See [SSH setup](docs/remote.md).

## Capabilities

- Native desktop on macOS and Windows; headless server on macOS, Linux and Windows.
- Local and SSH projects, persistent conversations and bounded history replay.
- Claude tool events, explicit single-use approval, cancellation and native resume.
- Request deduplication, reconnect and honest interrupted/unknown state after crashes.
- Host-owned terminal tabs, read-only file previews and project-scoped Git review.
- Frozen per-turn changes and guarded Undo; Windows Undo supports eligible local
  NTFS regular files in Git and non-Git projects.
- App-owned Provider configuration, per-host Agent associations, and per-turn
  model, effort and permission choices. Claude accepts Anthropic Messages;
  OpenAI configurations can be saved but have no implemented Agent adapter.
- Unsigned macOS packaging and Windows x64 per-user NSIS installation.

The CLI's existing permission rules still apply. Register only projects you trust:
Claude loads their configuration. Latte Work does not manage CLI login or provide
an Agent sandbox. Unsupported agents and capabilities fail explicitly.

## Build and verify

```sh
make ci           # formatting, Clippy, tests, frontend build, docs
make build        # native desktop and server debug build
make package      # unsigned native application and standalone release server
make cache-info   # Cargo output path and optional sccache statistics
```

See [development](DEVELOPMENT.md) for prerequisites, PowerShell commands, test
boundaries and package provenance; [architecture](docs/architecture.md) for runtime
contracts; [Provider configuration](docs/providers.md), [UI design](docs/design-system.md)
and [theme import/export](docs/themes.md) for their respective workflows.

Automatic remote binary installation, Windows Service deployment, Windows MSI/ARM
installers, signing/notarization, updater publication and guaranteed continuation of
in-flight work after a daemon crash are outside the current support profile.
