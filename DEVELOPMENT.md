# Development

Requires Rust 1.97, Node 22+, macOS Command Line Tools. `make ci` also
requires actionlint and ShellCheck (install with your package manager). Host server supports macOS
and Linux; v0.1 desktop is validated on macOS. No Windows runtime claim.

`make setup`, `make dev` starts the native application. `make ci` runs the local
gate; `make package` builds an app without Developer ID signing and standalone release host server,
and on macOS also produces the verified drag-install DMG described below.
Use `make build` for a debug native app bundle. On macOS, both build modes
finish with project-owned ad-hoc signing and strict bundle verification. The CLI fixture never calls a model.

## CI and merge checks

`make test-unit` runs Rust crate-local and frontend reducer tests.
`make test-e2e` runs the final server binary through its real socket bridge with
a deterministic Claude CLI fixture. `make test-doc` runs Rust documentation
tests. On macOS, `make test-native` compiles and runs the standalone WebKit
site-data boundary and localhost favicon transport harnesses. They use a disposable
cookie store and loopback HTTP fixture, without browsing, login state or model calls.
`make test` runs all three portable layers and includes `test-native` on macOS;
`make ci` also checks formatting,
generated protocol types, Clippy, workflow/shell scripts, the frontend build,
and Rustdoc. `make setup` uses public npm registry URLs recorded in the lockfile.

GitHub Actions runs host check/Clippy, UT, final-binary E2E, and release builds
on Linux and macOS. Frontend UT/build runs on Linux, macOS, and Windows;
Windows also checks, lints, and tests the portable Rust protocol and client.
The server uses Unix sockets and process groups, so Windows CI does not claim
server or native desktop support. A separate macOS job runs `make test-native`
with a three-minute step deadline before building the native app. A failed native
test fails that job and therefore `PR Gate`; successful compilation alone is not
treated as native behavioral coverage.
`PR Gate` fails when any dependent job fails, is cancelled, or is skipped.
The repository ruleset must require `PR Gate` on `main` for this to block a
merge; a workflow file alone does not activate that remote rule.

Like Latte Code, dependencies and lint policy live in the Cargo workspace,
Make is the stable command surface, and Cargo/npm lockfiles are checked in.
Debug information is reduced; release builds use thin LTO. If sccache is on PATH,
Make enables it automatically. Do not globally install tools during setup.
Cargo target defaults to each checkout's target/ (avoids competing worktree
locks). Override CARGO_TARGET_DIR for a dedicated persistent build volume. CI
caches Cargo dependencies and npm content, separately per OS/toolchain/lockfile.
`make clean` never purges sccache, the Cargo registry, npm cache or runtime data.

The server owns projects, sessions, SQLite events and agent children. Exactly
one daemon holds an exclusive file lock per canonical state directory (the
standard user state directory is the default). Clients use a private Unix socket;
SSH runs the same binary's `connect` bridge. The bridge starts the daemon when
absent, then forwards bytes. Disconnecting a bridge does not stop the daemon.
Native `host_request` failures from a broken client carry the private
`HOST_CONNECTION_LOST:` prefix (not a wire-protocol change). The WebView reports
that host as disconnected even for metadata requests, while Server/application
errors remain scoped to their controls. Reconnecting replaces the transport;
late failures from an older connection cannot invalidate its replacement. User
messages are never automatically replayed.
Explicit state-directory overrides exist for tests and separate installations.

The desktop's local transport uses `connect-local`: it compares the running server's
executable SHA-256 with the bundled binary and requests an idle shutdown before
starting the new build. This identity is separate from wire protocol v1. The
private `server_status` / `prepare_upgrade` lifecycle handshake is available before
Hello and is not a WebView API. The latter binds to a server instance and closes
request admission atomically; running/waiting tasks and any open terminal veto it.
Local coordinators serialize via `upgrade.lock` and time out after 20 seconds.
SSH still uses `connect`, with no automatic remote replacement. Legacy daemons
without the lifecycle handshake require a one-time manual restart after tasks
finish and terminals close. Restarting only the desktop leaves them running.

Wire protocol changes: edit latte-work-protocol then run `make types` and
`make fmt`. Desktop presentation must not import or reproduce Claude wire types.

Archiving an idle task first closes any retained native session, reaps its owned
process group, pauses pending messages and ends background subagents. Only a
confirmed close allows the archive flag to be saved; failures keep it unarchived.
Archive shares the exclusive execution-admission gate with undo, so another client
cannot reopen or send between close and persistence. Archived tasks cannot create
actors through queue actions either. Unarchive changes metadata only and does not
open an Agent; running/waiting tasks must still be stopped before archiving.

`recent_sessions` reads unarchived history across visible projects on the target host,
100 rows per page with an activity-time/session-ID cursor. Activity comes from saved
conversation events; opening/reading, lifecycle state and pin metadata do not reorder
history. Older databases are supported through an indexed event projection. Local and
SSH use the same request; older servers fail explicitly and should be updated.

Read docs/architecture.md for boundaries and docs/remote.md for SSH installation.

The desktop icon source is `apps/desktop/src-tauri/icons/source.svg`. Run
`make icons` after editing it, and include the generated PNG and macOS ICNS.
Keep the rounded tile within the 80% canvas safe area for consistent Dock sizing.

Provider protocol smoke tests can use a real installed Claude CLI with disposable
localhost upstream fixtures: `python3 scripts/smoke-providers.py --binary /path/to/latte-work-server --claude /path/to/claude`. This optional native check
uses no paid model API; the automated `make ci` suite uses deterministic fixtures.
See docs/providers.md for module boundaries, persistence and compatibility limits.

## Sidebar terminal development

The terminal uses the v1 protocol contract on both the desktop and host. For isolated
native tests, a debug desktop accepts `LATTE_WORK_SERVER` pointing at a wrapper
that invokes the checkout's server with `--state-dir` in a temporary directory.
Do not replace a user's running daemon to perform smoke tests.

Focused checks: `cargo test -p latte-work-server --test e2e terminal_ --locked`
and `npm test` (frontend includes PTY output-cursor and no-input-replay
regressions). Native checks should cover Tab switching, panel hide/restore,
multiple shells, resizing, shell exit, and keyboard copy/paste.
Confirmed shell exit automatically closes its workspace tab and releases the host
terminal through `close_terminal`. Live output is drained before closing; restored
exited metadata also retires the tab. Disconnects and read failures keep the tab,
and exiting a nested shell does not close a still-running parent shell. Failed
cleanup remains visible for explicit retry rather than automatically replaying it.

## Agent adapter development

`crates/latte-work-server/src/agents/adapter.rs` defines the internal contract;
`agents/mod.rs` registers implementations, and `agents/claude.rs` implements the
only supported Agent. Keep protocol order and wire messages in the implementation.
Runtime consumes common actions and retains approval authorization, process
supervision and persistence. Each opened conversation has a persistent actor and
native process. The additive `open_agent_session` and `close_agent_session`
requests do not send prompts; they share the local/SSH implementation. Runtime
serializes native output, durable turn admission, interruption and close. Native
results finish turns without closing stdin. Task lifecycle and session-state
messages stay in the Claude adapter. Configuration changes are rejected before
admission when background work cannot safely be released. `Ready` must never implicitly send a prompt.

