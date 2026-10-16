# Latte Work contributor guide

Latte Work is a Tauri desktop client and a standalone Rust host service. Read
DEVELOPMENT.md before changing build or runtime contracts.

## Boundaries
- Rust 2024 workspace: platform (shared OS primitives), protocol (wire types),
  client (transport), config (App-owned providers), server (state, filesystem,
  process supervision, internal agent adapters), desktop (Tauri shell).
- Keep OS selection at platform/backend module boundaries. Shared platform code
  must not depend on protocol, config, server or GUI. See docs/architecture.md.
- React/TypeScript owns presentation only. Never execute agents from the UI.
- Local and SSH hosts use the same server protocol. Server has no GUI dependency.
- Claude stream-json details belong only in server/agents/claude.rs.
- Unsupported agents/capabilities must fail explicitly; never simulate execution.
- Run in an explicit feature worktree. Preserve unrelated files and local state.
- No commit, push, release publication or destructive cleanup without authorization.

## Verification commands
- `make setup`: install locked frontend dependencies (npm ci once lock exists).
- `make fmt`, `make fmt-check`, `make lint`, `make test-unit`,
  `make test-e2e`, `make test-doc`, `make test`, `make web-build`.
- `make ci`: formatting, Clippy, Rust unit/final-binary E2E tests,
  frontend typecheck/tests/build, and Rust documentation.
- `make build`: server and native desktop debug build.
- `make dev`: launch the real Tauri app with the bundled local server.
- `make package`: produce the unsigned native application and standalone server.
- `make server`: run server in the foreground; `make types`: regenerate TS types.
- `make clean`: clean this checkout's Rust artifacts only; never remove shared caches.
- `make cache-info`: print target directory and optional sccache status.
- Direct cargo/npm commands used by these targets and their focused test variants
  are supported for diagnosis. `cargo generate-lockfile` bootstraps dependencies.
- Windows desktop: `scripts/desktop.ps1 check|test|build|dev|package` prepares the locked
  sidecar Tauri requires, then runs the selected step. Windows peer authentication
  uses the same user SID without an elevation filter; retain OS access checks.
  Run ordinary-user tests and the separate `scripts/test-windows-elevated.ps1` gate.
  `package` produces an unsigned x64 per-user NSIS installer and provenance;
  `scripts/verify-windows-package.ps1` inspects embedded binaries without installing.

## Quality and safety
- For UI work, read docs/design-system.md and reuse apps/desktop/src/tokens.css.
  Keep shared control styles consistent; validate native focus and disabled states.
- Deny warnings in Clippy. All crates forbid unsafe Rust except two reviewed
  boundaries: `latte-work-windows-security/src/win32.rs` (control-channel security,
  owned Jobs, ConPTY and NTFS project restoration) and
  `latte-work-windows-webview/src/win32.rs` (WebView2 find and
  capture). Both crates default to `deny(unsafe_code)` and allow it only on their
  private win32 module; retain `unsafe_op_in_unsafe_fn` and
  `undocumented_unsafe_blocks` as deny. See the corresponding SAFETY.md files.
  Do not expand the exceptions to callers.
- Add focused tests for protocol, persistence, approval, cancellation, reconnect,
  and path-boundary changes. Final-binary tests use a deterministic CLI fixture;
  real Claude and native UI smoke evidence must be reported separately.
- Use bounded reads, queues and timeouts. Persist user requests before spawning.
- Approval is explicit, single-use, and bound to the active session/tool request.
- Never auto-replay a prompt after a transport failure. Reconcile its request ID.
- Preserve unknown/interrupted state after daemon failure; never infer success.
- SSH uses the user's OpenSSH config and host verification; no password storage.
- Do not commit .DS_Store, credentials, node_modules, target, dist, logs or databases.
