# Windows security boundary

`latte-work-windows-security` is one of the two designated exceptions to the workspace's
`unsafe_code = "forbid"` rule (the other is the WebView2 boundary). Control-channel
security, owned Job supervision, helper-only ConPTY and NTFS project restoration
share its private Win32 boundary. Its only
production dependency is Microsoft's `windows-sys`. The crate defaults to
`deny(unsafe_code)`; only the private `win32` module is allowed to use unsafe.
`unsafe_op_in_unsafe_fn` and Clippy `undocumented_unsafe_blocks` remain denied,
including inside that module. Consumers and API signatures remain safe Rust;
production FFI and the opt-in native experiments stay private to win32. No global
lint has been weakened.

## Surface

- `ControlDirectory::create_or_validate(path)` takes the **control directory
  itself**. Creates only a missing leaf with a protected current-user DACL,
  then verifies it. Existing directories are never repaired or replaced.
- `ControlDirectory::open_verified(path)` never creates paths. Both constructors
  pin each checked ancestor and the leaf until the guard is dropped.
- `current_user_sid()` returns the process token's user SID, independent of
  elevation. Platform uses it to construct the separate pipe descriptor.
- `verify_pipe_peer(BorrowedHandle)` is used on the named-pipe server end. It
  impersonates the connected client at identification level and reads TokenUser
  from the thread token, then compares the SID with the server process. It does
  not open the peer process or token by PID, so medium/high same-user connections
  work. `verify_pipe_server(BorrowedHandle)` checks the actual client-side pipe handle's
  owner SID and exact protected sole-user DACL against the current process SID.
  Both ends authenticate the same user before protocol traffic without querying
  the remote process token; endpoint identity alone is not authentication.

- `ProcessJob` owns an unnamed, non-inheritable kill-on-close Job. It assigns and
  resumes a newly suspended owned child, terminates the Job, and queries active
  process accounting. It does not authenticate a peer or infer process ownership
  from protocol data. The platform retains the spawned child throughout assignment.

- `prepare_state_directory` creates ordinary missing directories with inherited
  ACLs, validating local-disk spelling and pinning each non-reparse component during
  traversal. It does not claim user-only permissions or repair an existing object.
- `create_private_file(path)` pins and validates the private parent, then creates
  a new file atomically with the token user's owner SID and a protected user-only
  DACL. This avoids the Administrators default owner of elevated tokens. It uses
  CREATE_NEW, never opens/repairs an existing file, and verifies the returned handle
  before any caller writes credentials. Share-delete permits NamedTempFile's owned
  rename/persist operation. A failed readback can leave an empty file, never payloads.
- `verify_private_file(&File)` uses the same bounded owner/ACE reader as control
  directories, with a distinct file policy: regular non-reparse file, same user,
  exactly one full-control Allow ACE with either zero flags or INHERITED_ACE.
  File DACLs may inherit from an already-verified protected private parent. Other
  flags, NULL/empty/multiple ACEs and different principals fail. No descriptor is
  modified; platform checks temp files before writing credentials and checks
  existing Provider files before reading them.
- `file_identity(&File)` exposes the full volume serial/128-bit file ID from an
  already-open handle. This is identity metadata only, not an ACL authentication API.
- `NativeSessionFile::open(path, append)` pins every ordinary ancestor and opens
  an existing non-reparse transcript with read access and optional append-only
  access. It checks the actual handle's owner against the process user, without
  requiring or changing private-storage DACLs. The full volume/file ID must be
  readable before returning. Ancestors and the leaf deny replacement throughout
  the guard lifetime; read/write sharing allows a native Agent to keep appending.
  This is same-user admission, not a promise that another ACL-authorized writer
  cannot change transcript contents. Missing, UNC/ambiguous, reparse and
  unsupported-identity paths fail without creation or permission repair.
- `open_private_file(path)` pins each ordinary ancestor while opening the leaf
  with `FILE_FLAG_OPEN_REPARSE_POINT`, read access and read-only sharing. It validates
  the same handle's regular-file attribute, owner and DACL before returning it;
  no secret bytes are read on rejection. The returned handle prevents writes and
  replacement during a bounded read. It never changes source permissions and
  supports safe legacy imports from ordinary mixed-use parent directories.