Subagent roles are optional `agent_type` metadata in both updates and snapshots.
The Claude adapter reads the explicit native `subagent_type` field from Agent/Task
inputs or lifecycle updates; descriptions, tool names and `task_type` are not roles.
Store merges preserve the role across native identity changes, progress, close and
restart. Older records and Servers may omit it; the UI leaves it absent.

Focused checks: `cargo test -p latte-work-server --bin latte-work-server --locked`
for contract/adapter/runtime UT, followed by `make test-e2e` for real server/bridge
behavior with the deterministic CLI fixture. These do not measure coverage or
validate a live model service. Internal interface changes do not require a wire
protocol version increment.

## Composer references and native Agent commands

`agent_commands` is an additive protocol request scoped to agent and project. The
host launches a metadata-only adapter handshake with a 12-second deadline, bounded
frames/message count and at most two concurrent probes. No user prompt or Latte
session is created. Process groups are terminated after discovery, failure or timeout.
Only the Claude adapter knows the initialize response's command schema. Commands
are checked again against the actual turn's initialize response before sending the
original prompt, avoiding Claude's unknown-command fallback to an ordinary model turn.
The existing send request ID, persistence, resume, cancellation and approval flow are
shared by native commands. Terminal-only commands absent from the native SDK catalog
are not synthesized. Agent command effects and configuration scope remain native;
Latte's explicit model/effort selections continue to apply on each launched turn.
App-owned `/agents` (and listing aliases), `/model` and `/effort` are resolved before
native command discovery or turn admission. The latter two open the existing model
selector or effort submenu; they never change native configuration via a CLI command.
These names are reserved across all Agents to keep one App navigation contract;
unsupported selections report their capability error rather than forwarding the
reserved name to a different native CLI.
Arguments are retained in the draft with a menu-selection notice. A disabled/unready
selector reports its reason without clearing the draft or stopping a running turn.
Namespaced project commands remain native. The workbench send boundary also rejects
model/effort commands if a caller bypasses composer navigation.

The `@` picker reuses the server's bounded, canonical project-file listing for local
and SSH hosts. It attaches relative path references for the Agent to read, not file
uploads or clipboard screenshots. A remote server must be updated for command discovery.
The `+` menu also accepts explicitly chosen files/folders outside the project:
native pickers are local-only, while manual absolute paths are resolved by the
selected execution host through `resolve_reference`. This bounded metadata-only
request returns a canonical absolute reference (or a project-relative path when
contained). It does not read contents, add allowed directories, change Agent
permissions, or relax `files`/`read_file` canonical containment. Remote hosts need
an updated server for this additive request. Late selection results are discarded
after project, host, session or composer state changes.
Focused tests: `npm test`, adapter/runtime unit tests and the final-binary
`agent_commands_discovery_dispatch_resume_and_unsupported_are_native` test. A real
CLI smoke check should separately verify discovery and a non-model native command.

## Remembered Agent selections

The desktop persists explicit model and effort choices per host / Agent in local
storage. New drafts restore those choices across projects and restarts; existing
sessions retain their own parameters. Reading a catalog or falling back from an
unsupported effort is not an explicit preference change. Selecting native defaults
persists null; missing effort leaves both CLI arguments and native effort environment
untouched, including when a Provider is bound. Claude resolves its own settings.
Focused checks: `useAgentPreferences.test.tsx`, `ModelPicker.test.tsx`, and the
final-binary `effort_is_applied_on_launch_resume_and_reset` test.

## Agent permission modes

