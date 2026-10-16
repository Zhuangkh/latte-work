# Architecture

## Ownership

```
React UI → Tauri commands → latte-work-client
                              ├─ local server connect-local bridge
                              └─ OpenSSH → remote server connect bridge
                                                ↓
                                   singleton latte-work-server
                                     ├─ SQLite projects/sessions/events
                                     ├─ read-only files / Git
                                     └─ AgentAdapter → Claude CLI children
```

`latte-work-platform` provides OS directory lookup, file locks and private storage
without dependencies on the protocol, configuration, Server or GUI. Platform-specific
implementations are selected at module boundaries; product paths and retry policy
remain with callers. Its Windows control-channel backend calls the project-owned
`latte-work-windows-security` boundary (windows-sys only) for user-only DACLs,
reparse checks and peer tokens. Unsafe is confined to that crate's private win32
module; platform and all other callers retain forbid. A second reviewed boundary,
`latte-work-windows-webview`, owns the WebView2 find/capture calls (see
[its SAFETY notes](../crates/latte-work-windows-webview/SAFETY.md)). The public
`ipc::Endpoint`
now owns transport identity and lock paths. Listener and accepted streams share
the singleton lease, and connect/accept return only authenticated streams retaining
their endpoint guard. See [the IPC contract](ipc.md). Ordinary Windows state uses
parent ACLs; Host Provider/launch files and attachments use verified private leaves.
Native path and snapshot rules are described below; build, packaging and validation
commands live in [Development](../DEVELOPMENT.md). The two Windows SAFETY documents
record the exact FFI ownership and bounds; callers do not gain unsafe allowances.

`latte-work-protocol` contains only serde wire types and TypeScript type export.
`latte-work-client` owns version handshakes, bounded frames, connection timeouts,
and local/SSH transport. It knows no Claude message formats.
`latte-work-server` owns all execution and durable host state. Its internal
`AgentAdapter` is defined in `agents/adapter.rs`, separately from implementations.
The registry in `agents/mod.rs` constructs a fresh adapter by `session.agent` for
each turn and rejects unsupported IDs. Only Claude is implemented; its executable
discovery, launch configuration, wire encoding/decoding and protocol state live in
`agents/claude.rs`. The internal transport contract currently covers bounded UTF-8
line-framed stdio, not arbitrary future Agent transports.
`apps/desktop` owns presentation, OS integration and saved host connections.
Its pending-close journal uses `latte-work-platform` `PrivateDir`/`FileLock` for the
validated private leaf, and `latte-work-client` launches stdio bridges with
`CREATE_NO_WINDOW` on Windows. Browser page find/capture uses the reviewed
`latte-work-windows-webview` boundary, and the SSH askpass helper shares the
control-channel private pipe. Windows desktop support includes the local sidecar, platform integration, window
chrome, SSH askpass, WebView2 find/capture and unsigned x64 per-user NSIS packaging.
Windows 11 terminals use pwsh/ConPTY; eligible local NTFS files support Undo.

## Adapter and Runtime boundary

The adapter receives `Start { prompt }`, a complete native `Message` line, or an
approval decision already authorized by Runtime. It returns ordered actions:
encoded bytes to write, readiness, native session identity, common events,
approval requests, or explicit completion. It cannot read the database, spawn a
child itself, or authorize approvals. Claude owns the sequence initialize →
acknowledgment → prompt, sends the prompt only once, and recognizes completion
from its native result message. Runtime never parses Claude JSON or sends a prompt
as a side effect of readiness; readiness only ends the startup timeout.

Runtime executes actions, supervises processes, bounds I/O, enforces deadlines,
handles cancellation, and persists session IDs/events/status. It maps native
approval requests to fresh public IDs and consumes an ID exactly once before
returning the original input and decision to the adapter. EOF or process exit
without explicit completion is a failure. Server API persists an accepted request
before launching Runtime, retaining the existing deduplication and restart rules.
The refactor does not change desktop/host protocol v1 or add another Agent.

## One daemon per host user