- `ManagedRecoveryRoot::create_new(project_root, expected_project_identity)`
  creates the fixed `.latte-work-undo` namespace for an ordinary project. It pins
  the project and all ancestors, verifies the expected project identity and local
  NTFS volume, then creates and verifies a token-user-owned, protected sole-user
  directory. Any existing namespace, including an empty interrupted creation,
  fails without adoption, repair or cleanup.
- `ManagedRecoveryRoot::open_verified(project_root, expected_project_identity,
  expected_store_identity)` creates nothing. It verifies both independently
  registered identities and the store's private directory policy. `identity()`
  and `project_identity()` read the pinned handles; a manifest alone never
  authorizes reopening. Registration and manifest interpretation belong to Server.
- Managed root manifest methods address only `owner.json`. Writes use a private
  CREATE_NEW file, verify it before writing and call `sync_all` before returning.
  Reads validate the same opened file's owner, protected sole-user DACL and ordinary
  project-file metadata before reading bytes, with read-only sharing. The caller's
  bound must be 1–65,536 bytes; reads allow one extra byte to detect oversized input.
  Failed writes may leave an incomplete manifest for inspection; no overwrite or
  automatic cleanup is provided.
- `ManagedRecoveryRoot::create_attempt(uuid)` creates a new private UUID-named
  directory on the project volume and returns a `RecoveryDir`. It clones every
  pinned ancestor handle, so dropping the root guard does not release the attempt's
  path protection. Existing attempts are never adopted, repaired or removed.

Platform retains its state-directory API and adds `control` exactly once before
calling the security crate. That distinction is covered by a regression test.
The earlier `0x8404` observation was a test passing the already-existing temp
parent to the leaf API. The parent correctly failed verification. A new leaf
created with the explicit descriptor has protected DACL control bits; tests
independently read its owner and SDDL through .NET. We check required flags,
not a hardcoded full control word which may include other valid Windows flags.

`ControlDirectory::identity()` reads volume serial / 128-bit file ID from the pinned
leaf handle using FileIdInfo. No pathname re-open is needed. IPC uses this identity
with the current SID to derive a stable pipe namespace across path spellings.
`ControlDirectory::verify()` rechecks the pinned leaf before connection/accept and
lock operations; directory guard lifetime prevents replacement, while this check
also detects later ACL changes. Unsupported identity queries fail explicitly.

Control-channel APIs never repair existing ACLs, adjust tokens, perform privilege-bearing
impersonation, take ownership or recursively change paths. Identification-only pipe
impersonation is narrowly allowed to read the connected client SID and always reverts. A failed control-directory creation
readback may leave an empty leaf; it never returns a trusted object or writes payloads.
Startup must propagate that error. The separately scoped terminal and project APIs
below launch owned processes and restore only explicitly requested project files.

## FFI ownership and bounds

- `OpenProcess` / `OpenProcessToken` results are uniquely owned `OwnedHandle`s
  for owned process supervision and current-process identity. Peer authentication
  does not open the peer process token; it uses server-side pipe impersonation.
- Process Jobs, process handles, thread snapshots and thread handles use
  `OwnedHandle`; no raw handle escapes. The unnamed Job has no inherited handle
  and allows no breakaway. Job limit/accounting buffers are aligned, fixed-size
  initialized structures with their exact byte lengths passed to Win32.
- Job assignment opens only the just-spawned child while the platform retains its
  original handle. The opened handle pins identity during enumeration/resumption.
  Assignment failure leaves the child suspended; the caller kills/reaps it. Resume
  failure closes the Job and kills the root. Platform has kill-on-drop enabled
  before spawn, including failures after CreateProcess succeeds.
- Thread snapshots are limited to 65,536 entries. Matching threads are opened with
  suspend/resume and limited-query rights, rechecked for owner PID, and held before
  resumption. CREATE_SUSPENDED must produce a prior suspension count of one;
  unexpected counts fail instead of pretending launch succeeded. This is for owned
  native children only, not arbitrary process attachment.
