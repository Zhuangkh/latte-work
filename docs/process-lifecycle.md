# Process ownership and daemon shutdown

`latte-work-platform::process` separates three operations:

- `ManagedChild::spawn` owns an Agent/probe process and its descendants. The root
  can exit first without releasing descendant ownership. `wait()` waits only for
  the root; `close(grace)` checks cleanup and is repeatable. Drop is a force-stop
  fallback for cancellation/error paths, not evidence of successful cleanup.
- `spawn_daemon` starts an independent process with null stdin/stdout/stderr and
  no kill-on-drop ownership. The stdio bridge may exit or be killed without
  stopping that daemon. The caller retains its existing startup deadline and IPC
  authentication/absence checks; the process helper neither retries nor sends
  protocol traffic.
- `Shutdown` registers host exit notifications once. Unix uses SIGINT/SIGTERM;
  Windows uses Ctrl+C, Ctrl+Break and console-close notifications. Detached
  Windows daemons ordinarily exit through authenticated lifecycle IPC. Console
  close/logoff/OS shutdown have platform deadlines and are not guaranteed graceful
  exits; no Windows Service lifecycle is implemented. Windows SSH uses
  `connect-existing` to reach a daemon started independently of the SSH session;
  it never calls `spawn_daemon`. See [SSH setup](remote.md#windows-openssh-targets).

## Native process trees

Unix creates a process group, sends TERM for the caller's grace period, then KILL
and reaps the root. After root exit it still kills the remaining group. As before,
descendants that create their own sessions/groups can escape; Unix groups are not
security sandboxes. Non-child zombies are reaped by their parents/init, and a
successful Unix close confirms group signaling and root reaping rather than a
Windows-style kernel count of zero.

On macOS only, explicit forced cleanup retries a typed SIGKILL/EPERM failure
within the same absolute three-second cleanup deadline used for root reaping and
the final tree check. Each retry polls the root so an exited child can be reaped,
then uses a cancellable timer of at most ten milliseconds. Root exit, EPERM and
elapsed time never count as successful group signaling: KILL must succeed or
report ESRCH, followed by confirmed root reaping. Persistent EPERM retains its
PermissionDenied error, signal/PGID/errno source and deadline context. Other KILL
errors and TERM errors propagate immediately. Grace remains a separate budget;
cancelled or failed close retains ownership for a later retry. Drop keeps its
synchronous best-effort force-stop path.

Windows creates an unnamed, non-inheritable Job with KILL_ON_JOB_CLOSE. It starts
the child suspended with CREATE_NO_WINDOW, assigns it to the Job, then resumes
its initial threads. Assignment/resume failure kills the root and closes the
Job; there is no fallback to an unsupervised child. Explicit close terminates the
Job, reaps the root and polls ActiveProcesses until zero with a three-second
cleanup deadline. Root exit alone never establishes that descendants have exited.
Polling uses cancellable async timers, not indefinitely blocked worker threads.
Nested Jobs are supported where the caller's external Job policy permits them;
assignment failure propagates. Breakaway is not enabled.

The project-owned FFI lives only in the existing private
`latte-work-windows-security::win32` module; all callers still forbid unsafe.
`ProcessJob` is a safe owned facade. The platform keeps the newly spawned child's
handle alive while assigning its PID. A separate opened process handle pins its
identity during assignment/resumption. Thread enumeration is bounded; opened
threads have their process identity checked before resumption. Native commands
retain separate OS-string arguments/environment. Explicit `.cmd`, `.bat` and
`.ps1` launches fail with Unsupported before creation. The Claude adapter now
resolves a native absolute path once; see [Development](../DEVELOPMENT.md#agent-shell-environment).

`process-wrap 10.0.1` was evaluated but is not a dependency. Its inspected Windows
wait implementation uses an uncancellable blocking completion-port wait and does
not verify that the received packet represents an empty Job. This implementation
uses the same suspended-assignment principle but retains its own Job handle and
queries accounting directly. It does not expand the unsafe exception to platform
or Server.

Daemon startup uses the existing new process group on Unix, and
DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP on Windows. It is never wrapped in
ManagedChild or a bridge-owned kill-on-close Job. An external supervisor's Job can
still contain both bridge and daemon; the helper does not bypass those policies
or promise survival when that external Job is terminated. The ordinary local
bridge exit and forced bridge-process exit are covered by native tests.

## Server integration

Runtime Agent sessions, native command discovery, help/version probes and Unix
shell environment capture use ManagedChild. The common bounded-output helper
caps metadata stdout, discards stderr, bounds time, and explicitly cleans the tree
on success, nonzero exit, oversized output and timeout. Version output is capped
at 16 KiB/5 seconds; permission help at 128 KiB/5 seconds. Native command discovery
retains its 12-second/frame/message limits. Cleanup can add at most three seconds
to a probe deadline. Cancelling the probe future still invokes owned Drop cleanup.
Probe cleanup errors take precedence over output-limit, timeout or exit results;
the probe's local guard then drops with best-effort cleanup rather than remaining
in a retry owner.

Windows environment initialization inherits Server variables and executes no
shell/profile. Unix capture remains in a platform-selected module with its
existing shell rules, protected-variable filtering and output/time limits.
Claude's native Interrupt still stops a turn without closing the session. Close
uses tree cleanup; a failed explicit close reports an error and retains ownership
for retry instead of recording Stopped. Automatic failure recovery cannot reopen
a process if cleanup was not confirmed. Unfinished state stays failed/unknown.

Daemon exit now closes request admission for existing connections as well as new
accepts. If an in-flight dispatch cannot yield admission within two seconds, its
connection tasks are cancelled before fencing. Terminals use their platform close path and retain ownership until cleanup is
confirmed; Windows ConPTY uses its own Job-owned helper. Agent close requests run concurrently with a six-second total
queue/cleanup budget. Errors from one actor do not prevent the others from being
awaited. Remaining actors are aborted and given one second to release their owned
guards; timeout/error is returned, never converted into a successful close event.
Aborted actors leave unfinished persistence for restart reconciliation.

The existing three-second IPC draining window runs concurrently with cleanup.
Connection tasks are reaped before listener/singleton release, including listener
failure paths. A signal received during bounded startup work is registered and
pending for the acceptance loop. This does not extend force-kill guarantees:
Windows Job closure handles daemon death, while Unix SIGKILL cannot execute group
cleanup. No prompt is replayed.

## Evidence and limits

Native Windows tests run real processes using the test binary as a deterministic
fixture. They cover root-before-descendant exit, repeated close, owner Drop and
task abort, supervisor force-kill, nested Jobs, native Unicode executable paths,
arguments/environment, missing binaries, script rejection, oversized output and
probe timeout. File locks independently demonstrate descendant exit without PID
reuse assumptions.

The daemon fixture uses the real IPC Endpoint and singleton lease. After normal
and forced bridge exit it exchanges bytes, retains the singleton, and releases it
on an explicit shutdown exchange. It also proves the bridge's lock handle was not
inherited. This validates platform integration, not the final Server binary.

Unix adds a real SIGTERM notification/cleanup test. Linux and macOS platform code
and tests cross-compile with Clippy; native execution remains for their CI hosts.
Retry-policy tests inject typed signal errors under a paused clock, checking
EPERM recovery, persistent failure, root polling, deadline reuse, cancellation
and immediate propagation of other errors. Real-process fixtures also check
failed-close ownership and, on macOS, cancellation followed by a successful close.
These tests do not establish zombie membership in a historical CI failure.
A Server unit regression checks that shutdown propagates failed cleanup, permits
other actors to finish, and reaps remaining tasks. Final-server native lifecycle
and Agent E2E tests must also pass on Unix CI. This Windows workstation cannot run
them; Linux cross-check still requires the target C toolchain for aws-lc-sys.

The Windows Server native suites additionally exercise final-binary lifecycle,
ConPTY helper cleanup and guarded NTFS Undo. Snapshot compatibility and restoration
limits are in [architecture](architecture.md). Failed cleanup retains ownership for
retry and quarantines the resource from unsafe reopening, Undo or upgrade; Drop is
never proof of success. The user reports real local Claude, Windows SSH and
installation/upgrade acceptance (see [Development](../DEVELOPMENT.md)).
Console-event variations, incompatible external Job policies and injected assignment/
resume failures still require specific native evidence. Windows Service lifecycle
remains unsupported; fixture success does not establish a detailed native UI or
cross-user attack matrix.

Focused checks:

```sh
cargo test -p latte-work-platform process:: --locked
cargo clippy -p latte-work-platform -p latte-work-windows-security --all-targets --locked -- -D warnings
cargo test -p latte-work-server --bin latte-work-server runtime::tests --locked
cargo test -p latte-work-server --test e2e --locked
```