Default state is `~/.local/share/latte-work` on Unix and the system Known Folder
`%LOCALAPPDATA%/latte-work` on Windows. `server.lock` is an OS file lock held
for the daemon lifetime. `connect` attaches to its private Unix socket, starting
`serve` only when necessary. Racing launch attempts are serialized by the lock;
losing bridges attach to the winning daemon. A bridge is not another server.
The local `connect-local` bridge additionally compares executable SHA-256 fingerprints
using a private lifecycle handshake before the application Hello. Fingerprints
identify builds without changing protocol v1. `upgrade.lock` serializes local
coordinators. A mismatched server receives `prepare_upgrade` bound to its instance
ID; a write admission lock prevents new sends or terminal creation racing the idle
check. Running/waiting sessions, live run controls, or any open terminal block
replacement. An idle daemon drains, removes its socket and releases `server.lock`
before the bundled server starts. The coordinator verifies the replacement on the
actual socket it forwards. All waits are bounded; no requests are replayed.
Legacy servers without this handshake require a manual restart, never a forced
kill. SSH does not perform automatic upgrades. Windows `connect-existing` requires an
independent same-user daemon; Unix `connect` retains on-demand startup.

`serve`, `connect`, and `connect-local` call the same platform IPC API; the Unix
backend preserves the established socket address and the Windows backend uses a
user-only named pipe. `process::ManagedChild` supervises Agents/probes with Unix
process groups or owned Windows Jobs; daemon launch and exit notification have
separate APIs. Shutdown fences admission and checks cleanup before releasing IPC.
See [process lifecycle](process-lifecycle.md). The Windows core passes native
fixture E2E, including Windows 11 terminals and Undo. The user reports real local Claude, Windows SSH and installation/upgrade acceptance;
fixture coverage and platform-specific limits remain distinct (see Development).
Windows Service deployment is unsupported.

There is no per-project or per-session daemon. Multiple sessions can have
independent Claude processes. The explicit state-directory option creates an
isolated instance for testing or a separate installation, not ordinary projects.

Unix directory mode is 0700 and socket mode is 0600. Windows ordinary state inherits
parent ACLs; control/private leaves have the stricter policies in [IPC](ipc.md) and
[Provider storage](providers.md). There is no public TCP listener.
SSH uses the user's OpenSSH configuration and host-key verification. The desktop
can select a local identity file or provide a password through OpenSSH askpass;
the latter remains in process memory for the current app run and is never saved
in `hosts.json`.
Remote programs are shell-quoted and Host input rejects option injection.
Local and remote hosts use exactly the same command and protocol handlers.

## Durable lifecycle

A send transaction records a request ID, input event and Running state before
spawning. Repeating the same ID and body returns an accepted duplicate, without
launching another process. Conflicting reuse fails. Each session has at most one
active turn; different sessions can run concurrently.

Each opened conversation owns a persistent, supervised CLI process group. Opening
initializes or resumes its native session without sending a prompt. Subsequent
turns share the same input/output stream; a native result ends one turn and never
closes stdin or background subagents. Background completion can start an autonomous
follow-up, which is persisted without inventing or replaying a user message.
Cancellation interrupts the current turn through the adapter's native control
protocol. Only explicit Close or graceful App exit releases the process group;
transport loss and conversation switching do not. Unexpected native process loss
reopens its persisted native ID without replaying the interrupted prompt. This
restores history, not lost background execution; failure remains explicit.

Configuration changes can replace an idle process and resume its native history.
Native task/session lifecycle evidence must establish that no background work will
be discarded; otherwise the change is rejected before accepting a prompt. Older
CLIs without lifecycle evidence are treated conservatively after delegation.
An initialization acknowledgment is required before sending input. Stdout frames
are bounded to 1 MiB, stderr is drained, and initialization, writes, active turns
and shutdown have deadlines. Per-host retained sessions are bounded to 64; reaching
that limit rejects opening another session, never evicts an existing one.

Events have monotonic sequence numbers and are stored on the host. First display
uses bounded reverse history windows with adjacent text chunks compacted in transit;
exclusive raw cursors retain complete ordering when older windows are loaded.
Desktop polls
incrementally and deduplicates replay after reconnect. A completed turn requires
the native result event; malformed output or premature exit fails. On daemon
restart, previously active sessions become Unknown, never Completed. Host crashes
cannot guarantee cleanup of all descendants or recovery of an interrupted tool;
users must reconcile the project before continuing an Unknown session.