- Termination and accounting never retain caller pointers or block indefinitely.
  Platform polls accounting under an async deadline and only reports Windows close
  success after root reaping and zero ActiveProcesses. Dropping an unconfirmed Job
  is a kernel cleanup fallback, not a confirmation of successful application work.
- `TokenUser` uses a bounded (16 KiB), pointer-aligned `Vec<usize>`. Query output
  length, the embedded SID pointer, SID alignment, fixed header and subauthority
  extent are checked before SID conversion. The backing allocation is retained
  until conversion to an owned string. Elevation is not queried or used for
  authorization; the same-user comparison uses TokenUser, not the token's default owner.
- `CreateFileW` directory handles become `File` once. Each open requests listing,
  read-attributes and read-control, omits share-delete, opens the reparse point
  itself, and verifies directory/reparse attributes before walking further.
- SDDL parsing and `GetSecurityInfo` produce LocalAlloc-owned descriptors held
  by a non-cloneable `LocalMemory` guard. Only `LocalFree` frees these results.
  The same guard owns SID conversion output, including error paths.
- Returned descriptor components borrow its live allocation. Windows supplies a
  valid complete descriptor. The ACL is checked, then ACE pointer offset/header/
  size are bounded inside `AclSize`; type and exact flags precede allow-ACE
  interpretation. Unaligned fixed-prefix reads avoid alignment assumptions.
  The variable SID extent is bounded by `AceSize` before calling SID APIs.
- Paths and SDDL are terminated UTF-16 buffers alive throughout their calls.
  SID strings come from Windows, not user input; they cannot inject SDDL syntax.
  No raw pointer/handle or security descriptor is exported.

## Policy and limits

The control leaf must have the current user's owner SID, a present non-null
protected DACL and exactly one Allow ACE: full file access, exactly OI|CI flags,
and that same SID. Empty/null, inherited/unprotected, extra allow/deny,
inherit-only, different rights or identities are rejected. Changing an existing
ACL would not revoke already-open handles, so there is no repair path.

The path layer validates original spelling before normalization, rejects roots,
relative paths, UNC/device namespaces, ADS, dot components, reserved DOS names
and trailing-dot/space aliases. It preserves native UTF-16 and uses extended
local-disk paths for long names. Every ancestor is opened and pinned before the
next lookup. Reparse points (including junctions) are refused at every level;
redirected user profiles consequently may be unsupported. Ancestor ACLs are
neither changed nor required to match the leaf's exact policy.

This relies on the OS/filesystem enforcing ACL and share semantics and trusted
existing ancestor permissions. Elevated tokens are allowed under the same user-SID
policy as ordinary tokens, matching Unix's same-UID policy. No token adjustment, privilege-bearing impersonation, privilege enablement or
mandatory-integrity bypass is performed. Identification-only impersonation reads
identity without acting with client privileges. Windows may still deny mixed-integrity
access; such failures
propagate without weakening access checks.
Mapped/network volumes are not a validated deployment target. It does not defend
against administrators, SYSTEM, kernel compromise, or malicious same-user code.
Guards prevent path replacement, not same-user ACL changes. Ordinary state files
continue to use their parent permissions; this crate does not secure the entire
state tree.

Peer role and PID come from a live named-pipe handle. The server impersonates
its connected client at identification level, reads TokenUser from the thread token,
reverts through an owned guard, and rechecks the kernel peer PID. The client validates owner SID and the exact sole-user DACL on its live pipe
handle, without querying the Server process token. This is same-user OS identity
validation, not cryptographic
executable authentication or protection from malicious same-user handle transfer.
Callers must use local-only pipes and perform validation before any protocol traffic.
Platform sets explicit pipe DACLs and identification-only SQOS; this crate does not
transport application bytes.

## ConPTY and project restoration

`win32/console.rs`, `win32/project.rs` and its `project/managed.rs` child are within
the same Win32 boundary. Platform re-exports safe owned interfaces; no HANDLE, HPCON,
security descriptor pointer or unsafe requirement escapes to Server.

