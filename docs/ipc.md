# Local IPC

`latte-work-platform::ipc` is the only application-facing local transport API.
`serve`, `connect`, and `connect-local` use the same `Endpoint`, `Listener` and
`Stream`. The client still launches a disposable stdio bridge; application JSON,
Hello, upgrade messages and request replay policy are unchanged.

## Endpoint and state

Callers prepare the ordinary state directory before IPC initialization.
`Endpoint::prepare(state)` is explicit initialization: Unix retains private
0700 state-directory behavior; Windows creates or validates a private `control/`
leaf without repairing existing ACLs. `Endpoint::open(state)` is read-only and
never creates a missing control directory. `connect` also never creates paths.
Neither operation launches a process or sends protocol traffic.

| | Unix | Windows |
| --- | --- | --- |
| Transport | Tokio Unix stream/socket | interprocess 2.4.5 byte-mode named pipe |
| Address | `<state>/control.sock`, unchanged | `\\.\pipe\latte-work-v1-<sha256>` |
| Singleton / upgrade locks | `<state>/server.lock`, `<state>/upgrade.lock` | Corresponding files inside `<state>/control/` |
| Access | User-owned 0700 state, 0600 socket, peer UID check | Protected user-only directory/pipe DACL, reparse rejection, same-user peer check (no elevation filter) |
| Endpoint identity | Canonical state path | Versioned hash of SID + pinned control-directory volume serial and 128-bit file ID |

Windows identity is independent of path casing and extended-length spelling;
separate state directories remain isolated. Directory IDs are queried from the
validated pinned handle, not via an unguarded pathname. Unsupported FileIdInfo
fails explicitly instead of falling back to a potentially different name.
The hash is a namespace discriminator, never authentication.

## Resources and shutdown

`try_lock_server()` and `try_lock_upgrade()` each try once and distinguish
contention from I/O/security errors. Their guards retain the endpoint, including
the Windows directory handles. `ServerLock::listen()` consumes the actual
singleton guard; `Endpoint::bind()` is the acquire-and-listen convenience API.
A client cannot invoke socket reclamation without acquiring that lock.

Listener and every accepted Stream share the singleton lease. Dropping just the
listener does not let another daemon take over while old connections still run.
Client streams retain their own endpoint guard. `into_split()` retains resources
until **both** halves drop; it exposes no raw unauthenticated backend.

Server tracks connection tasks in a JoinSet. During idle upgrade it retains the
existing short draining period, allowing established clients to receive
`server_upgrading`, then cancels and reaps connections before dropping the
listener and releasing the final lease. The coordinator waits on the endpoint's
actual singleton lock, not on a socket filename disappearing.

Unix bind removes only a stale socket while holding the singleton lock, and
refuses a regular file or symlink at `control.sock`. Listener cleanup checks the
socket's device/inode before unlinking so it does not remove a replacement.
The old Unix address remains interoperable with legacy Tokio Unix listeners;
legacy version-handshake rejection remains explicit.

Windows listener creation rejects remote clients and uses first-instance
ownership plus an explicit user-only DACL. Tokio can defer kernel handle release
until cancelled overlapped I/O completes. While holding the singleton lease,
bind retries only first-instance AccessDenied for at most one second, yielding
between attempts; persistent collision/denial still fails without removing or
adopting the existing pipe. Directory verification errors are never retried.

Windows stream drop opts out of interprocess's flush-on-drop linger pool so an
unresponsive peer cannot keep a previous pipe instance alive after lease release.
As with the bridge's existing EOF behavior, a successful write is not a protocol
acknowledgment: buffered data can be lost on abrupt disconnect, and user requests
must be reconciled by request ID rather than replayed. Portable half-close is not
promised; close both halves to end a connection. Windows flush/shutdown on the
byte stream are no-ops, matching local-socket semantics.

## Authentication and failure

Both ends authenticate the same OS user before exposing a Stream or exchanging
application bytes, including `server_status` and `prepare_upgrade`. Unix checks
effective UID. Windows authenticates the current process's TokenUser SID without
an elevation filter; it does not trust usernames, JSON Hello, PID files or fingerprints.
The mechanisms differ at the two pipe ends:

- The client opens the actual local pipe with identification-only SQOS and verifies
  that live handle's owner SID and exact protected sole-user DACL match its current
  SID. It never queries the Server process token across integrity levels.
- The server uses identification-only `ImpersonateNamedPipeClient`, reads TokenUser
  from the connected client's thread token, reverts through an owned guard, and checks
  the kernel-reported peer PID. It requires the same SID as the Server process;
  it does not query a remote process token through `OpenProcessToken`.

