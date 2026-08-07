# Build log

This file records verified engineering milestones without machine-specific
paths, account names, repository names, credentials, or private test data.

## 2026-08-06 - v0.1.23 recording work done outside the app

Found by using the tool: a push to another repository from a terminal left
nothing in the trail but `credential_served`. The database confirmed the shape
of it - the repository pushed through the app had 27 `push` events, and every
repository pushed from a terminal had none.

Git hooks are the only place git states the operation, so that is where this
had to be solved. Three problems in the first implementation were found and
fixed before shipping:

- **Double counting.** The app runs `git push` itself, which triggers
  `pre-push`, so an action taken in the app would have been recorded twice -
  once by the app and once by its own hook. The git runner now sets
  `SHEHATA_INTERNAL_GIT` on every process it starts and the hook stands down
  when it sees it. Proved against a real repository: the count went from zero
  to one, not to two.

- **Appending after an early exit.** The block was appended to the end of an
  existing hook. A hook that ends in `exit 0` is ordinary, and the block would
  never have run. It is now inserted immediately after the shebang.

- **`core.hooksPath` ignored.** When it is set, `.git/hooks` is not read at
  all. Installation would have reported success and recorded nothing, which is
  worse than not offering the feature - the trail would claim a completeness it
  did not have. It is now detected and skipped with a warning.

Also corrected: hooks are written to the common git directory, so a linked
worktree shares one set rather than silently having none.

The `hook-event` command treats everything it receives as untrusted, because it
is called from a shell script: the repository id must be a canonical UUID, the
event must be one of the three hooks installed, and free text is redacted,
stripped of control characters, and length-bounded before it is stored. Entries
are shaped like the app's own, so the trail reads as one history.

Verified end to end against a real repository rather than only in unit tests: a
terminal push recorded one correctly-shaped entry, the same push with the
internal marker recorded nothing, and a pre-existing user hook still ran.

126 Rust tests pass.

## 2026-08-07 - v0.1.23 recording what happens outside the app

Found by using the tool: a push to another repository left nothing in the trail
but a `credential_served` row. Querying the database showed the shape of it -
27 `push` events for this repository, which is pushed through the app, and zero
for every repository pushed from a terminal.

- Added `hooks`: `pre-push`, `post-commit`, and `post-merge` scripts written
  into a repository when routing is enabled. Design constraints that mattered:
  - The app marks its own git invocations with an environment variable and the
    hook skips when it sees it. Without that, every push through the app would
    have been recorded twice - once well, once poorly.
  - The block is inserted after the shebang, ahead of any existing hook body. A
    hook ending in `exit 0` is common, and appending would have meant the
    audit silently never ran.
  - Every hook ends in `|| true` and redirects output. An audit trail must
    never be the reason a push fails.
- Added `ActionCaller::label`, and carried the caller through `NetworkPlan` into
  the audit writer. The caller is declared at the boundary, so this is exact -
  unlike the hooks, which can say an operation happened outside the app but
  cannot prove whether a person or an agent typed it. The two are worded
  differently on purpose.
- `hook-event` resolves the repository's assigned account and records it, so a
  single row answers both what happened and as whom.
- Split the activity trail into Operations and Credentials, defaulting to
  Operations, with counts on both tabs. Credential rows were 90 of 169.

### On the version number

All of this shipped as 0.1.23 rather than as several releases. 0.1.23 existed
only in the manifests and had never been tagged or published, so each fix
belonged in the same unreleased box. Publishing 0.1.23 and 0.1.24 minutes apart
would have invented a version nobody could ever have run.

## 2026-08-03 - v0.1.22 CI and supply chain

- Added `scripts/check-versions.mjs`, in Node rather than shell so it runs the
  same way on the Windows and macOS runners. Wired into CI and, with the tag
  as an argument, into the release workflow.
- Added a `security` job on ubuntu so advisories report in minutes instead of
  waiting behind the Windows quality gate.
- Pinned all ten actions to commit SHAs, each with the tag and the date it was
  resolved so a future maintainer can tell what a pin actually is.
- Ran `cargo deny` locally before wiring it into CI, which is what caught two
  false alarms that would have shipped a permanently red pipeline:
  - Ten `unmaintained` findings for the GTK3 bindings Tauri uses for its Linux
    backend - code that never ships in the Windows and macOS builds and that
    this project cannot fix. `unmaintained = "workspace"` keeps the signal for
    dependencies this workspace actually chose.
  - `wildcard` errors for this repository's own path dependencies.
    `allow-wildcard-paths` does not apply to crates that look publishable, so
    the honest fix was `publish = false` on every internal crate - correct
    metadata that also blocks an accidental `cargo publish`.
- Deliberately did **not** apply the plan's `prerelease: true`. GitHub excludes
  prereleases from `releases/latest`, which is exactly what the README download
  buttons resolve through; it would have traded an honest label for broken
  downloads. The unsigned state is stated in the README and every release note.