- PseudoConsole uniquely owns HPCON; synchronous non-inheritable pipes are Files.
  Attribute-list storage is bounded and pointer-aligned, and its destructor runs
  before the backing allocation is released. CreateProcessW receives native UTF-16,
  a bounded quoted argument buffer, explicit invalid stdio handles and no inherited
  handles. The helper is assigned to a kill-on-close Job before it can spawn the
  Shell. Clearing the helper's inherited ignore-Ctrl+C flag is a helper-local process
  setting. Root exit is checked with a zero-time wait before reading its exit code.
- ConPTY methods/Drop and pipe writes may block. They run only in the disposable
  helper; Server owns deadlines, continuously drains framed output and confirms
  Job cleanup. They are not general async APIs safe to put on a Server reactor.
- ProjectPath validates local fixed/removable NTFS, rejects reparse/short-name/.git
  aliases and holds every ancestor without share-delete. Per-file handles reject
  concurrent writers/deleters and read full identity, attributes, links, bounded
  stream information and owner/group/DACL from the same object. Named streams,
  hard links and unsupported attributes fail explicitly.
- Security descriptor allocations come from GetSecurityInfo or validated SDDL
  conversion and are uniquely LocalFree-owned. Component pointers borrow the live
  allocation through SetSecurityInfo. Returned UTF-16 is read only through its first
  terminator within the returned bound. Null/incomplete restoration descriptors fail;
  neither SACL access nor privilege adjustment is requested.
- Managed store and attempt creation pass an explicit token-user owner and
  protected sole-user full-control DACL to CreateDirectoryW. The terminated UTF-16
  path, SECURITY_ATTRIBUTES and LocalMemory-owned descriptor remain live for the
  call. Creation collisions propagate; only a successfully opened and verified
  non-reparse directory becomes a capability. Manifest protection checks use a
  GetSecurityInfo allocation owned by LocalMemory, with no borrowed pointer escaping.
  Operations recheck the pinned store's private ACL before manifest access or
  attempt creation. Project ancestry rejects ambiguous, short-name, `.git` and
  reparse aliases; ancestry ACLs are preserved rather than made private.
- StagedFile creates an exclusive new same-directory file. It sets recorded security
  and verifies exact readback before returning a content writer (inherited ACEs can
  change with the parent); then Server writes bytes, attributes and flushes. FILE_RENAME_INFO uses bounded aligned
  storage and the owned source handle. Published files are never removed by Drop;
  unpublished temporary objects are deleted through their own handle. Only those
  temporary objects may have readonly cleared for cleanup. Delete of a user target
  requires a separate validated DELETE handle and never clears its readonly flag.
- Both recovery capabilities displace an exact held source through a no-replace
  rename, check the destination identity and return the source handle. Displacement
  preserves its bytes and security descriptor; it does not rewrite it to the
  managed directory's ACL policy. The returned file only closes on Drop. Staged
  publication likewise uses no-replace rename and retains the published handle.
  The existing `RecoveryDir::create(git_dir, attempt)` keeps its Git namespace and
  inherited ACL behavior; the managed store's private-directory guarantees do not
  apply to all Git recovery roots or to displaced files.
