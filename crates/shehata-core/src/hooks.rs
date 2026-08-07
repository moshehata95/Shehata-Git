// Copyright (c) 2026 Dr Mohamed Shehata. All rights reserved.
// Licensed under the MIT License. See LICENSE in the project root.

//! Recording Git operations that happen outside this application.
//!
//! The credential helper sees every authenticated network operation, but git's
//! credential protocol never says *what* the operation is — a push, a fetch,
//! and an `ls-remote` all look identical to it. So a repository pushed from a
//! terminal showed only "credentials served" in the activity trail, while one
//! pushed through the app showed the branch, the commit, and the change.
//!
//! Hooks close that gap. They are the only place git tells us the operation.
//!
//! Three rules shape everything here, because these scripts run inside the
//! user's own repository:
//!
//! 1. **Never break a push.** Every command is guarded so a missing binary, a
//!    locked database, or an uninstalled app cannot fail the user's operation.
//! 2. **Never disturb an existing hook.** Our block is inserted after the
//!    shebang and never calls `exit`, so a hook the user already had keeps
//!    running exactly as before.
//! 3. **Never touch stdin.** `pre-push` receives ref updates there, and a hook
//!    the user wrote may be reading them. Branch and commit are read from git
//!    instead.

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{Result, ShehataError};

/// Markers delimiting the block this application owns inside a hook file.
const SENTINEL: &str = "# >>> Shehata Git audit hook — managed block, do not edit";
const SENTINEL_END: &str = "# <<< Shehata Git audit hook";

/// Hooks this application installs, and the event each records.
const MANAGED_HOOKS: &[&str] = &["pre-push", "post-commit", "post-merge"];

fn locate_cli() -> Result<PathBuf> {
    let current = std::env::current_exe()
        .map_err(|e| ShehataError::Internal(format!("cannot locate own executable: {e}")))?;
    let cli = current.with_file_name(if cfg!(windows) {
        "shehata.exe"
    } else {
        "shehata"
    });
    if cli.is_file() {
        return Ok(cli);
    }
    which::which("shehata").map_err(|_| ShehataError::Internal("shehata CLI not found".to_string()))
}

/// Build one hook script body.
///
/// `record` is the `shehata hook-event` invocation; it is wrapped so that no
/// failure escapes, and skipped entirely when the operation came from this
/// application, which records it directly and more precisely.
fn block(cli: &str, repository_id: &str, event: &str, extra: &str) -> String {
    format!(
        "{SENTINEL}\n\
         if [ -z \"${{{marker}:-}}\" ] && [ -x '{cli}' ]; then\n\
         \x20 shehata_branch=$(git symbolic-ref --quiet --short HEAD 2>/dev/null || echo '')\n\
         \x20 shehata_commit=$(git rev-parse HEAD 2>/dev/null || echo '')\n\
         \x20 shehata_subject=$(git log -1 --format=%s 2>/dev/null || echo '')\n\
         {extra}\
         \x20 '{cli}' hook-event --repo-id '{repository_id}' --event {event} \\\n\
         \x20\x20\x20 --branch \"$shehata_branch\" --commit \"$shehata_commit\" \\\n\
         \x20\x20\x20 --subject \"$shehata_subject\" >/dev/null 2>&1 || true\n\
         fi\n\
         {SENTINEL_END}",
        marker = shehata_git::INTERNAL_MARKER,
    )
}

fn script_for(cli: &str, repository_id: &str, hook: &str) -> String {
    match hook {
        // `$1` is the remote name git is pushing to.
        "pre-push" => {
            let extra = "  shehata_remote=\"${1:-origin}\"\n";
            let body = block(cli, repository_id, "external_push", extra);
            body.replace(
                "--subject \"$shehata_subject\"",
                "--subject \"$shehata_subject\" --remote \"$shehata_remote\"",
            )
        }
        "post-commit" => block(cli, repository_id, "external_commit", ""),
        _ => block(cli, repository_id, "external_pull", ""),
    }
}

/// Install the audit hooks for one repository.
///
/// `hooks_dir` must be the directory git will actually consult. Callers that
/// have a repository record should pass the common git directory, so linked
/// worktrees share one set of hooks.
pub fn install_hooks(git_dir: &Path, repository_id: &str) -> Result<()> {
    install_hooks_with(git_dir, repository_id, &locate_cli()?)
}