This permits same-user elevation combinations subject to Windows ACL/integrity
checks. There is no privilege-bearing impersonation, token adjustment or access-check
bypass. Missing/changed peer identity or a validation failure rejects that connection.
An invalid peer is dropped; accept returns PermissionDenied and the initialized
Server may continue accepting. Other listener failures propagate. Cached endpoints
recheck the pinned control directory's ACL before connect/accept.

Windows opens only internally generated local Latte pipe names. It uses
identification-only SQOS before peer validation, never interprocess's unbounded
connect path. Only ERROR_PIPE_BUSY is retried inside the connection deadline.
A missing pipe returns promptly. No protocol bytes are sent during validation.

Only NotFound/ConnectionRefused mean `daemon_absent`. Main may start one daemon
attempt on those errors, then wait inside its existing 15-second budget. Busy
or timed-out connections to an existing endpoint do not trigger another spawn.
Security/I/O failures propagate; upgrade status retries cannot hide them or
repair permissions. Read-only status exchange retries remain separate from
upgrade admission and application requests.

## Windows object and path policy

Ordinary state uses its parent's ACL; a custom `--state-dir` does not inherit the
default LocalAppData privacy guarantee. Control/private leaves do not secure a shared
SQLite database. Control directory creation atomically supplies current-user owner
and `O:<sid>D:P(A;OICI;FA;;;<sid>)`; it never creates broadly and tightens later.
Existing objects are validated without changing owner, ACL or contents. A failed
creation readback can leave an empty leaf, never a trusted endpoint or payload.

The directory must have a present, non-NULL protected DACL with exactly one Allow
ACE, matching current SID, full access and OI|CI flags. Extra allow/deny ACEs,
unprotected inheritance, unknown ACEs, wrong owner/rights/flags and empty DACLs fail.
A new private file uses CREATE_NEW with explicit user owner/DACL and is checked before
credentials are written. Private file policy permits only one full-control user ACE,
explicit or inherited; see [Provider storage](providers.md).

Original local-disk spelling is checked before normalization. Roots, relative/UNC/
device paths, ADS, ambiguous trailing dots/spaces and reserved aliases are rejected.
UTF-16 and extended-length paths are retained. Every ancestor and leaf is opened
without following reparse points and pinned without share-delete; listing,
read-attributes and read-control rights are required. Junctions/redirected profiles
therefore may be unsupported. Ancestor permissions are not rewritten. The leaf's
owner/DACL and identity come from its pinned handle, not a later path reopen.

The pipe has its own protected current-user-only full-control DACL without directory
inheritance flags or extra SYSTEM/Administrators/Everyone entries. It rejects remote
clients and handle inheritance and uses FIRST_PIPE_INSTANCE. An existing listener
collision fails rather than adopting or deleting it. The singleton file lock remains
authoritative for Latte daemon coordination.

The pinned interprocess version's Windows local-socket wait is unsuitable for bounded
connects; the platform opens only generated local pipe names with standard-library
SQOS/OVERLAPPED options and transfers the owned handle safely. Listener descriptors
set owner/DACL without a group field, avoiding the pinned descriptor-clone issue.
Neither dependency behavior nor a namespace digest replaces authentication.

Startup prepares and validates control security, singleton and listener before database
recovery/Agent probes. Security initialization failure aborts startup. A failed peer
closes only that connection; security failures never trigger permission repair, an
alternative daemon or endpoint guessing. Diagnostics identify the failed stage without
tokens, credentials or request contents. Admin/SYSTEM, kernel compromise and malicious
same-user code/handle transfer are outside this user-isolation boundary.

## Verification

Platform/security tests exercise real process pairs and byte streams, full duplex,
concurrent clients, reconnect, isolated state, split-stream leases, pinned guards,
ACL mutation, first-instance conflicts, rejected aliases/reparse paths, peer exit,
EOF and bounded busy-pipe handling. Unix includes legacy-listener interoperability
and stale/non-socket refusal. Server native final-binary suites use a feature-gated
native CLI fixture for persistence, approval, cancellation, reconnect, upgrade and
crash reconciliation; test-fixture is excluded from production packages.

Ordinary and already-elevated Windows gates are separate. The user reports real
Windows SSH acceptance; this does not establish a full mixed-integrity or cross-user
attack matrix. Cross-user/remote impersonation attempts and target-specific access
policies require separate evidence. Cross-compilation is type/lint evidence;
Unix/macOS execution belongs to native CI. See [Development](../DEVELOPMENT.md) for
commands and acceptance boundaries and [process lifecycle](process-lifecycle.md) for
supervision.

```sh
cargo clippy -p latte-work-platform -p latte-work-windows-security --all-targets --locked -- -D warnings
cargo test -p latte-work-platform -p latte-work-windows-security --lib --locked
cargo test -p latte-work-server --test e2e upgrade_ --locked
```

The [FFI safety notes](../crates/latte-work-windows-security/SAFETY.md) define memory,
handle and descriptor ownership.