- Project ACLs are not App private-storage ACLs. Caller preflight, owned admission,
  Unknown intent, per-file revalidation and final readback live in Server. Legacy
  rename cannot atomically compare destination identity; multi-file restoration is
  not transactional. Process death can leave staging files for manual inspection.
  See [the restoration contract](../../docs/architecture.md#per-turn-restoration-and-windows-undo).

## Test-only ConPTY and NTFS experiments

`src/experiments/` is a private child module of `win32`, included only with
`cfg(test)`. It inherits this boundary's unsafe allowance; no caller or public
production API gains one. The original experiments remain separate from the
production Console and project implementations.
All experiment entrypoints are ignored by default. Run them explicitly with
`scripts/test-windows-terminal-undo-experiments.ps1`; see
[Development](../../DEVELOPMENT.md#sidebar-terminal-development) for groups and logs.

- The disposable ConPTY helper uniquely owns HPCON and synchronous pipes. Attribute
  lists have bounded, aligned storage and are destroyed before that storage. Process
  and thread handles become OwnedHandle exactly once. The helper runs inside an
  observer-owned kill-on-close Job before it can create descendants. Blocking close
  and I/O stay inside the helper; the observer enforces a deadline and verifies Job
  accounting plus held process handles, including observed console hosts.
- The root-exit fixture deliberately leaves a descendant alive to test Job cleanup;
  its narrow `zombie_processes` allowance is test-only and documented at the function.
  Supervisor-crash tests kill only the inner supervisor while retaining an outer
  cleanup Job. No fixture enumerates and kills unrelated user processes.
- NTFS experiments operate only on tempfile-owned files. Security descriptors are
  Windows-owned LocalAlloc results; owner/group/DACL pointers borrow that allocation
  through SetSecurityInfo. No SACL, privilege adjustment or ownership takeover is
  attempted. Aligned FILE_RENAME_INFO buffers bound the UTF-16 name and length.
  PowerShell/.NET independently reads back descriptors and attributes.
- These experiments establish specific Windows 11 behavior, not a reviewed
  production filesystem restoration API or a transaction guarantee. Target rename
  races, unsupported metadata and per-file partial completion remain documented.

## Verification

The IPC facade is documented in [ipc.md](../../docs/ipc.md). It owns directory
guards and leases across Listener/Stream lifetimes; callers cannot obtain the raw
transport or skip accept-side authentication through the public API.

Native tests can run with ordinary or elevated tokens. The ordinary-user CI suite
continues to verify that administrator rights are unnecessary; the separate
`scripts/test-windows-elevated.ps1` suite requires an already elevated shell and
checks private storage, peer authentication and the real SSH bridge command.
It never requests elevation or provisions an account. Production code never invokes
PowerShell; tests use bounded disposable
fixtures and .NET ACL APIs as an independent oracle. Only fixture code restores
its own deliberately damaged ACLs for cleanup.

Tests cover private creation and readback, parent ACL preservation, read-only
rejection without repair, empty/null/unprotected/extra/deny/inherit-only ACLs,
wrong ACE flags/rights, leaf and ancestor junctions, failed-call handle release,
Unicode/long paths, ambiguous path rejection and non-pipe handle rejection.
Platform tests cover real parent/child pipe authentication in both directions,
peer exit rejection, first-instance collision, SQOS-opening round trip, remote
name rejection and busy-pipe timeout, with bounded process/I/O waits.

`project/managed_tests.rs` covers managed root/manifest/attempt private creation,
registered-identity reopening, collision and identity rejection without mutation,
manifest bounds and unprotected ACL rejection, store ACL changes without repair,
retained attempt guards after root drop, and displaced identity/security preservation.
It also covers unsafe attempt names, project/store junctions and manifest hard-link
and reparse rejection. The short-name fixture exercises alias rejection only when
the NTFS volume provides an 8.3 alias. These are native capability tests, separate
from Server registration, durable Undo recovery and real UI acceptance.

Platform's [process lifecycle tests](../../docs/process-lifecycle.md) also cover
real descendants after root exit, nested Jobs, supervisor death, task cancellation,
bounded probes, daemon survival after bridge exit and singleton/lock release.
Assignment/resume fault injection and incompatible inherited Job policies remain
separate native evidence.

Actual second-user and remote impersonation attempts require separate evidence;
passing same-user tests does not establish cross-user claims. The user reports real
local Claude, Windows SSH and installation/upgrade acceptance. No detailed integrity
matrix is inferred from that report. Current terminal/Undo limits and snapshot
compatibility are in [architecture](../../docs/architecture.md), with build and
evidence boundaries in [Development](../../DEVELOPMENT.md).

Run:

```sh
cargo clippy -p latte-work-windows-security -p latte-work-platform --all-targets --locked -- -D warnings
cargo test -p latte-work-windows-security -p latte-work-platform --lib --locked
```