/// The body of [`install_hooks`], with the CLI path supplied by the caller.
///
/// Discovery is separated from writing so the tests do not depend on this
/// machine having the app installed. They did, and so they passed on a
/// developer machine and failed on a clean CI runner — the tests were
/// asserting something about the environment rather than about this code.
fn install_hooks_with(git_dir: &Path, repository_id: &str, cli_path: &Path) -> Result<()> {
    // Hook scripts run under `sh`, which wants forward slashes even on Windows.
    let cli = cli_path.to_string_lossy().replace('\\', "/");

    let hooks_dir = git_dir.join("hooks");
    fs::create_dir_all(&hooks_dir)
        .map_err(|e| ShehataError::Internal(format!("cannot create hooks directory: {e}")))?;

    for hook in MANAGED_HOOKS {
        let script = script_for(&cli, repository_id, hook);
        write_block(&hooks_dir.join(hook), &script)?;
    }
    Ok(())
}

/// Remove this application's block from every managed hook, leaving anything
/// the user wrote untouched.
pub fn remove_hooks(git_dir: &Path) -> Result<()> {
    let hooks_dir = git_dir.join("hooks");
    for hook in MANAGED_HOOKS {
        let path = hooks_dir.join(hook);
        if path.exists() {
            strip_block(&path)?;
        }
    }
    Ok(())
}

/// Whether git will read `.git/hooks` at all for this repository.
///
/// `core.hooksPath` redirects hooks elsewhere — husky and several company
/// setups use it. Installing into `.git/hooks` there would look successful and
/// silently record nothing, which is worse than not offering the feature.
pub fn hooks_directory_is_active(local_hooks_path: Option<&str>) -> bool {
    local_hooks_path.map(str::trim).unwrap_or("").is_empty()
}

fn write_block(path: &Path, script: &str) -> Result<()> {
    const SHEBANG: &str = "#!/bin/sh";

    let existing = if path.exists() {
        fs::read_to_string(path)
            .map_err(|e| ShehataError::Internal(format!("cannot read hook: {e}")))?
    } else {
        String::new()
    };

    let content = match (existing.find(SENTINEL), existing.find(SENTINEL_END)) {
        // Replace our previous block in place.
        (Some(start), Some(end)) => {
            let mut updated = existing.clone();
            let end = end + SENTINEL_END.len();
            updated.replace_range(start..end, script);
            updated
        }
        // Fresh file.
        _ if existing.trim().is_empty() => format!("{SHEBANG}\n{script}\n"),
        // Insert directly after the shebang so our block runs before anything
        // the user's hook does — including an `exit 0` at its end.
        _ => {
            let (first, rest) = existing.split_once('\n').unwrap_or((existing.as_str(), ""));
            if first.starts_with("#!") {
                format!("{first}\n{script}\n{rest}")
            } else {
                format!("{SHEBANG}\n{script}\n{existing}")
            }
        }
    };

    fs::write(path, content)
        .map_err(|e| ShehataError::Internal(format!("cannot write hook: {e}")))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))
            .map_err(|e| ShehataError::Internal(format!("cannot make hook executable: {e}")))?;
    }
    Ok(())
}