The composer requests `agent_permissions` from the execution host. Each adapter
owns discovery, labels and launch mapping. Claude probes the installed CLI's
`--help`; when a flag or its choices are hidden, an invalid-value `--permission-mode`
probe with `--help` reads the CLI parser's own allowed choices without starting a
session or sending a prompt. Each subprocess has 5-second / 128 KiB combined
stdout/stderr limits and shares the probe concurrency limit. Only known modes
explicitly listed by the installed CLI are offered; help omission or an exit code
alone never implies support. See the [CLI reference](https://code.claude.com/docs/en/cli-reference),
which notes that `--help` does not list every supported flag. Account, model and managed-policy eligibility is
still enforced by the native Agent; rejection is surfaced, never replaced by a
different mode. Unsupported adapters fail explicitly.

A nullable `permission_mode` travels with each send, is stored with the Session
and participates in durable request identity. Null omits the CLI override.
Explicit choices are remembered per host / Agent; running turns cannot be changed.
Only the native CLI decides which tools require approval. Requests that reach the
host approval flow retain its single-use session-bound checks, even in bypass mode.
Permission changes do not edit native settings or expand sandbox/network policies.
The Hello capability `permission_settings` prevents a new client from sending
an override to an older server that would silently ignore the field. Remote hosts
must update their server; local and SSH transports share the same implementation.

## Clipboard attachments

Composer paste snapshots text/files before awaiting native work. Plain text stays
at the selection; more than 8,000 UTF-16 code units or 200 lines becomes a UTF-8
`.txt` attachment. File URLs from the macOS pasteboard are handled by native code;
a conflicting text snapshot takes precedence over a subsequently changed clipboard.
Images/video/documents are file references, not simulated vision/video processing.

The desktop stages all pasted bytes in the local host's private `paste-tmp/`
directory under its state directory. An SSH task then receives a streamed copy in
that host's own `paste-tmp/`; only the destination path is attached. The native
adapter does not run Agents or change their permissions. macOS file URLs are
revalidated against the current pasteboard before opening. Local pasted directories
are references only; remote directory transfer is explicitly unsupported.

The additive `begin_attachment` / `attachment_chunk` / `finish_attachment` /
`abort_attachment` protocol bounds chunks to 64 KiB, each file to 64 MiB, concurrent
uploads to 8, and each host's managed cache to 2 GiB / 4,096 entries. Offsets and
declared sizes must match. Files become visible only after complete writes and
fsync; private cache directories use 0700 and files 0600. Incomplete in-memory
uploads expire after ten minutes on subsequent admissions; completed files remain
available for saved prompts and retries. They are not automatically deleted on
session close. Remote servers must be updated before using clipboard transfers.

Tests: `pasteAttachments.test.ts`, `Composer.test.tsx`, server attachment unit tests,
and final-binary `pasted_attachments_are_chunked_host_owned_and_abortable`. Report
native clipboard tests and actual remote-host execution separately.

## App-owned Provider configuration

`latte-work-config` owns App Provider definitions and per-host/Agent choices. It has
no runtime Agent or Server dependency. Tauri accesses it on a blocking worker using
a short-lived OS file lock. UI Provider CRUD must use `provider_request`, never
`host_request` or a local Server connection. Only model discovery and execution
contact the selected host. Each desktop send chooses an explicit snapshot or CLI
configuration, including for the local host. See `docs/providers.md` for read-only
legacy migration, paths and confidentiality requirements.

## Read-only change inspection and presentation metadata

`changes` returns bounded, project-relative `GitChange` entries grouped by staged,
unstaged and untracked status. `change_diff` returns one file's bounded content;
tracked files use literal Git pathspecs with external diff and textconv disabled,
and untracked previews use the existing canonical project containment checks.
The legacy `diff` request remains available, with status scoped to the same project
as its patches. New clients explicitly report unsupported requests on older
servers; use a matching remote Server revision for the structured view.

Approval events may include optional `tool_use_id` metadata from the adapter.
It only links presentation to a known tool call. Authorization remains bound to
the existing public approval request ID, session and single-use runtime decision.
Histories without this metadata still display explicit approval outcomes, but do
not guess which parallel tool an old decision belongs to.

## macOS window and application lifetime

The red close button hides the window and preserves its WebView and drafts. Dock
reopening restores and focuses it. A small macOS Objective-C delegate hook routes Cocoa Dock termination through
the event-backed Quit menu because Tao 0.35 does not expose a cancellable
applicationShouldTerminate callback. It adds only that missing method and leaves
Tao window/reopen handling intact; Rust contains no unsafe code.

Dock Quit and Command-Q silently close resources opened or sent by this App
instance. A dedicated transport bypasses local upgrade checks and UI polling;
cleanup batches are idempotent and bound to both the daemon ID and App owner ID.
The host closes and reaps the Agent process group before acknowledging. An App
reclaim fences stale closes from an earlier App instance. New requests are blocked
while quitting; admitted requests cannot send new work after claiming a resource
if Quit has begun. Prompt input is never replayed.

Quit persists a private, atomic resource record before closing. The UI is never
blocked by a quit-failure dialog: cleanup has an 8-second overall deadline, then
the App exits. Unconfirmed work is not reported as stopped. Tab cleanup is advisory,
with a one-second budget; native resource ownership remains authoritative. Local
unconfirmed closes are resolved before the next local connection is exposed.
A disconnected remote server keeps executing and retains its Agent sessions:
it cannot infer App Quit from SSH EOF. On the next connection, the App reattaches
to surviving remote resources and fetches their actual state; it never replays an
old remote close, Open or Send. If the daemon has restarted or a resource belongs
to a newer App instance, the old record is discarded without touching that resource.
Records contain host targets and resource IDs, never SSH passwords, prompts or
Provider credentials. This lifecycle requires an updated server supporting native
resource ownership. Busy/shared daemons are not force-killed. OS force kill/crash
cannot run explicit cleanup and remains outside this graceful-exit guarantee.

Explicit Quit first emits a native `app-close-requested` transaction. Every mounted
Tab registers `useAppClose`, including hidden tabs: terminal panes await PTY close;
file/change panes pause refresh. Tabs own their cleanup; unmounting or hiding a
window is not a shutdown signal. Tab callbacks have a one-second advisory budget;
stale acknowledgements cannot approve a later Quit. Native ownership tracking
remains authoritative, blocks new work, and retains ambiguous terminal creation
records. Cleanup failures are diagnostic only and retain unconfirmed resource
records without opening a dialog. Within the eight-second overall budget, native
cleanup closes any remaining App-owned terminals and Agent sessions before
dropping connections; unreachable remote resources remain available for adoption
on the next connection.

The local daemon receives the existing instance-bound idle/drain handshake after
cleanup; busy/shared resources veto daemon exit without being killed. Remote daemons
remain alive. No unrelated sessions or terminal IDs are enumerated and killed.
App transport pipes close on exit; connect-local and SSH bridges end on EOF.
Reopening reconciles events without replaying prompts.

## macOS drag-install packaging

- `make package`: build the release server and desktop, ad-hoc sign, then create the DMG.
- `make package-dmg`: package the existing signed release app without rebuilding it.
- Output: `$CARGO_TARGET_DIR/release/bundle/dmg/Latte-Work-<version>-<arch>.dmg`
  and the adjacent `.dmg.sha256`. The default target directory is this checkout's `target/`.
  Version and architecture come from the app bundle and Mach-O binary, not hardcoded release names.

`scripts/package-dmg.sh` provisions a private Python venv in the target directory
using `scripts/dmg-requirements.txt` (pinned versions). macOS, Python 3 with venv/pip,
Command Line Tools and network access on first use are required. It never installs
global Python packages; unchanged dependencies are reused offline.

`scripts/package-dmg.py` owns the 600×400 white installation window, large app and
Applications icons, and center arrow. All staging and alias paths are canonicalized
before creating Finder metadata, avoiding the `/var` versus `/private/var` alias bug.
It validates the background reference, arrow pixels, icon positions, hidden window
chrome, Applications symlink, image checksum and strict bundle signature. It remounts
the compressed image read-only at a different path, checks the layout again, and
compares the desktop/server binary hashes with the source bundle.

The existing output is replaced only after validation succeeds. Temporary mounts
are detached on failure; failed staging directories are retained for diagnosis.
Run packaging serially within a checkout: do not run `make prepare`, debug builds
or another packaging process concurrently, since they share the bundled server.
Native Finder visual acceptance is separate from structural validation: double-click
the resulting DMG and check the arrow and icon layout. Opening its directory in an
existing Finder window can inherit that window's view settings. No Apple notarization
or Developer ID signature is implied.

## Context and usage telemetry

The additive `usage` event is persisted and replayed with normal session events,
shared by local/SSH transports; update the remote server to receive it. Old
histories remain unknown. Presentation filters telemetry before merging streamed
text. The composer exposes one neutral ring with a click/keyboard statistics panel.

Claude assistant input + cache-read + cache-write tokens describe the latest
main-agent request input snapshot, not a live tokenizer or cumulative context.
Per-step output tokens are placeholders and are excluded. Result `modelUsage`
provides contextWindow for the reported model or the explicit main-agent model
resolved at launch/init, allowing Provider aliases that differ from API model
names. Exact identities take precedence; normalized aliases must match uniquely.
Unrelated billing entries and subagent totals are never used for context occupancy.
Changing the main-agent model invalidates the previous capacity. Compaction/reset
invalidates the snapshot until a subsequent request. Result usage and API duration
are displayed for the latest submitted turn; sending clears those totals.
Cache hit rate = cache-read / (uncached input + cache-read + cache-write).
Missing capacity/counters remain unknown, including older CLI/provider responses.
Context breakdown, tool duration, TTFT and generation TPS are not fabricated from
wall time. Sources: [SDK cost tracking](https://code.claude.com/docs/en/agent-sdk/cost-tracking)
and [SDK wire types](https://github.com/anthropics/claude-agent-sdk-python/blob/main/src/claude_agent_sdk/types.py).

The additive `progress` event carries only waiting/thinking/replying phases.
Claude thinking block/delta events map to thinking without storing their text;
phase changes are deduplicated and subagent phase events ignored. See the
[streaming event contract](https://platform.claude.com/docs/en/build-with-claude/streaming).
The UI combines phases with current-turn pending tool IDs, explicit approval,
connection and cancel state. Silence never implies thinking; terminal sessions
remove the status line. Both progress and usage are excluded before text merging.

## Agent Shell environment

At daemon startup, the host Server captures exported variables from the user's
default shell (`SHELL`, falling back to the account shell), using `-ilc` in the
home directory. zsh, bash, sh/dash and fish are supported; each shell follows its
own startup-file rules (Bash login profiles must source `.bashrc` if desired).
The snapshot stays in memory and is reused for Agent discovery, permission probes,
command discovery and execution. It does not mutate the daemon environment or
Claude settings. Local and SSH Servers capture independently on their own hosts.

Shell exports overlay inherited variables; host identity, cwd and `LATTE_WORK_*`
controls remain authoritative. Adapter-specific environment removals/overrides and
the explicit per-turn Provider settings apply afterwards. Shell aliases/functions
and project-directory hooks are not imported. New shell configuration is loaded
on Server restart. For service managers or curated environments, set
`LATTE_WORK_AGENT_ENV=inherit` before starting Server to skip shell initialization.

Capture has a 3-second timeout, a 512 KiB output cap and process-group cleanup.
Startup output is discarded using a unique NUL-delimited marker, not logged.
Failure retains the inherited environment and adds a diagnostic to Agent details.
Environment values, including credentials, are never persisted in the snapshot.

Provider authentication defaults follow the selected protocol: Anthropic uses
`x-api-key`; OpenAI uses `Authorization: Bearer`. A new blank API Key saves
`ProviderAuth::None`. Existing hidden credentials are retained until explicitly
cleared; switching to None removes the stored secret. Existing explicit auth
choices remain compatible. Claude Code requires a nonempty credential before
making a request, so the None adapter sends the public, non-secret token
`latte-work-no-auth` rather than inheriting a real credential. Thus None needs no
server-side authentication, but does not promise absence of the Authorization
header for Claude Code. Gateways that reject any such header are not supported
by this adapter.

Provider model catalogs preserve ID-to-display-name mappings in `model_labels`;
older files default to an empty map. Display names never replace IDs in Agent
requests. The native `fetch_provider_models` command resolves a draft's saved
credential only when its endpoint, auth and protocol still match, releases the
configuration lock, then fetches the model catalog. Discovery uses `/v1/models`
for a bare origin and preserves configured API prefixes, with Anthropic cursor
pagination. Requests reject redirects, have a 15-second total deadline, and
bound each response to 1 MiB, five pages and 256 unique models. A failure leaves
the edited catalog unchanged; successful results remain an unsaved draft.

Model discovery first requests the unversioned catalog to retain canonical IDs from
compatible gateways. For Anthropic providers, an HTTP 400 response retries with
`anthropic-version`; the existing timeout and pagination bounds still apply.

Local lifecycle checks retry a failed read-only `server_status` exchange up to three
attempts inside the existing 20-second coordinator deadline. Upgrade admission and
application requests are never retried by this helper. Desktop startup progress
lives in the composer; disconnected errors use a concise retry control with the
original diagnostic available on hover.

Opening historical sessions reads a bounded latest `history` window (up to two
turns, 128 compacted events / 256 KiB, with a 16,384 raw-event scan cap). Consecutive
text fragments are joined for transfer only; original persisted events remain
unchanged. A raw `before` cursor supports older pages without gaps or duplicate
text. Split final text and older unresolved approvals in the current live turn
must be assembled before reveal. Older records load on upward scrolling or explicit
request, preserving the viewport anchor. Older servers without `history_window`
still use the compatible full Poll replay. The conversation reveals the latest
window after positioning at the bottom across two animation frames; later live
updates retain normal follow behavior and manual reading positions. A scope change cancels
pending reveal work so a stale session cannot reveal the new conversation.

## Conversation lifetime and reading position

Opening an unarchived conversation also opens its native Agent session, even without
sending a message. Switching conversations preserves all opened sessions. The
conversation menu's Close releases the native runtime and returns the selected
conversation to a draft; history remains on the host. Stop interrupts one turn
and keeps the native session open. Native reconnect restores every App-owned
session, including conversations not currently selected, without replaying prompts.
An actual process/daemon restart cannot recover in-flight subagents; interrupted
execution stays failed/unknown while native history is reopened. Busy/background
sessions veto an automatic local daemon upgrade. Both desktop and remote Server
need this revision for the additive lifecycle requests.

The transcript stores its scroll offset and follow-latest preference in App memory
per host/session. Switching back restores that position after concealed layout;
newly opened/explicitly closed-and-reopened conversations and recovered host
connections use a fresh reading scope and reveal the latest position. Settings
navigation does not discard reading positions. No scroll preferences persist across
App exit.

Local and SSH hosts share one initialization and connection-reuse flow. Concurrent
startup consumers share the same handshake and project snapshot; switching to an
already connected host does neither again. A deliberate reconnect or a transport
failure replaces the connection and reconciles the owned sessions without sending
prompts. Disconnected hosts retry independently, including hosts whose conversations
are not selected; healthy hosts are never reinitialized by those retries.
Returning to an opened conversation reuses its completed history and polls
after the last sequence number. The in-memory history cache is bounded to 12
conversations and 16 MiB; a changed daemon identity invalidates that host's cache.
Reading-position restoration is independent of transport and cache availability.

Each host keeps independent persistent request lanes for history/UI reads, Agent
control and slower CLI metadata. Local and SSH use identical routing. Additional
lanes attach to the existing daemon during initial connection and are coalesced;
navigation does not create transports. A reconnect replaces all lanes and fences
stale responses by connection identity and daemon ID. No Send is retried.

## Native subagent presentation

The additive `subagent` event and `subagents` request use the same local/SSH protocol.
The server persists a latest child-task projection atomically with each event so a
bounded history window cannot hide a still-running child. Snapshots return up to
128 recent records, with live tasks first and an explicit truncation flag.
Only the Claude adapter interprets native Agent/Task calls, task_started,
task_progress, task_notification and task_updated patches. Tool-use IDs merge
foreground calls with native task IDs; background launch acknowledgements do not
mean completion. Shell background tasks are excluded from the subagent list.
Child tools update child activity rather than the main-agent transcript/status.
A main result does not finish its background children. Explicit process close
marks children stopped only after process-group cleanup; process failure or daemon
restart marks unfinished children unknown, including after a completed main turn.
Live text streaming tracks at most 512 child IDs. On overflow, existing streams
continue and new child streams wait for complete assistant messages/native history
instead of failing the parent. Skipped streams never restart midway through their
text; admission resets only at a new turn with no unfinished children or streams.
Transport reconnect neither starts children nor replays their prompts.

The conversation summary opens a closeable Subagents workspace tab, grouped by
active, completed and other states. Selecting a task opens a separate named tab,
keyed by stable child ID; repeated selection reuses it and leaves the list intact.
The child tab reads dedicated `subagent_history` forward pages. Full redacted
stdout events and native child records persist separately from parent events,
joined by the task's stable ID. The legacy `subagent_transcript` bounded snapshot
remains available for old clients, but its limits do not govern the child UI.
The additive optional `prompt` field reads the original, already-redacted parent
Agent/Task tool input by session and tool-use ID, independently of loaded chat
pages, and survives restart. Missing input stays absent; the UI exposes saved
input in a separate “任务指令” disclosure above process and final output.
History pages contain at most 128 events / roughly 256 KiB; a larger individual
event remains intact within the protocol frame limit. The client reads successive
pages while visible, including after completion, and joins the complete final
answer before marking it final or folding the process. Hiding a tab stops further
page reads; reopening or reconnecting resumes from persisted records without
executing an Agent or replaying a prompt. Cursor progress is validated.
Claude filters child text from parent stdout, so the owning Runtime also indexes
its native JSONL from the beginning in forward pages. Paths derive only from the
registered cwd, parent UUID and native child ID; output_file paths are ignored.
Each scan reads about 512 KiB of complete records, individually bounded by the 2 MiB protocol frame with reserved envelope space;
oversized or malformed records fail explicitly instead of being silently clipped
or skipped. No-follow regular-file checks reject symlink directories and changed
file identities. Every accepted record verifies parent/child/sidechain identity;
hidden thinking blocks remain excluded. Partial last lines are retried after the
producer appends a newline. SQLite persists the file cursor and redacted pending
handback messages, so confirmations can cross page boundaries. A capture cannot
regress a newer persisted cursor. Terminal notifications also index full pages
under a three-second total budget before the original context can be replaced;
subsequent visible reads continue an unfinished capture. After a runtime closes,
the desktop can supply the same ephemeral Provider snapshot through the native
client bridge. Recovery compares its digest with the accepted parent request
before the child started; unknown or changed contexts only serve redacted caches.
Matching historical reads index the original file without opening an Agent or
replaying any request. Only a digest enters the durable request ledger.
Successful native `SubagentHandback` delivery promotes its full `message` only
after the matching result explicitly confirms success. Tool fields and final
prose have no legacy 16 / 32 / 128 KiB clipping. Source credentials are redacted
before storage. Pre-upgrade snapshots are still usable without execution; if
fields were already discarded and the owning original record cannot be read,
the UI explicitly marks missing source evidence instead of a display-size limit.
Native reads have a three-second deadline and a shared two-reader limit; stopped
runtimes serve persisted records or recover native pages only with a verified
historical redaction context, without opening or replaying an Agent. No untrusted transcript paths are read. The visible child list refreshes independently of parent
events, and active child transcripts use the same 650 ms polling gap as the main
conversation. Reads are serialized; metadata revisions coalesce into one queued
refresh without invalidating an in-flight read. Completion queues a final read
and stops periodic polling. Hiding a tab stops its periodic reads; showing it
refreshes immediately. Host/session/child/connection changes fence late responses,
and recorded evidence survives read failures and reconnects. Successful completion folds the process by default
and keeps the final native output visible; failed/unknown children retain their
process evidence. The detail uses the main conversation layout, tool rows and
reported-phase status row without a composer; scrolling up pauses following and
offers the shared return-to-latest control. Tools without a result remain unconfirmed, never successful.
Old histories without captured child events keep their native summary. Both the
local client and remote host need the additive transcript request to use this view.
Hiding or closing a child tab never stops execution. Child result envelopes cannot
finish the parent turn. Trailing empty/duplicate success results while idle are
ignored; result-only autonomous replies and actual failures remain visible.

The Task Summary tab shows compact, read-only sections with uniform empty states.
Each section has an independent disclosure button and a count that updates while
collapsed. Child-agent status groups also collapse independently: completed children
start collapsed, while active and other states start expanded. Explicit choices
are cached by host/session for the current app run, bounded to 64 scopes; refresh
and incoming events do not overwrite them. Collapsed content stays mounted to
preserve nested state. Disclosure is presentation-only and does not change polling
or execution. The separate refresh control never toggles disclosure; native button
keyboard behavior and reduced-motion chevrons are preserved.
Its append-only event projection reuses parsed records; prepended history and
replaced records reset it. Unfinished streamed text is parsed on settlement, not
on each token, and Markdown fences are stripped in a linear line scan. TodoWrite
replaces its own snapshot while retaining TaskCreate records. Branch reviews are
read on opening, settlement and explicit refresh, independent of streamed tool
activity; the lightweight source inventory refreshes at most once every two seconds
while running, with one read in flight. Authentication prompts suspend these reads.
The single hover/focus background on each workspace tab includes its close control.
Project names and history-scope paragraphs are not repeated in the inspector;
the footer offers explicit earlier-history reads, with scope in its tooltip.
Resource links reuse a visited page's declared favicon when available, otherwise
load the corresponding HTTP(S) origin's `/favicon.ico`, without a referrer or a
third-party icon service. Missing icons retain a link glyph without an error notice.
Image loading is lazy; summary collection does not fetch page bodies to discover
icons or titles. The main WebView CSP allows HTTP(S) favicon paths
while keeping script, connect and frame policies unchanged. Titles still come from
conversation labels or previously visited browser pages; untitled links show one
compact address rather than repeating the hostname above the URL.

`skill_info` resolves current installed Skill metadata on the execution host without
starting an Agent, changing access or evaluating Skill body/template commands.
The Claude implementation reads known personal/project folders (including linked
skill folders) and namespaced installed-plugin records; unsupported, missing and
ambiguous metadata are explicit. This is current-file metadata, not proof of the
exact Skill version used in an earlier turn. Location reference:
[Claude skills](https://code.claude.com/docs/en/skills#choose-where-skills-load).
Metadata reads use two concurrent blocking workers, a three-second deadline,
64 KiB Skill files, a 256 KiB plugin index, and bounded candidate counts. Clicking
an available Skill selects its canonical directory through `resolve_reference`,
then opens `SKILL.md` directly in the shared Files tab, with the directory tree.
Task files use the same Files tab. An external file click resolves exactly that file;
its preview does not browse or grant access to its parent. Explicit directory
selections retain their canonical preview scope, separate from project browsing.
Task Summary presents file names, paths and Skill descriptions with direct open actions;
it does not expose retained permission scopes or authorization management. Hover only reads
metadata and never grants directory access. Preview still enforces canonical containment,
including escaping nested symlinks. The Server retains the `reference_access` metadata API
for reading the project's canonical file/directory scopes.
`revoke_reference_access` removes exactly that project's recorded scope even when
the original path no longer exists; subsequent preview requests recheck permission.
If no exact recorded key matches, existing absolute aliases are canonicalized with
a five-second deadline outside the database lock before deletion. Missing paths
remain idempotent. Admission checks and upserts share one SQL statement, so concurrent
connections cannot exceed the 512-scope limit; activity-index replacement is transactional.
Other explicit task sources or uploaded attachments may independently
allow access. Retained grants are bounded to 512 per project; list responses also
stay within half the wire-frame budget, accounting for JSON escaping. Older
oversized lists are disclosed as truncated and can be reduced by revoking entries. Neither listing
nor revocation opens an Agent or grants new access; older Servers fail explicitly.
Local and SSH hosts share this additive request; older Servers must update for
metadata. Failures do not fabricate descriptions or paths. Focused checks include
`claude_skills` unit tests, `SkillRow`/`TaskInfo` frontend tests and the final-binary
`skill_metadata_is_host_local_read_only_and_preview_requires_explicit_reference` test.

The topbar Task Overview popover links to changes and subagents. The Sources
section and source tabs are currently removed from the workspace UI. The source
inventory and scoped preview APIs described below also back the summary and Files tab;
Composer references continue to use their existing add and send flow.
The desktop owns a single `/agents` (子智能体) navigation entry for every Agent.
Native commands carrying the `subagents` presentation hint, and the reserved
`/agents`, `/list-agents`, `/tasks` and `/subagents` aliases, collapse into this
entry instead of exposing duplicate native wizard/list controls. The built-in
entry remains usable if native catalog discovery fails. Selection fills the
draft; submission opens the current conversation's Subagents tab without sending
to the Agent, creating a conversation or writing a user event. Bare navigation
clears only command text and preserves references. Arguments never reach the
Agent: the panel opens with an explicit unsupported-arguments notice while the
draft and references remain intact. Navigation works during active tasks and
while disconnected; it never stops the Agent or changes its lifecycle state.
Adapter-specific pasted navigation commands are routed after metadata discovery,
with late results fenced by the current composer context.
Unsupported commands are not added to the catalog. These controls observe native
execution; individual child steering/stopping is not synthesized.

Focused checks: adapter/store UT, frontend SubagentsPanel/useSubagents/Composer/
useWorkbench tests and the final-binary
`subagents_native_commands_snapshot_reconnect_and_close_share_one_protocol` test.
Native UI and actual model-driven Claude delegation remain separate evidence.
`python3 scripts/smoke-subagents.py` uses the installed real Claude CLI with a
disposable localhost Anthropic model fixture. It verifies actual foreground and
background Agent execution, child Read isolation, terminal notifications,
reconnect snapshots and native listing-command metadata without paid API calls.
It never dispatches listing commands into the conversation used for UI checks.
It does not verify a live model's delegation decisions. `--pause-for-ui` prints
an isolated state directory and bounded release/finish markers for native UI checks.
SDK lifecycle reference: [native task types](https://github.com/anthropics/claude-agent-sdk-python/blob/main/src/claude_agent_sdk/types.py).


### Task changes and source previews

Before the first accepted agent request spawns, the server stores a durable file
baseline in the task database. The request is persisted first. Git-visible files
(including tracked and untracked dirty files) are read without changing the index,
refs or user files. Non-Git projects use a bounded recursive file inventory, skipping
`.git`, `node_modules`, `target` and `.DS_Store` and refusing symlink traversal.
This inventory visits at most 5,000 entries; edits, creations and deletions use the
same saved before/after bytes and do not require Git repository metadata. Baselines are limited to 1,000 files, 512 KiB per file, 8 MiB
total and 12 seconds. Unknown, oversized and symlink entries are explicitly partial;
old tasks without a baseline never retroactively infer one. Task totals/diffs compare
this baseline with the current worktree, regardless of staging or later commits.
This is temporal attribution: other writers in a shared directory can appear in
these net changes. The UI provides separate task and workspace views.

Explicit user reference manifests and actual native tool calls are indexed durably.
The compact Composer manifest and legacy expanded manifest are both recognized;
the compact form preserves paths/types and necessary attachment names/kinds while
omitting repeated MIME and formatting metadata. Saved requests still contain the
exact prompt dispatched to the Agent. The transcript extracts only valid complete
App manifests for attachment presentation; arbitrary/malformed prose stays visible.
Composer image chips and transcript thumbnails share an in-App image dialog and
the existing host-scoped preview reader. No new preview protocol or filesystem
permission is added. Visible image reads share a two-reader/64-waiter bound;
hidden/off-screen readers stop starting chunks and release object URLs. The image
dialog traps/restores focus, closes on scope changes and uses sniffed raster MIME.
Tool-use IDs deduplicate calls, including child calls. MCP tool names identify
connector groups; installed-but-unused connectors are never fabricated. Source
inventories return at most 128 entries with a truncation flag.

The host-only preview API reads 64 KiB chunks with a five-second timeout. It accepts
project-contained paths, completed managed uploads, explicitly resolved project
references, or the task's persisted references. Canonical containment is checked;
ordinary read_file boundaries and agent permissions are unchanged. Images, PDF and
video are sniffed from bytes; HTML and SVG remain plain text. Directory previews
skip symlinks. Files larger than 64 MiB fail explicitly. Desktop inline media are
limited to 16 MiB, text to 512 KiB, and full system preview copies to 32 MiB.
Object URLs are revoked on disposal and late responses are fenced by host/task.
The desktop renders PDF first-page thumbnails through macOS Quick Look (two concurrent
renders, ten-second timeout, at most 8 MiB of PNG), with full-document system preview.
No iframe or external document scripts are enabled. Quick Look uses a private 0600 local copy, at most two concurrent processes,
and removes that copy on preview close, application Quit or after one hour. SSH hosts use the same chunked
protocol; the server has no GUI dependency.

Each accepted user turn also saves a request-linked baseline before native dispatch.
At the main result or confirmed process stop/failure, the server freezes before/after
bytes and a `turn_changes` event, atomically replacing the pending baseline. Only
changed bytes are retained (the same per-file/total capture limits apply to both
snapshots). Turn cards show file counts and per-file/total added and removed lines;
selecting a file or View Changes opens a deduplicated, closeable turn tab. Historical
turn diffs read these stored bytes, not HEAD or the current filesystem. Main results
with unfinished background children are explicitly labelled and cannot be undone.
Shared-directory attribution remains temporal; other concurrent writers can appear.
Daemon restart marks unfinished snapshot collection unknown, never captures a late
filesystem state as that turn's result. Legacy turns have no invented snapshots.

Undo is a host operation for the conversation's latest turn only. It requires a
complete snapshot, no active host tasks/background work/PTYs, and byte/permission
matches against every stored after-image. Exclusive host admission prevents new
Send/Open/Approval/terminal writes during restoration; overlapping idle Agent
runtimes are closed and can be reopened normally. Symlink paths are refused and
all files are checked before the first write and again before each atomic replacement.
Files are restored without touching Git's index or refs. A durable unknown intent
precedes writes; only full success becomes reverted. Failure/crash stays unknown
and is never automatically retried. Completed repeated Undo requests reconcile
idempotently. Independent external writers are not locked; post-turn conflicts
are rejected and an interrupted restoration requires manual workspace inspection.

## Embedded browser

Browser tabs use site favicons; a missing or failed image keeps a small link glyph
without layout changes. On macOS, finished navigation reads at most eight declared
icon URLs from the current document, then requests the first valid HTTP(S) icon
with an origin `/favicon.ico` fallback. The native image transport uses ephemeral
cookie-free and credential-free requests, accepts only supported image MIME types,
limits each response to 64 KiB, follows at most three validated redirects without
HTTPS downgrade, and applies two-second request / three-second resource deadlines.
At most four icon collections run concurrently, with an eight-second completion
deadline. A 64-entry in-memory cache contains only bounded image data, keyed by
document URL; navigation fences discard stale results. No third-party icon service
or website login credentials are used. Other platforms retain the origin fallback.

The address field includes an anchored site-information entry. Its dialog shows
the current domain/title and two entries: connection details and Cookie/website data.
Copy URL and system-browser actions remain outside this dialog. Connection status
comes from the native committed WebView's evaluated server trust and secure-content
state, never from an HTTPS string alone; unavailable trust stays unknown. It does
not claim to manage website permissions. Connection details include the committed
native certificate subject and issuer when available (macOS 12+). Overlapping
dialogs and menus retain a
decoded viewport image behind the overlay before hiding the native surface; closing
restores the live surface before releasing the image. Escape and outside click
restore keyboard focus. Site-info
uses the existing caller-checked, nonce-bound native bridge, with a 128 KiB response
limit; it grants no page IPC access or additional filesystem permissions.

Focused checks: `BrowserSiteInfo.test.tsx`, `SiteIcon.test.tsx`, `BrowserPane.test.tsx`
and native desktop tests. On macOS, `python3 scripts/test-site-icons.py` compiles a
temporary native harness and verifies the production image transport against a
local HTTP fixture (size/MIME/redirect/authentication/timeout/header boundaries).
Real site icons, native connection status and popup layout require separate UI smoke.

Overlay previews use the same caller-checked, nonce-bound snapshot bridge without
a save picker. Native JPEG output is limited to 2048 logical pixels on its longest
side and 8 MiB; the UI receives binary data, decodes it with a 1.5-second deadline,
and holds only a temporary Blob URL. The private bridge files are removed after the
request. Images are not added to browser history, downloads or workspace state.
Capture and geometry changes serialize per pane; late snapshots after close,
navigation, tab hiding or unmount are discarded. Failed captures keep the live page
and dismiss browser menu/site-information overlays rather than showing a blank page.
Overlays outside the native page bounds do not request an image or hide the page.

The browser toolbar's menu supports find, 25–300% page zoom, the system print/PDF
panel, visible-viewport PNG snapshots, native downloads, searchable visit history,
and explicitly confirmed data clearing. Native menu accelerators route through
the selected browser pane (including when the child WebView owns keyboard focus).
History is bounded to 500 entries; title callbacks and data-clear reloads cannot
recreate cleared history. Downloads retain WebKit's original request and cookies;
at most four transfers/save dialogs can be outstanding and the latest 100 records
are persisted. Finished files up to 128 MiB are offered in the native save picker;
cancel/failure/interruption are explicit and never auto-replayed. Larger files are
rejected after transfer; this is a saved-file limit, not a network transfer quota.
Clearing download records preserves user-saved files.

On macOS, `macos_browser.m` gives only incognito-constructed browser surfaces a
shared profile separate from the workbench. macOS 14+ uses a persistent named
WebKit data store; older macOS uses a shared session-only store. Existing default
store website logins are not migrated. Browser-only configurations append
`Version/<installed Safari version> Safari/605.1.15` through WebKit's
`applicationNameForUserAgent`. The default macOS WKWebView UA lacks those tokens,
which can trigger obsolete-browser detection even on a current WebKit. The version
is read from the installed `com.apple.Safari` bundle, not hardcoded or replaced with
a Chromium identity; absent or invalid metadata leaves WebKit's default unchanged.
The main workbench UA is unaffected. This changes identification, not engine capabilities.
Find/snapshot/clear use a single-use nonce
bridge backed by a private per-process request directory, bound to the exact
WebView, with bounded payloads and ten-second completion timeouts. Pages cannot
pass paths or arbitrary commands; Rust remains free of unsafe code. Snapshots
capture only the current viewport (up to 4096 × 4096 points, 64 MiB PNG), and show
a save picker only after successful native capture. None of these tools grant
remote pages the workbench IPC capability. Non-macOS find/snapshot/profile tools
fail explicitly; no native cross-platform smoke claim is implied.

Focused checks: `BrowserTools.test.tsx`, `browserHistory.test.ts`, native desktop
tests, and separate real macOS smoke of find/zoom/PDF/PNG/POST downloads/clear.

The desktop uses Tauri's `unstable` child-WebView API to place independent native
HTTP(S) pages in workspace tabs. Main UI capabilities target only the `main`
WebView, not the whole window. Browser commands validate their caller and URLs;
remote pages cannot navigate to the App origin, custom protocols or URLs with
credentials. The App CSP keeps `frame-src 'none'`; no iframe is used. New-window
requests create workspace tabs; downloads use the native transfer and explicit save
flow described above. Closing a tab releases its native surface; hiding it retains navigation.
Dock reopening uses the native Window handle because a multi-WebView window is
no longer a WebviewWindow. Server/Agent execution and SSH contracts are unchanged.

Browser preference lives in `latte-work.link-target.v1` local storage (default
`internal`). Browser URL/title metadata follows workspace persistence; pages are
created lazily when selected. Native IDs are per-mounted-pane, so first-send draft
migration does not collide with a retained draft surface. Focused tests cover link
routing, preference write failure, unsafe URL restoration and surface lifecycle.

Site-data reads enumerate only the isolated browser profile, select matching WebKit
records using their public-suffix-grouped domain names, and return domain/type
metadata without cookie values. Subdomains can share a data scope; the UI names
that scope before an explicit confirmation. The clear token expires after two
minutes and is single-use, bound to the exact surface, URL, store, records and
scope. The bridge limits reads to 4,096 records/cookies, eight selected records,
32 types, 512 selected cookies and an 8 KiB response. Navigation and store changes
reject stale operations. Clearing removes only the selected records, then explicitly
deletes matching HTTP session cookies, and refreshes the current page without
creating a new history visit. A failed or lost clear response is not replayed;
the UI requires a fresh read. Other domains and the workbench data store are
preserved. `python3 scripts/test-site-data.py` exercises this implementation against
a real ephemeral WebKit cookie store without browsing or using a user profile.
Native UI checks and the data-store test are recorded separately in
`docs/validation/browser-site-info-2026-10-16.md`.

## Native session titles

Agent-generated titles are host metadata, not UI-generated summaries. Claude's
`session_title_changed` reports an explicit native session name (not an AI title).
It updates the saved custom name without creating a turn, including a native-side
rename after an earlier Latte rename.
Manual Rename uses the running session's native `rename_session` control request
and commits the displayed title only after acknowledgement. Rejection preserves
the old title; timeout stays unconfirmed and does not replay the request. Closed
sessions use the official SDK's append-only `custom-title` metadata format and
read back the result; no Agent is launched and no user prompt is sent. Before a
native session exists, the requested title is passed with `--name` at creation.

Native metadata reads use the same host shell environment as execution, validate
the exact native UUID, reject escaping/symlink session files, and read bounded
incremental pages of at most 4 MiB per file. The 256-entry metadata index keeps
only titles, offsets and a partial record of at most 64 KiB, not transcript text;
it resets on file replacement or truncation. Reads continue until the current end
before applying a title, so metadata in the middle is not lost or replaced with
an older name during catch-up. Offline renames verify a bounded 64 KiB tail.
Each refresh processes at most 16 files. Custom names take priority over AI titles.
Metadata reads refresh closed sessions and unnamed live sessions, allowing
AI titles to appear without closing an Agent. Named live sessions follow native
events so stale transcript flushes cannot overwrite a rename.
File reads are debounced per session/native identity for two seconds across
all clients, with at most 2,048 entries. Repeated polls reuse the current database
projection. External native renames appear at the next eligible metadata read;
read results apply only if the native identity, title and custom-title state still
match the original snapshot; named live actors remain protected from file reads.
These semantics are shared by local and SSH hosts. Unsupported native live rename
returns an error instead of silently renaming only the Latte record.

The unified change page uses an additive `git_commits` metadata read (50 first-parent
commits per page). The returned head SHA pins subsequent offset pages; no fetch or
Git mutation occurs. `GitReviewScope::Commit` compares a selected full commit SHA
with its first parent, or the empty tree for a root commit. File reads use the
resolved base/head from the summary, the same literal project path boundary and
external-diff/textconv restrictions as branch review. Local and SSH use the same
handler; older remote servers report unsupported history and need an update.
Focused checks: `git_review::tests` and the final-binary
`git_review_wire_preserves_scope_and_branch_snapshot_after_reconnect` test.

## Agent questions

`AskUserQuestion` is translated inside the Claude adapter into generic `question`
events, independently of tool approvals. It requires user input even in
`bypassPermissions`: [native SDK user input](https://code.claude.com/docs/en/agent-sdk/user-input).
The additive `answer_question` request submits selected option indices and optional
free text, or null for an explicit skip. Answers are checked against the active
session's single-use question ID before consumption; invalid or cross-session
responses do not stop execution or consume the question. Original native questions
are retained only in the actor; the adapter returns them with the answers mapping
through `updatedInput`. Persisted question/resolution events are redacted, do not
create a user turn and are not automatically replayed after a transport failure.
Questions and approvals share the existing 32-interaction and process time bounds.
Old servers cannot accept structured answers; the client offers an explicit update
message and optional skip, never an empty approval. Focused final-binary check:
`user_questions_are_not_permissions_and_answers_are_bound_single_use_and_persisted`.

## Persistent message queue

`send_queued` is an explicit additive alternative to `send`; an idle session starts
immediately, a busy session saves the prompt in the host's SQLite FIFO. The desktop
uses this API and the same native provider injection for local and SSH hosts.
`message_queued` acknowledges durable admission, not execution; the user event and
request record are written only when the actor starts that turn. Starting a queued
turn and retiring its pending row share a transaction. Original admission identities
and cancellation tombstones prevent lost responses from re-enqueuing messages,
including after editing. Transport reconnect never re-sends a prompt.

Queue listing, revision-checked editing, removal, clearing and explicit resumption
share the host protocol. Drag/keyboard reordering compares the complete pending
request ID order and atomically persists its replacement. Stale, missing or duplicate
IDs fail without mutation; a repeated identical order is idempotent. Dispatch always
uses this saved order. The session actor serializes reorder/edits/cancellation against
execution. Capacity is 16 pending messages / 512 KiB serialized content per session,
with the existing 128 KiB per-prompt limit. Queue records contain prompts and launch
option metadata, never Provider snapshots or credentials. A pending queue has one
Provider identity; each item retains its model, effort and permission choices.
Native launch configurations stay in bounded actor memory. Resumption must supply
that original Provider identity again; changed credentials/settings fail explicitly.

Normal completion drains the saved order even after the initiating client disconnects. Stop,
close, failure and daemon restart retain the remaining queue in a paused state.
Restart never replays the interrupted active request. After checking the current
conversation, the user can resume the pending messages with fresh native configuration,
or edit/remove them. Pending active queues veto idle shutdown; paused queues remain
safe to close. Closing/clearing the queue does not stop the current turn.

Focused validation: final-binary `message_queue_` tests cover FIFO, multi-client
reconnect, reordering/restart persistence, edits/removal, request identity, stop/restart, bounded admission, session
isolation, and credential-free persistence/configuration fencing. Native fixture
smoke evidence is separate from real-model behavior.

The queued-row send arrow uses `dispatch_queued_message` with `interrupt: false`.
While a native turn is running, the server writes the selected prompt to that same
Claude stream immediately; it never sends an interrupt or waits for the host queue
head. A persisted `queued_message_sent` receipt carries the original text and request
ID, distinct from the `user` execution-start event. The desktop immediately displays
it with the shared user-message bubble, original send time, copy and attachment controls.
For legacy histories with a later execution event, it correlates by session/request
ID and text, keeping one visible bubble at delivery time. A history page without
the receipt still displays the execution event normally. Handoffs never end an in-flight tool or disappear
inside a completed turn's folded process. Legacy prose receipts remain readable;
the UI does not infer request identity from their text. Native
completion consumes inputs by their native UUID attribution (`user_message_uuids`,
or `user_message_uuid`), with replay acknowledgements as compatibility evidence.
One result can settle several inputs folded into the same turn. Handoffs do not
create synthetic execution-start events or trigger another turn when a result arrives;
only native output starts a native follow-up. Unconfirmed inputs remain held without
replay and block host-held messages until native confirmation or explicit stop/close.
Outstanding native inputs
share the 16-message/512-KiB admission budget. Streamed handoffs cannot provide a
safe independent filesystem boundary, so overlapping turn snapshots cannot be undone.

Active native turns retain their 24-hour execution limit. Each handed-off input has
an independent 24-hour confirmation deadline measured from its delivery, including
after its original turn completes. New inputs do not extend older confirmation
deadlines or the active turn limit. A confirmation timeout identifies unconfirmed
delivery, preserves its receipt and pauses the queue without replay, instead of
reporting that an already completed turn exceeded its execution limit.

Reproduce the real CLI queue contract without a model service or login:
`python3 scripts/smoke-claude-queue.py --binary target/debug/latte-work-server --claude ~/.local/bin/claude --output target/real-queue.json`.
The local fixture provides only Anthropic API responses. The installed CLI must
accept `--replay-user-messages` and generate its own replay/UUID receipts; the
script captures those fields, checks two delivered inputs settle together with
one native result and no duplicate execution-start event, and closes its isolated
Server. CLI version, binary/source hashes and the observed UUID fields are saved
separately from Python-CLI E2E evidence. This smoke is optional and outside CI.

The extra menu's “停止当前并发送” explicitly requests `interrupt: true`. With no
native backlog, it waits for both the native result and interrupt acknowledgement
before starting the selected message. If native inputs were already handed off,
stopping closes that process and marks their processing state uncertain; they are
never replayed automatically. The selected host-held item then starts in the saved
session. Stop, close, failure and restart also quarantine unconfirmed native inputs
and preserve their original prompt receipts. Remaining host-held messages retain
their order and the queue's original paused/running choice.

Editing uses revision-bound `take_queued_message`: atomically retire the pending
item and return its exact content. A repeated take reconciles a lost response.
The client merges it into the session draft, preserving existing text and attached
references, and focuses the composer. Submitting uses a new request ID and appends
to the host queue. No in-place edit or automatic resend occurs.

Sidebar project ordering is an App-only preference (`latte-work.project-order.v1`),
keyed by the host/project pair rather than names or paths. Mixed-host ordering
survives project catalog refresh and restart; new projects append, and temporarily
absent hosts retain their saved slots. Pointer-based dragging shows a floating
project and insertion line, scrolls at the navigation edges, and moves the entire
project group without changing its expanded state or selected conversation.
Escape, pointer cancellation, window blur and dropping outside the project list
cancel the gesture. Option+Up/Down provides keyboard ordering. Only held primary
mouse movement beyond the threshold starts a drag; accessibility clicks and
ordinary hover must not become a drag or suppress project expansion. No Server
mutation or Agent request is used to save this display order.

Queue dispatch failures are checked after each activation, before considering the
next item; auto-drain reads the current session status. A delayed immediate-send
confirmation pauses the queue without classifying the native process as failed or
killing it. Later interrupt acknowledgements cannot release the selected pending
message without an explicit resume. A drag uses the latest visible queue order;
removed source/target rows cancel the drop, and the Server still checks full CAS.
Previously acknowledged requests produce an informational receipt on explicit
reconciliation. Taken-for-edit and cancelled requests retain distinct explanations.
The edit/clear queue protocol requests remain for existing clients and host tests;
the desktop exposes take-to-edit and per-item deletion only.
