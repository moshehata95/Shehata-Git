# Changelog

All notable changes to Shehata Git are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and versions follow
[Semantic Versioning](https://semver.org/).

## 0.1.24 - 2026-08-07

### Fixed

- **A newly signed-in account no longer arrives needing repair.** Sign-in asked
  GitHub only for the default scopes, so every account was missing `workflow`
  the moment it was added - System check then reported it and sent you back to
  the browser for a second approval nobody mentioned beforehand. The scope is
  now requested during sign-in, where you are already approving access.

  Accounts added before this still need the one-time grant; the button in
  System check remains for them.

- **Connecting a repository shows the commit identity it will actually use.**
  The form read only the repository's own `user.name` and `user.email`. A fresh
  clone usually sets neither and inherits them from your global config, so the
  fields opened blank for the most common case. They now show what git would
  really sign a commit with.

  Backups are unaffected: unlink still restores exactly what the repository
  itself had, so an inherited value is shown here but never recorded as though
  the repository had set it.

### Added

- **Open on GitHub.** The remote name on a repository card is now a link, and
  repository details carry a matching button.

## 0.1.23 - 2026-08-07

### Added

- **Operations performed outside the app are now recorded.** A repository
  pushed from a terminal, an editor, or a coding agent used to show only
  "Credentials served", because git's credential protocol never says what the
  operation is - a push, a fetch, and an `ls-remote` look identical to it.
  Linking a repository now installs audit hooks that report the push, commit,
  or merge with the same repository, branch, commit, and change description the
  app records for its own actions.

  The hooks are written to run inside someone else's repository:

  - They never fail an operation. Every command is guarded, so a missing
    binary, a locked database, or an uninstalled app cannot break a push.
  - They never disturb an existing hook. The block is inserted after the
    shebang and never calls `exit`, so a hook you already had keeps working -
    including one ending in `exit 0`, which would otherwise have swallowed a
    block appended after it.
  - They never read stdin. `pre-push` receives ref updates there and your own
    hook may be reading them; branch and commit are read from git instead.
  - They skip the app's own git calls, so an action taken in the app is
    recorded once, precisely, instead of twice.
  - They are removed when a repository is unlinked, leaving your own hook
    content untouched.

  Where `core.hooksPath` redirects hooks elsewhere - husky and some company
  setups do this - installation is skipped and logged, rather than writing
  files git will never read.

- **The trail says who acted.** Every network entry now names its source:
  `by a coding agent`, `from the app`, or `from the command line`. This was the
  quieter of the two gaps and the larger one: an agent's push through the app
  was already recorded, but written identically to one made by hand, so a trail
  full of pushes could not answer the question this tool exists to answer.

  The wording differs by how certain the record is. A caller that reaches the
  app declares itself, so that attribution is exact. A hook can prove an
  operation happened outside the app but not who typed it, so those entries
  claim only that.

- **Activity is grouped into Operations and Credentials.** Handing a credential
  to git happens every time git asks for one, far more often than anything is
  pushed, and those rows buried the rest. Each tab carries its own count, so
  the split organises the trail without hiding any of it.

### Fixed

- Entries recorded by the hooks now carry the account the operation
  authenticated as. Without it the trail needed two rows to answer one
  question: what happened, and as whom.

## 0.1.22 - 2026-08-03

### Added

- **A version consistency check.** Four files carry the product version and
  every release bumps all four by hand. `node scripts/check-versions.mjs` fails
  when they disagree, and the release workflow additionally refuses to publish
  when the tag does not match the manifests - an installer whose file name and
  contents describe different builds is worse than a failed release.
- **Supply-chain checks in CI**: `cargo deny` (advisories, licences, sources),
  `cargo audit`, `pnpm audit --prod`, workflow linting, and secret scanning.
  They run as their own fast job so a dependency advisory does not wait behind
  a forty-minute Windows compile.
- **SHA-256 checksums** are published beside every installer.
- **Documented CLI exit codes** in the README, so a script can branch on
  outcome instead of parsing message text.

### Changed

- **Every GitHub Action is pinned to a commit**, not to a moving tag. A tag can
  be repointed by whoever owns the action; a commit cannot.
- **Internal crates are marked `publish = false`.** They ship as application
  installers and were never meant for crates.io; the marker also prevents an
  accidental publish.
- **`shehata gh` is faster and no longer times out.** It used to probe a token
  for every signed-in account before running a single command, costing a
  network round trip each - enough to exceed the GitHub CLI timeout on a busy
  connection. It now reads account state once without probing.

### Fixed

- A failure to restore the previous CLI default account after `shehata gh` is
  now logged instead of being discarded silently.

## 0.1.21 - 2026-08-03

### Changed

- **A repository now has two push policies instead of three.** The third,
  "Require approval", promised to ask a human for a decision - but there was
  nowhere to answer, so for a coding agent it simply refused, which is exactly
  what blocking does. A setting that describes itself as asking while it is
  really blocking is worse than no setting at all.

  Repositories already set to it keep the behaviour they had: they are read as
  **Block AI push**, and saving re-records them under that name. No repository
  becomes more permissive on upgrade.

- **`--yes` on `shehata push` is no longer a gate.** A push typed at a terminal
  is already a human action. The flag is still accepted so existing scripts
  keep working, and it now reads as an explicit confirmation rather than a
  requirement.

### Removed

- The `approval_required` error code, which is no longer reachable.

### Note on direction

An approval queue for agent pushes was considered and deliberately not built.
This tool exists so automation can run with the right identity; its safety
comes from making dangerous operations impossible - force push, destructive
reset, and remote deletion are absent from the code, routing fails closed, and
every action is recorded - not from interrupting safe ones. A prompt in the
middle of an automated flow would work against the reason the tool exists.

## 0.1.20 - 2026-08-03

### Added

- **One state-changing operation per repository at a time.** The desktop app,
  the CLI, an MCP client, and a terminal can all reach the same repository at
  once. A second push or pull is now refused immediately with
  `operation_in_progress` instead of colliding inside git part-way through.
  The lock is released by scope exit, so an error or panic cannot strand it.

### Changed

- **Connection tests say what actually failed.** Every failure used to be
  reported as an authentication problem, including an unreachable network. The
  probe now classifies git's own output into DNS, TLS, timeout, unreachable,
  repository-not-found, and authentication causes. A transport problem is never
  reported as bad credentials, because that sends users to rotate a working
  token over what is really a proxy or DNS fault.
- **Sensitive file detection is much wider.** Preview now also withholds
  `.npmrc`, `.pypirc`, `.netrc`, key stores (`.p12`, `.pfx`, `.jks`,
  `.keystore`), `terraform.tfstate`, kubeconfig, anything under `.ssh`,
  `.gnupg`, `.aws`, or `.kube`, and any name containing `secret` or `password`.
- **Diff previews are checked by content, not just by file name.** A file with
  an innocent name that adds a private key, an `Authorization` header, or a
  token is withheld entirely rather than partially redacted — a partial view of
  a secret is still a leak.

## 0.1.19 - 2026-08-03

### Security

- One redaction routine now guards every boundary. It covers GitHub token
  prefixes, URL userinfo (`https://user:secret@host/...`), `Authorization:`
  values for the `Bearer`, `Basic`, and `token` schemes, and PEM private key
  blocks - while deliberately leaving commit SHAs, branches, and repository
  paths readable so errors stay actionable.
- The CLI and the MCP server now redact their error output. Both previously
  emitted error text verbatim; the MCP path matters most, because a coding
  agent copies tool output into its own context and logs.
- MCP repository results use a narrower projection than the desktop app: no
  absolute filesystem path (which contains the local user name), no raw remote
  URL (where legacy embedded credentials live), and no commit author email.
- Credential helper discovery is hardened. The resolved path is written into a
  repository's git config as a `!` command, so release builds now ignore the
  `SHEHATA_HELPER_PATH` override entirely, every candidate must have the
  expected file name, and a `PATH` fallback is logged as a warning.

## 0.1.18 - 2026-08-03

### Changed

- **All SQLite connections now set `busy_timeout(5s)`.** The read-write path
  (`open_at`) previously had no busy timeout, causing `SQLITE_BUSY` errors when
  the credential helper read while the desktop app wrote. Both paths now wait
  up to 5 seconds before failing.

- **WAL synchronous mode set to NORMAL.** Reduces fsync overhead while
  retaining durability against application crashes. Only an OS-level crash
  could lose the last committed transaction — acceptable for local-only,
  non-financial data.

- **Migrations are now atomic.** Each migration SQL + `user_version` bump is
  wrapped in a single `BEGIN EXCLUSIVE … COMMIT` transaction. A crash or error
  mid-migration rolls back cleanly, preventing half-applied schema states.

- **Unix file permissions restricted.** On Unix systems, the database file is
  set to `0600` (owner read/write only) and its parent directory to `0700`
  (owner access only). Windows relies on `%LOCALAPPDATA%` ACL inheritance.

## 0.1.17 - 2026-08-02

### Security

- **Credential helper now enforces exact repository path scoping.** Previously
  the helper validated only the host; now it compares the requested `path` field
  against the linked repository's `owner/repo_name`. Missing path data is denied
  (fail-closed). Requests with embedded credentials in the URL field are also
  rejected. This closes a credential-routing gap where any git request to the
  same host could receive the token of a different repository.

- **Remote URLs containing embedded credentials are rejected.** The remote URL
  parser now refuses `https://user:token@host/...` URLs, query strings,
  fragments, and extra path segments. The `raw` URL field has been removed from
  `RemoteUrl`; only a safe `canonical_url()` reconstructed from parsed
  components is stored. This prevents accidental token persistence in SQLite.

- **Timed-out git and GitHub CLI processes are now killed on drop.** Both
  process runners set `kill_on_drop(true)`, ensuring that a timed-out push,
  pull, or authentication command cannot continue executing in the background
  after the user is told it failed.

- **Unlink backup state is now accurate.** When unlinking with
  `restore_identity=false`, identity backups (user.name, user.email) are no
  longer falsely marked as restored. Only actually-restored configuration keys
  are marked, preserving the ability to recover original values later.

### Added

- Author credit embedded across the project: `Cargo.toml` workspace `authors`
  field, copyright headers in every crate entry point and the Tauri bridge,
  `package.json` `author` field, and a visible Author section in the README.
  These survive forks and ensure the original creator is attributed regardless
  of how the project is distributed.

## 0.1.16 - 2026-08-02

### Added

- The credential helper (`git-credential-shehata`) now writes a best-effort
  audit event every time it serves or denies credentials. Any `git push` or
  `git pull` invoked outside the app — from an IDE, terminal, or AI coding
  agent — now appears in the audit log. On success a `credential_served` event
  is recorded; on denial a `credential_denied` event records the specific
  reason (host mismatch, missing assignment, token failure, etc.). The audit
  write is fire-and-forget and never blocks the credential flow.

## 0.1.15 - 2026-08-02

### Fixed

- A network action no longer refuses to run because of a stale account state.
  A token probe that failed during an outage used to stay recorded as
  unavailable until the accounts page was refreshed by hand; live GitHub CLI
  state is now re-read once before the action is refused. Routing still fails
  closed — it never falls through to a different account.

## 0.1.14 - 2026-08-02

### Changed

- Failed and blocked network actions now carry the same context as successful
  ones — repository, branch, and remote — instead of one bare sentence. A
  failure is when that context matters most.
- The activity trail can be sorted newest or oldest first.

## 0.1.13 - 2026-08-02

### Changed

- Activity entries are now two lines instead of one long sentence: the change
  itself is the title, and repository, branch, and short commit sit on a
  quieter line beneath it. Pushes stay labelled "Normal push" there, because
  the trail is where the never-force-push guarantee has to remain visible.
- Activity search also matches the repository, branch, and commit line.

## 0.1.12 - 2026-08-02

### Changed

- Activity entries now identify what an action touched: repository, branch,
  short commit, and the commit subject — instead of one fixed sentence that
  looked identical for every repository. Commit subjects are redacted and
  truncated before being stored.

## 0.1.11 - 2026-08-01

### Added

- `shehata gh <command>` runs any GitHub CLI command as the account assigned to
  the current repository, then restores the previous CLI default. Git already
  routed per repository; this closes the same gap for `gh` commands such as
  `gh pr create`. The passthrough is command-line only and is not exposed to
  the desktop app or the MCP server.

## 0.1.10 - 2026-08-01

### Changed

- Automatic setup now detects whether Windows Package Manager exists before
  offering to install Git and GitHub CLI. When it is missing, the panel
  explains why and links to App Installer instead of failing after the click.

## 0.1.9 - 2026-08-01

### Added

- System check can now repair a missing `workflow` scope in place. Choosing
  **Grant workflow access** opens GitHub's own approval flow for that exact
  account and restores the previous CLI default account afterwards, including
  when the request fails or is cancelled.

### Security

- Only scopes on an explicit allow-list may be requested from the GitHub CLI,
  so a future caller cannot widen an account's permissions by accident.

## 0.1.8 - 2026-08-01

### Changed

- The audit trail now refreshes itself every few seconds while the page is
  open, so actions performed from a terminal or a coding agent appear without
  a manual refresh. Polling stops while the window is in the background, and
  it reads only the local database — no Git or GitHub CLI process is launched.
- Overview no longer describes its state as "live", which promised realtime
  updates the app does not perform.
- Documentation leads with the problem the app solves, links directly to the
  Windows and macOS installers, and shows real product screenshots.

## 0.1.7 - 2026-08-01

### Added

- First-publish support: pushing a branch that has never been pushed now
  creates the remote branch and records it as upstream in one safe step,
  instead of failing with `no_upstream`. Smart Sync previews such branches
  as ahead-only.
- macOS CI build producing an unsigned `.dmg` artifact on every push.

### Fixed

- Pushes rejected by GitHub for a token missing the `workflow` scope now
  surface a clear, actionable message instead of a raw git error, and the
  Doctor flags signed-in accounts whose token lacks that scope.

## 0.1.6 - 2026-08-01

### Added

- Explicit, confirmed switching of the GitHub CLI default account without
  changing repository identity assignments.
- Public project documentation, sanitized screenshot placeholders, community
  templates, and clean-environment CI.
- Smart Sync readiness guidance, searchable working-tree filters, visible-file
  selection, and an in-app safe-push confirmation.

### Changed

- Renamed the ambiguous `active in gh` state to `CLI default` and clarified
  that all accounts marked `ready` remain available for repository routing.
- Refined all search controls into a consistent Liquid Glass search surface.
- Replaced remaining native Windows confirmation prompts with branded in-app
  confirmation dialogs and simplified verbatim Windows paths for display.

### Security

- Smart Sync still fetches before deciding, permits only fast-forward pulls or
  normal pushes, and stops when local and remote history diverge.
- Copyable diagnostics no longer include executable paths that can contain a
  local Windows username.

## 0.1.5 - 2026-08-01

### Added

- Searchable account and repository selectors that remain usable with long
  lists.
- Collapsible repository cards and searchable account, repository, and audit
  panels.
- Per-event audit deletion and complete local audit-history clearing.
- Real cancellation for GitHub browser login and repeat-copy confirmation for
  one-time codes.
- Official Shehata Git branding across the app, executable, Windows installer,
  and macOS icon asset.

### Changed

- Repository route checks now run concurrently with a bounded worker count.
- Query caching avoids unnecessary refreshes when navigating between panels.
- `Workspace density` is now the clearer `Layout spacing` control.

### Security

- All background Git and GitHub CLI processes remain hidden on Windows and use
  fixed executables plus argument arrays.
- Browser-login cancellation terminates the child GitHub CLI process.

## 0.1.0 - 2026-07-31

### Added

- Initial Tauri desktop, Rust core, CLI, credential helper, MCP server, SQLite
  storage, repository-scoped credential routing, safe Git actions, diagnostics,
  and NSIS packaging foundation.