fn strip_block(path: &Path) -> Result<()> {
    let content = fs::read_to_string(path)
        .map_err(|e| ShehataError::Internal(format!("cannot read hook: {e}")))?;

    let (Some(start), Some(end)) = (content.find(SENTINEL), content.find(SENTINEL_END)) else {
        return Ok(()); // Someone else's hook; leave it alone.
    };
    let mut end = end + SENTINEL_END.len();
    if content.as_bytes().get(end) == Some(&b'\n') {
        end += 1;
    }

    let remaining = format!("{}{}", &content[..start], &content[end..]);
    // A file that only ever held our block should not be left behind as an
    // empty script.
    if remaining.trim() == "#!/bin/sh" || remaining.trim().is_empty() {
        let _ = fs::remove_file(path);
    } else {
        fs::write(path, remaining)
            .map_err(|e| ShehataError::Internal(format!("cannot write hook: {e}")))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// A stand-in for the installed CLI. The scripts only embed this path and
    /// guard on it being executable at run time, so no real binary is needed.
    fn fake_cli() -> PathBuf {
        PathBuf::from(if cfg!(windows) {
            "C:/Program Files/Shehata Git/shehata.exe"
        } else {
            "/usr/local/bin/shehata"
        })
    }

    fn install_into(dir: &Path) {
        install_hooks_with(dir, "cd27fa83-54b9-40e4-bd22-9015e85998d9", &fake_cli()).unwrap();
    }

    #[test]
    fn installs_every_managed_hook_once() {
        let temp = TempDir::new().unwrap();
        let git_dir = temp.path().join(".git");
        install_into(&git_dir);

        for hook in MANAGED_HOOKS {
            let body = fs::read_to_string(git_dir.join("hooks").join(hook)).unwrap();
            assert!(body.starts_with("#!/bin/sh"), "{hook} needs a shebang");
            assert_eq!(body.matches(SENTINEL).count(), 1, "{hook} duplicated");
        }

        // Re-linking a repository must not stack blocks.
        install_into(&git_dir);
        let body = fs::read_to_string(git_dir.join("hooks").join("pre-push")).unwrap();
        assert_eq!(body.matches(SENTINEL).count(), 1);
    }

    #[test]
    fn an_operation_from_this_app_is_skipped() {
        let temp = TempDir::new().unwrap();
        let git_dir = temp.path().join(".git");
        install_into(&git_dir);

        let body = fs::read_to_string(git_dir.join("hooks").join("pre-push")).unwrap();
        // Without this guard the app's own push is recorded twice: once by the
        // app and once by the hook the app's `git push` triggers.
        assert!(
            body.contains(&format!(
                "[ -z \"${{{}:-}}\" ]",
                shehata_git::INTERNAL_MARKER
            )),
            "hook must stand down for operations this app performs"
        );
    }

    #[test]
    fn the_block_runs_before_a_hook_that_exits_early() {
        let temp = TempDir::new().unwrap();
        let hooks = temp.path().join(".git").join("hooks");
        fs::create_dir_all(&hooks).unwrap();
        // A hook ending in `exit 0` is common; appending after it would mean
        // this application's block never runs.
        fs::write(hooks.join("pre-push"), "#!/bin/sh\necho existing\nexit 0\n").unwrap();

        install_into(&temp.path().join(".git"));
        let body = fs::read_to_string(hooks.join("pre-push")).unwrap();

        assert!(body.contains("echo existing"), "user's hook preserved");
        assert!(
            body.find(SENTINEL).unwrap() < body.find("exit 0").unwrap(),
            "our block must come before the early exit"
        );
    }

    #[test]
    fn the_block_never_reads_stdin_or_exits() {
        let temp = TempDir::new().unwrap();
        let git_dir = temp.path().join(".git");
        install_into(&git_dir);
        let body = fs::read_to_string(git_dir.join("hooks").join("pre-push")).unwrap();

        // git delivers ref updates on stdin and a user's hook may be reading
        // them; consuming or short-circuiting would break it.
        assert!(!body.contains("read "), "must not consume stdin");
        assert!(!body.contains("\nexit "), "must not exit the user's hook");
        assert!(body.contains("|| true"), "failure must never block a push");
    }

    #[test]
    fn removal_keeps_what_the_user_wrote() {
        let temp = TempDir::new().unwrap();
        let git_dir = temp.path().join(".git");
        let hooks = git_dir.join("hooks");
        fs::create_dir_all(&hooks).unwrap();
        fs::write(hooks.join("post-commit"), "#!/bin/sh\necho mine\n").unwrap();

        install_into(&git_dir);
        remove_hooks(&git_dir).unwrap();

        let body = fs::read_to_string(hooks.join("post-commit")).unwrap();
        assert!(body.contains("echo mine"));
        assert!(!body.contains(SENTINEL));

        // A file this application created outright is removed entirely.
        assert!(!hooks.join("pre-push").exists());
    }

    #[test]
    fn a_redirected_hooks_directory_is_detected() {
        assert!(hooks_directory_is_active(None));
        assert!(hooks_directory_is_active(Some("  ")));
        assert!(!hooks_directory_is_active(Some(".husky")));
    }
}