Permission requests are projected with fresh, opaque, single-use IDs. Only the
active session's control channel can resolve them, using the original tool input.
The browser cannot supply replacement tool inputs. Existing CLI permission rules
are honored; Latte Work is not a sandbox around Claude.

## Inspection and frontend

The conversation uses a presentation projection over durable protocol events.
Tool calls and results are paired by ID within a user turn; consecutive tools
share a collapsed, borderless summary. Assistant text separates activity groups,
and failures and approval controls remain outside them. Expanded operations
show both input and output in one bounded, scrollable region. Missing results
after interruption are marked unconfirmed, never inferred successful. Unmatched
results remain inspectable. This changes neither host history nor wire types.

Read-only file operations reject traversal and canonical symlink escapes and cap
text output. They are designed for trusted project directories, not as a hardened
filesystem sandbox against concurrent malicious renames. Git disables external
diff/textconv/fsmonitor and optional locks. Untracked paths appear in status;
untracked file contents are available through file preview.

The frontend never executes shell commands. Generated TypeScript protocol types
come from Rust. React renders Markdown without raw HTML, and CSP disallows remote
scripts and frames. Host connection failures do not imply agent failure. Only
server state can mark a task completed.

## Protocol contract

Each connection must first send `hello { version: 1 }`. Version mismatches fail
before commands. A line is one JSON request or response; requests on a connection
are serialized, while separate hosts/connections run independently. Application
errors are typed response envelopes, distinct from broken transport errors.

The current development protocol baseline is v1. Feature work does not automatically
increment this number; incompatible contract changes need an explicit versioning decision.
Use desktop and host server builds from the same source revision. The v1 label alone
does not establish compatibility with earlier development snapshots.

Project creation selects a host and directory together. The sidebar merges host-scoped
project catalogs and caches them for offline display; server state remains authoritative.
Local folder selection uses the native Tauri dialog. Remote selection uses the
`browse_directories` request (directory metadata only, absolute paths, bounded output).
Project file previews still enforce the registered project root. Desktop and host
server must implement the same protocol contract; version mismatches fail the handshake.
An unsaved task draft selects a host-scoped project before its first send. The
desktop connects to that project's host and sends `create_session` and `send` to
the same host with the selected project ID. A disconnected host disables send;
switching project does not move an existing session or persist an empty draft.

The protocol supports project rename and reversible removal. Removal hides a registered
project from the catalog without deleting its directory, sessions, or events;
adding the same canonical directory restores its existing identity and history.
Projects with running or waiting sessions cannot be removed. Pinning applies to
sessions, as described below. The desktop owns project context menus and suppresses
the WebView's default menus; Finder reveal is local-only and
resolves a registered project ID through the server before opening its directory.

The protocol supports host-owned model catalogs and a per-turn `send.model` selection.
Provider model lists travel with the existing SSH snapshot. The server validates
selection before accepting the request and only overrides that turn's launch
snapshot. Session JSON retains the selection; the requests table migration adds
a nullable model column so duplicate detection includes model identity while old
requests remain valid. Legacy Provider records default to an empty extra-model
list, with their existing model still available as the default.

## Session organization

Each host persists session title, explicit-title flag, pin timestamp, manual unread
flag, and archive flag in the existing session JSON. Legacy records default to
unarchived/unpinned/read. Organization requests run under the existing database
lock; no agent-native transcript or working directory is modified. Archive rejects
active runs, clears the pin, and prevents new sends until restored. Restore does
not re-pin. The pinned-session endpoint excludes hidden projects and caps pins at
100 per host. Desktop combines saved hosts into one global pinned section ordered
by pin time, retaining the original project row and namespacing IDs by host.

