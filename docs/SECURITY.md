# Security model

Shehata Git reduces wrong-account pushes without becoming another credential
store. Its default trust boundary is the local machine.

## Data boundaries

### Stored locally

- Canonical repository paths and safe remote metadata
- The selected account login and host for each repository
- Optional repository-local commit author name and email
- Exact backups of Git configuration values changed by Shehata Git
- Redacted action metadata and user preferences

### Never persisted by Shehata Git

- GitHub access tokens, passwords, cookies, or authorization headers
- Environment-variable dumps
- Repository source contents or diffs
- Credential-helper password output

SQLite includes a schema guard test that fails if a credential-shaped column is
introduced.

## Credential flow

```text
git push
  └─▶ repository-local credential.helper invokes git-credential-shehata
        ├─▶ resolves the repository UUID from a validated helper argument
        ├─▶ opens SQLite read-only and finds the exact assigned login
        ├─▶ verifies that the Git remote host matches the assignment
        ├─▶ requests a token from GitHub CLI for that exact host and login
        └─▶ returns it to Git over the credential protocol, then drops it
```

If any step fails, no credentials are emitted. The repository-local helper
configuration clears inherited helpers first, preventing silent fallback to a
different account.

## Process execution

- Commands launched by the application use explicit executables and argument
  arrays. Git's `!` credential-helper syntax is a required exception; its value
  is generated only from a canonical helper path and validated repository UUID.
- Hostnames, logins, repository IDs, and paths are validated before use.
- Commands have timeouts; background console programs use no-window flags on
  Windows.
- Raw GitHub CLI login output never crosses into the frontend. Only curated
  progress events and a validated one-time device code may cross Tauri IPC.

## Git configuration

Shehata Git modifies only repository-local configuration after review. Original
values are backed up before a change and restored exactly during unlink.
Repository operations never change the GitHub CLI default account. A user may
change that default through a separate, explicit, confirmed account action.

## Files written into your repositories

Enabling routing for a repository writes two things into its `.git` directory:
the credential helper configuration, and audit hooks (`pre-push`,
`post-commit`, `post-merge`) that record operations performed outside this app.

The hooks contain no credentials. They call the `shehata` command line with the
repository id and the branch and commit git reports, and every call is guarded
so that a failure cannot break the git operation you asked for. A hook you
already had is preserved - the managed block is inserted after the shebang and
never exits - and unlinking removes the block while leaving your own content in
place.

## Push policies

A repository is either **Allow normal push** (humans and coding agents may
push) or **Block AI push** (humans may push, agents may not). There is no
"ask" state: this tool is built so automation can run, and its safety comes
from the operations that do not exist in the code rather than from prompting
during a flow that is meant to be unattended.

A caller's own confirmation - the desktop push dialog, or `--yes` on the CLI -
never grants an agent access that the policy denies.

## Diff previews

Preview is the one place where file *contents* could reach the UI, the activity
trail, or a coding agent, so it errs toward hiding. A preview is withheld when
the file name is one that holds credentials by convention (`.env*`, `.npmrc`,
`.pypirc`, `.netrc`, private keys and key stores, `terraform.tfstate`,
kubeconfig, anything under `.ssh`, `.gnupg`, `.aws`, `.kube`, or any name
containing `secret` or `password`), and also when the changed lines themselves
contain a token prefix, an `Authorization` header, or a PEM key block. Content
that trips either check is withheld whole rather than partially redacted.

## Concurrent operations

State-changing operations take a per-repository lock, so a push arriving from
an MCP client while the desktop is already pushing is refused up front rather
than failing part-way through git. The locks are in-process: they serialise the
surfaces this application owns and are not a claim to have locked the
repository against other programs.

## Deliberately unavailable operations

Force push, hard reset, clean, rebase, amend, remote deletion, and arbitrary MCP
shell execution are not implemented.

## Logs, diagnostics, and MCP

Every text that leaves the core - desktop error, CLI message, MCP envelope,
activity entry - passes through one redaction routine first. It removes GitHub
token prefixes (`ghp_`, `gho_`, `ghu_`, `ghs_`, `ghr_`, `github_pat_`, which
GitHub Enterprise Server shares), URL userinfo (`https://user:secret@host/...`),
`Authorization:` values including `Bearer`, `Basic`, and `token` schemes, and
PEM private key blocks. It deliberately leaves commit SHAs, branches, hosts, and
repository paths readable, because an error that hides those is not actionable.

Activity events contain summaries and outcomes, not tokens or file contents.
Safe diagnostics exclude account names, repository paths, remotes, environment
values, and credentials.

MCP responses use structured envelopes, never return tokens, and carry a
narrower repository projection than the desktop app: no absolute filesystem
path (which contains the local user name), no raw remote URL (where legacy
embedded credentials live), and no commit author email. An MCP client copies
tool output into its own model context and logs, so that surface is minimised
by construction rather than by redaction alone.

The credential helper path written into a repository's git config runs on every
authenticated git operation. Release builds therefore resolve it only from the
binary shipped beside the application, falling back to `PATH` with a warning;
the `SHEHATA_HELPER_PATH` override is available in debug builds only, and every
discovery path checks the expected file name.

## Vulnerability reporting

Follow the private process in the repository root [SECURITY.md](../SECURITY.md).