- Checksum step written to work on both runner families: macOS ships `shasum`,
  Windows and Linux ship `sha256sum`.
- Fixed `run_gh_as`, found by using it: publishing the v0.1.21 notes failed
  with a 30-second GitHub CLI timeout because the function probed a token for
  every account before running one command. It now reads `gh auth status` once.

## 2026-08-03 - v0.1.21 honest push policies

- Reduced `PushPolicy` to `AllowNormalPush` and `BlockAiPush`. `parse()` still
  accepts the retired `ask_before_push` and maps it to `BlockAiPush`, so old
  rows load and old behaviour is preserved without a data migration.
- `enforce_push_policy` no longer has an approval branch. The `approved`
  argument is kept in the signature because a human confirming at their own
  keyboard is meaningful, but it can never grant an agent access the policy
  denies - a test now asserts exactly that.
- Removed the `ApprovalRequired` error variant and its CLI repair hint; both
  were unreachable once the policy was gone.
- Removed the third option from both policy selects in the desktop, and made
  `normalizePushPolicy` fold the retired value into `block_ai_push`.
- Merged the two credential-helper environment tests into one. They both drove
  the same process-wide variable and raced under the parallel test runner - a
  reminder that a test touching global state cannot be split for readability.

### Design note

The phase plan called for a full approval workflow here: a `pending_approvals`
table, expiry, single-use nonces, and a desktop approval card. Work on it was
started and then reversed after checking it against what the product is for.
Shehata Git exists so automation runs with the correct identity. Its safety
model is structural - the dangerous operations do not exist in the code - and
adding a human prompt into an automated push contradicts that. What remained
worth doing was removing the setting that lied about it.

## 2026-08-03 - v0.1.20 operation safety

- Added `locking`: a per-repository async mutex registry. `try_lock_repository`
  refuses rather than queues, so a caller learns immediately instead of waiting
  on a network operation it cannot see. Guards release on scope exit, which is
  what makes the error path safe. Wired into push and pull.
- Documented the honest limit in the module: the locks serialise the surfaces
  this process owns. They are not a claim to have locked the repository against
  other programs, and are not a security boundary.
- Added `ConnectionFailure` classification over git stderr. Ordering is
  deliberate - transport signals are checked before authentication wording,
  because git reports a failed proxy or TLS handshake using authentication
  words too. Unrecognised output stays `Unclassified` instead of being called
  an authentication failure.
- Widened `is_sensitive_diff_path` and added `diff_content_is_sensitive`, which
  inspects only added and removed lines so a context line that merely names a
  token variable does not blank an otherwise useful preview.
- Added 13 tests (4 locking, 2 classification, 4 sensitive content, plus
  expanded path coverage). Total: 120 Rust tests, 7 frontend tests.

### Deviation from the phase plan

Phase 4 also specifies the human approval workflow for agent pushes. It is held
for its own release rather than shipped here: the backend alone would have MCP
return an approval id that the desktop has no screen to approve, which is
strictly worse than the current refusal. It ships together with its UI.

## 2026-08-03 - v0.1.19 secret redaction & MCP minimisation

- Expanded `redact` into a single `redact_secrets()` entry point covering token
  prefixes, URL userinfo, `Authorization` schemes (`Bearer`/`Basic`/`token`),
  and PEM private key blocks. Written by hand rather than with a regex crate to
  avoid a new dependency on a security-critical path.
- Confirmed the routine is idempotent and that commit SHAs, branches, and hosts
  survive it - an error message that hides those cannot be acted on.
- Found and closed a real gap: the CLI and MCP crates had **zero** redaction
  calls. The desktop had 35. Both now redact at their error boundaries.
- Added `McpRepository`, an MCP-only projection excluding `canonical_path`,
  `remote_url`, `remote_protocol`, `commit_name`, and `commit_email`.
- Hardened `locate_helper()`: extracted `locate_helper_with(allow_env_override)`
  so both behaviours are testable, gated the environment override behind
  `cfg!(debug_assertions)`, added a file-name check on every discovery path,
  and downgraded the `PATH` fallback to a logged warning.
- Added 21 tests (14 redaction, 5 MCP projection, 2 helper discovery). Total:
  111 Rust tests, 7 frontend tests.

## 2026-08-03 — v0.1.18 database & concurrency hardening

- Added `busy_timeout(5000)` to all write connections (`open_at`). Read-only
  already had it; now both paths tolerate concurrent access.
- Set `synchronous=NORMAL` for WAL mode — reduces fsync calls.
- Wrapped each migration SQL + `user_version` update in `BEGIN EXCLUSIVE …
  COMMIT`. Crash mid-migration now rolls back cleanly.
- Added `#[cfg(unix)]` file permissions: `0600` on DB file, `0700` on parent
  directory. Windows relies on `%LOCALAPPDATA%` ACL inheritance.