The desktop refreshes pinned sessions and the selected project's list every five
seconds. Its local pin cache preserves shortcuts while a host is unavailable;
mutations are acknowledged by the owning server. Read acknowledgement follows
[sidebar activity and unread state](#sidebar-activity-and-unread-state). Archived records are accessible from the
project's archived-list toggle and remain read-only until restored. Markdown copy
loads paginated history with size/time/page bounds and copies user/assistant text,
without raw tool payloads; exceeding a bound reports an error rather than silently
copying a truncated conversation. The last fetched page defines the live snapshot.

## Per-turn reasoning effort

`send` and `Session` carry nullable, enum-validated `effort` (low, medium, high,
xhigh, max). Null requests automatic model effort. SQLite migrates the requests
ledger with a nullable effort column; duplicate request identity includes effort.
Legacy session JSON and request rows retain null. `models` accepts an optional
selected model and returns adapter effort levels. Known unsupported models are
rejected before accepting a turn; unknown custom model IDs defer to the CLI.
Both desktop and remote server builds must implement the effort fields. The desktop never launches or controls the CLI directly.

The model catalog also accepts an optional registered `project_id` and returns
optional/default-empty `model_labels`. These are additive display metadata:
older requests and responses remain valid, older Hosts fall back to raw model IDs,
and selection/send semantics are unchanged. Native labels are resolved on the
execution Host only when no Provider is bound; the UI does not read host settings.

## Interactive terminals

The protocol supports host-owned PTYs (`terminals`, `create_terminal`, `read_terminal`,
`write_terminal`, `resize_terminal`, `close_terminal`). Local and SSH use the same
requests. The server starts the host user's default login shell on Unix, and pwsh
on Windows, in a registered project directory; the desktop only renders UTF-8 bytes
with xterm.js. A terminal
is a full user shell, not the read-only file preview or an agent approval channel.

Creation uses a client UUID, is idempotent for that live terminal, and rejects
reuse in another project. On a lost creation response, the UI lists terminals
instead of spawning another shell. Input is serialized, bounded, and never
replayed on transport failure. Unknown/partially delivered input is shown explicitly.
The output cursor counts bytes; xterm's streaming decoder handles split Unicode.

Each host permits 16 open terminals (including exited tabs), each with a 1 MiB
output ring and 64 KiB read batches. Overrun is explicit. Input frames are at most
16 KiB, with bounded UI/server queues and a 2-second write deadline. Unix uses
nonblocking PTY I/O; Windows isolates synchronous ConPTY I/O in a Job-owned helper.
Dimensions are limited to 2–500 columns and 1–300 rows. Close retains the registry
entry until cleanup is confirmed; failure remains retryable and vetoes Undo/upgrade.
Owned-resource batches close concurrently. Unix hangs up the shell and kills its
foreground job with a bounded kill fallback; deliberately detached Unix processes
retain ordinary shell semantics. Windows closes the entire owned Job, including
remaining descendants after the root exits. See the platform-specific terminal and Undo rules below.

Sidebar state belongs only to the conversation ID. Local storage retains its
ordered tabs, selection, file location, visibility, expansion and width. First
open and closing the last tab both show the empty launcher. Draft conversations
have independent IDs; the first send copies their sidebar state to the persisted
conversation. Tabs retain their original host/project resource bindings when the
draft's target changes. Host/project are not part of the sidebar persistence key.

Tabs/emulators survive hiding and conversation switches. Host PTYs survive
client/SSH bridge disconnection. Reopening restores only that conversation's
saved terminal IDs; listing a project never imports other conversations' terminals.
They are ephemeral: output is not stored in SQLite, and a daemon restart never
recreates shells or replays commands. Closing a tab explicitly stops its PTY;
normal daemon shutdown closes all terminals. Remote hosts need a matching
server; version mismatch remains an explicit connection failure.

### Sidebar activity and unread state

The Host marks a session unread atomically with its transition from active to
completed, failed or stopped. Read acknowledgement is explicit and durable.
The desktop acknowledges the selected conversation after reading its latest window
and catching up with incremental polling while the window is visible and focused.
Older compatible servers replay all available events before acknowledgement. Background completions
remain unread. The sidebar distinguishes running, waiting for approval and unread
states; collapsed projects aggregate running/waiting state. Other project lists
are refreshed serially every five seconds without launching agents or reconnect
prompts, so collapsing a project does not stop its status updates.

## Remote Provider execution

The App-owned `latte-work-config` store owns Provider definitions and associations
keyed by desktop host ID and Agent ID, without Agent registry or Server dependencies.
Desktop association changes do not contact or mutate the remote Provider catalog.
Provider CRUD uses native App commands and works with every Server offline.
The legacy local Provider file is read once without starting its daemon, and remains
untouched. Configuration transactions use an OS file lock and atomic replacement.
For model catalogs, the native client reads the selected Provider and forwards only
its metadata to the execution host. For each send it resolves the session's Agent,
reads a fresh selected snapshot and attaches it to that send over the existing target
connection. The WebView cannot export credentials or receive snapshot responses.

Both local and remote turns explicitly choose a snapshot or their target CLI configuration, overriding any
legacy saved binding. Only the selected host resolves native model names and defaults;
no request connects to another environment first. The receiving host validates protocol, models and effort,
then persists the request before spawning. Snapshots are held in native memory and
CLI temporary settings only, never in the remote Provider catalog, request ledger or
events. Existing remote Provider data is preserved; migration does not delete it.

The request ledger stores a SHA-256 digest of the explicit turn configuration alongside
text, model and effort. Exact retries return the original acceptance; reusing an ID with
a changed snapshot, including changed credentials, fails. Reconnect does not replay a
send automatically. Daemon failure still produces Unknown state. Existing request rows
have a null digest and preserve their original deduplication behavior. Deploy desktop
and host server from the same source revision so both implement the turn override.

UI history reads use a persistent transport independent of slow Agent control and
CLI metadata. Local and SSH hosts share the same request routing and connection
lifetime; only bridge creation and network latency differ.

## Native paths and Windows snapshots

Host absolute paths remain native `Path`/`PathBuf` values. Protocol-relative paths
are encoded from components with `/`; Unix backslashes remain filename characters.
Paths that cannot be represented in a required UTF-8 protocol field fail explicitly.
Git path metadata is decoded strictly; lossy display previews never become paths.
Windows rejects drive-relative, UNC/device, ADS, reserved DOS and trailing-dot/space
aliases, and checks reparse points before canonical containment. These rules also
apply to attachment names; external references retain explicit reference authorization.
Git status and review stay within the registered project, including nested projects.
Git's `/dev/null` diff sentinel is distinct from a filesystem path; isolated global
Git configuration uses an actual empty temporary file.

Persisted root identity keeps Unix `[device,inode]` serialization unchanged and adds
`windows_v1` with volume serial and all 128 file-ID bits. Platform identities cannot
match across systems. `windows_files_v1` records file identity, owner/group/DACL and
supported attributes instead of fabricating Unix modes. Content and metadata are
read from the same opened object and checked for changes. Legacy Windows history
remains readable, with an explicit `undo_unavailable` reason when restoration metadata
is absent; current files cannot supply missing historical metadata. Legacy non-Git
snapshots that did not assess Windows Undo eligibility remain explicitly unverifiable;
this capability does not make every historical snapshot undoable.

## Windows terminal backend

The shared terminal registry owns project/UUID validation, queues, output cursors
and create/close confirmation. Windows starts the same Server's internal `__terminal`
helper suspended, assigns it to a non-inheritable kill-on-close Job, then resumes it.
The helper creates ConPTY/pwsh and exchanges bounded stdio frames without another
listener. Blocking console I/O and close remain isolated in that disposable helper.

Only PowerShell 7 is supported: discovery uses absolute PATH directories and the
standard installation location, never the project cwd. Missing `pwsh.exe` fails
explicitly; no Windows PowerShell/cmd/Git Bash/WSL fallback occurs. Interactive pwsh
uses `-NoLogo` and its user profile; Agent environment capture still inherits Server
variables without profiles. Test-fixture builds alone may disable profiles. Shell cwd
converts local verbatim-drive spelling for pwsh without changing file authorization.
The helper clears its inherited ignore-Ctrl+C flag; Ctrl+C remains terminal input,
and bytes lost while the shell interrupts are never replayed. Raw UTF-8/VT forwarding
does not guarantee that every console program uses Unicode output encoding.

Close bypasses the input queue and continues draining output. Success requires helper
exit and an empty Job; blocked cleanup forces the Job closed. A close call waits at
most four seconds. Failure keeps registry ownership and remains retryable; caller
cancellation cannot establish closure. Disconnection retains the terminal, while
daemon restart never recreates it or replays commands. Windows 10, other shells,
user-profile variations, full-screen TUI/IME interaction and long-running concurrency
need their own evidence rather than being inferred from fixture success.

## Per-turn restoration and Windows Undo

A request is persisted before its baseline is captured and native dispatch begins.
Frozen before/after bytes drive historical turn diffs, independently of HEAD and later
edits. Capture is bounded to 1,000 files, 512 KiB per file, 8 MiB total and 12 seconds;
Windows permission metadata has separate 64 KiB/file and 8 MiB total limits.
Incomplete collection and unfinished background children cannot become a complete
undoable result. Attribution is temporal: independent writers may appear in changes.

Undo requires the latest complete turn, no active host tasks/background work/open
terminals, and exact stored after-image matches. Host admission blocks new writes;
overlapping idle Agent runtimes close first. The owned admission guard stays with the
blocking restore worker through preparation, publication and durable outcome, even
if its caller disconnects. Git index and refs remain unchanged.

Windows supports additions, modifications and deletions of eligible local NTFS regular
files in Git and non-Git projects, including empty/binary/CRLF content. It restores
saved before bytes, restorable owner/group/
DACL protection/inheritance, and readonly/hidden/system/archive attributes. It refuses:

- Missing/partial/legacy restoration metadata, metadata-only turns, non-latest turns,
  or unfinished/background/terminal resources.
- Network/mapped or non-NTFS volumes, reparse/junction/symlink paths, ADS, multiple hard
  links, short-name aliases, `.git` aliases, missing parents and unsupported attributes
  such as compression/encryption.
- Changed after bytes, identity, ACL or attributes; occupied or readonly targets; or
  before metadata that cannot be recreated with the current user's ordinary rights.

No elevation, ownership takeover or temporary ACL widening is attempted. File IDs,
timestamps, SACL auditing, empty directories and whole deleted directory trees are
not restored. The Windows directory identity comparison recognizes casing aliases.

Git recovery continues to use `<gitdir>/latte-work-undo/<attemptUUID>` and the existing
`windows_undo_recovery_v1` intent. This does not classify every Git recovery root as
private managed storage. Non-Git Undo lazily creates a separate private namespace
at `<project>/.latte-work-undo`, with one child per attempt UUID. A v2 registration
in the daemon's SQLite store and a flushed `owner.json` manifest (at most 64 KiB)
must agree on kind/version, store UUID, project/namespace paths and native identities.
Protected ACLs and retained directory handles are also verified. Registration commits
before attempt creation and before-image staging; the non-Git durable intent uses
`windows_undo_recovery_v2` and includes the managed registration.

Only verified registered managed roots are excluded from live capture, file listings
and workspace diffs, including Git force-tracked paths and both rename endpoints.
Identity checks cover casing aliases and overlapping projects; merely naming an
ordinary directory `.latte-work-undo` does not hide its contents. Frozen historical
turn patches and Git commit/blob views keep their existing semantics.

The registry and manifest prevent accidental collisions; they do not defend against
a malicious same-user process. Existing namespace collisions, identity/manifest
mismatches and recognizable orphaned stores fail closed. A lost or alternate daemon
database cannot adopt an existing namespace. No automatic repair, adoption, deletion
or garbage collection occurs. Attempt directories and displaced originals are kept
after both success and failure.

Restoration pins the root and parents, validates every after-image, and prepares all
same-directory before-image files before touching targets. A staged file receives its
recorded security and exact readback before content is written, then attributes/flush
and metadata verification. Preparation failure removes only owned temporary files.
Before restoration mutates any target, the worker durably records **Unknown**.
It retains each validated source handle, moves the original into its same-volume
recovery attempt through that handle, then publishes the verified before-image with
no replacement. Undo of an added file leaves its original in recovery and verifies
that the project path is absent. Undo of a deleted file also uses no-replace
publication, so a newly occupied destination cannot be overwritten.
Only complete restoration and readback records **Reverted**. Repeated successful Undo
returns the saved outcome without overwriting later edits. Failure/restart stays
Unknown, with no automatic retry or rollback. Preparation/publication each have a
12-second budget checked at file boundaries, not a promise to interrupt kernel I/O.

Retained source handles reject ordinary writers and deleters; independent external
editors are not globally locked. An editor can occupy the destination after displacement,
in which case publication fails without replacing that file and the displaced original
remains in recovery. Multi-file Undo is nontransactional: failure may leave only some
files restored. It does not recreate deleted directory trees. A killed worker may also
leave `.latte-undo-*` staging files with the restored permissions; users must inspect
these and the retained recovery attempt alongside the Unknown workspace. Restart
neither cleans them automatically nor resumes the operation.