- Added 5 new tests: busy_timeout, synchronous, WAL mode, concurrent
  reader/writer, atomic migration consistency. Total: 95+ tests.

## 2026-08-02 — v0.1.17 author attribution + security hardening

- Added `authors` field to the workspace `Cargo.toml` — published with every
  crate and embedded in compiled binary metadata.
- Added copyright headers to all 8 crate/app entry points (4 lib.rs, 4 main.rs).
- Added `author` field to the desktop `package.json`.
- Added an Author section to the README with name, title, and GitHub profile.
- **P0-1 fix**: Credential helper now validates exact repository path
  (`owner/repo`) against the linked record, not just the host. Missing path
  denied (fail-closed). Embedded credentials in the URL field rejected.
- **P0-2 fix**: Remote URL parser rejects userinfo, query strings, fragments,
  and extra path segments. `RemoteUrl.raw` field removed; `canonical_url()`
  reconstructs a safe URL from parsed components only.
- **P1-1 fix**: Both Git and GitHub CLI process runners set `kill_on_drop(true)`
  so timed-out child processes are terminated immediately.
- **P1-2 fix**: Unlink with `restore_identity=false` no longer marks identity
  backups as restored. Only actually-restored keys are marked.

## 2026-08-02 — v0.1.16 credential helper audit logging

- The credential helper (`git-credential-shehata`) now writes a best-effort
  audit event every time it serves or denies credentials. This closes a blind
  spot where `git push` or `git pull` invoked outside the app (from an IDE,
  terminal, or AI coding agent) would succeed via the helper but never appear
  in the audit log.
- On success: a `credential_served` event is recorded with the repository
  display name and the account login.
- On denial (host mismatch, missing assignment, token failure, etc.): a
  `credential_denied` event is recorded with the specific reason.
- The audit write opens a separate read-write database connection and is
  fire-and-forget — if it fails, the credential flow is unaffected.
- Passed full quality gate: `cargo fmt`, Clippy (warnings denied), 78 Rust
  tests, 7 frontend tests, TypeScript typecheck.

## 2026-08-01 — Public repository preparation

- Reworked the README, roadmap, contribution guide, security policy, changelog,
  community templates, and CI workflow for an open-source release.
- Removed stale internal handoff material and sanitized historical build notes.
- Added branded screenshot placeholders so real screenshots can be reviewed for
  personal data before publication.
- Clarified GitHub CLI default-account behavior in the UI and CLI.
- Added an explicit, confirmed default-account switch that never changes
  repository assignments.
- Refined search focus styling across account, repository, picker, and activity
  surfaces.
- Removed executable paths from the copyable diagnostic report and added a
  regression test for that privacy boundary.
- Fixed clean-checkout CI ordering so Tauri sidecars are built before Rust
  workspace validation.
- Verified a dependency-free working-tree copy: frozen pnpm install, sidecar
  release build, frontend lint/typecheck/tests/production build, Rust format,
  Clippy with warnings denied, and all workspace tests passed.
- Built the current Windows NSIS installer locally after the public-release
  changes. The unsigned artifact remains local and unpublished.

## 2026-08-01 — v0.1.6 Smart Sync workspace polish

- Replaced silent disabled sync controls with a remote, identity, and route
  readiness checklist plus a direct setup action.
- Consolidated Smart Sync to one primary action and replaced its native push
  prompt with the shared Liquid Glass confirmation dialog.
- Added working-tree search, staged/changed/untracked filters, visible-file
  selection, and human-readable Windows paths.
- Replaced the remaining native confirmation prompts for setup, unlink, and
  normal push with consistent in-app dialogs.
- Added unit coverage for workspace filtering and Windows path display.
- Passed the full Rust and frontend quality gate, built the Windows NSIS
  installer, installed v0.1.6, and verified the installed sidebar version,
  repository workspace, Smart Sync readiness guard, search/filter controls,
  and in-app default-account confirmation flow.

## 2026-08-01 — v0.1.5 desktop workflow refinement

- Adopted the official Shehata Git logo across in-app and bundle assets.
- Added scalable searchable selectors, list search, and collapsed repository
  cards.
- Added real browser-login cancellation and repeat-copy confirmation.
- Added local audit-event deletion and full history clearing.
- Reduced repeated UI fetching and parallelized independent repository route
  checks with a bounded concurrency limit.
- Built and smoke-tested the Windows NSIS installer locally. No artifact was
  published.

## 2026-07-31 — Initial product foundation

- Created the Tauri/React desktop, shared Rust core, storage, Git/GitHub runners,
  CLI, credential helper, and MCP server.
- Implemented system diagnostics, account discovery, repository registration,
  identity assignment, credential routing, safe Git actions, audit events, and
  configuration backup/restore.
- Added unit, integration, protocol-contract, and security guard tests using
  temporary repositories and fake binaries.
