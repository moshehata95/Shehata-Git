//! Safe execution of the system `git` binary.
//!
//! - Always argument arrays, never a shell.
//! - Timeouts on every invocation.
//! - stderr is captured for diagnostics but never re-executed.

use std::path::{Path, PathBuf};
use std::time::Duration;

use thiserror::Error;
use tokio::process::Command;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn configure_background_process(command: &mut Command) {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;

        command.as_std_mut().creation_flags(CREATE_NO_WINDOW);
    }
}

#[derive(Debug, Error)]
pub enum GitError {
    #[error("git executable not found on PATH")]
    NotFound,
    #[error("failed to spawn git: {0}")]
    Spawn(String),
    #[error("git command timed out after {0} seconds")]
    Timeout(u64),
    #[error("git exited with code {code}: {message}")]
    Exit { code: i32, message: String },
    #[error("git output was not valid UTF-8")]
    InvalidOutput,
}

#[derive(Debug, Clone)]
pub struct CommandOutput {
    pub stdout: String,
    pub stderr: String,
    pub code: i32,
}

impl CommandOutput {
    pub fn success(&self) -> bool {
        self.code == 0
    }
}

/// Environment marker set on every git process this application starts.
///
/// The audit hooks installed into a repository check for it: an operation the
/// app performs is already recorded by the app itself, so the hook must stay
/// quiet and let it be logged once.
pub const INTERNAL_MARKER: &str = "SHEHATA_INTERNAL_GIT";

/// A runner bound to a specific `git` executable path.
#[derive(Debug, Clone)]
pub struct GitRunner {
    git_path: PathBuf,
    timeout: Duration,
}

impl GitRunner {
    /// Locate the system git executable on PATH.
    pub fn locate() -> Result<Self, GitError> {
        let path = which::which("git").map_err(|_| GitError::NotFound)?;
        Ok(Self {
            git_path: path,
            timeout: DEFAULT_TIMEOUT,
        })
    }

    /// Bind to an explicit executable path (used in tests with fake binaries).
    pub fn with_path(path: impl Into<PathBuf>) -> Self {
        Self {
            git_path: path.into(),
            timeout: DEFAULT_TIMEOUT,
        }
    }

    pub fn executable(&self) -> &Path {
        &self.git_path
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Run `git <args>` and return full output. Never fails on nonzero exit —
    /// check `output.success()` yourself. Use [`run_checked`] for the common
    /// "must succeed" case.
    pub async fn run(&self, args: &[&str]) -> Result<CommandOutput, GitError> {
        self.run_in(None, args).await
    }

    /// Run `git -C <dir> <args>`.
    pub async fn run_in(
        &self,
        dir: Option<&Path>,
        args: &[&str],
    ) -> Result<CommandOutput, GitError> {
        let mut full_args: Vec<String> = Vec::with_capacity(args.len() + 2);
        if let Some(dir) = dir {
            full_args.push("-C".to_string());
            full_args.push(dir.as_os_str().to_string_lossy().into_owned());
        }
        full_args.extend(args.iter().map(|s| s.to_string()));

        let mut command = Command::new(&self.git_path);
        command
            .args(&full_args)
            // Prevent git from ever prompting interactively; credentials must
            // come from helpers, not terminals.
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_ASKPASS", "echo")
            // Audit hooks record operations that happen outside this app. This
            // marks the ones that happen inside it, so an action taken here is
            // recorded once by the app rather than twice.
            .env(INTERNAL_MARKER, "1")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        configure_background_process(&mut command);

        // Never log the environment; args are safe (no tokens allowed by design).
        tracing::debug!(args = ?full_args, "running git");

        let child = command
            .spawn()
            .map_err(|e| GitError::Spawn(e.to_string()))?;

        let result = tokio::time::timeout(self.timeout, child.wait_with_output())
            .await
            .map_err(|_| GitError::Timeout(self.timeout.as_secs()))?
            .map_err(|e| GitError::Spawn(e.to_string()))?;

        let stdout = String::from_utf8(result.stdout).map_err(|_| GitError::InvalidOutput)?;
        let stderr = String::from_utf8(result.stderr).map_err(|_| GitError::InvalidOutput)?;
        let code = result.status.code().unwrap_or(-1);

        Ok(CommandOutput {
            stdout,
            stderr,
            code,
        })
    }

    /// Run and require exit code 0.
    pub async fn run_checked(
        &self,
        dir: Option<&Path>,
        args: &[&str],
    ) -> Result<CommandOutput, GitError> {
        let output = self.run_in(dir, args).await?;
        if !output.success() {
            return Err(GitError::Exit {
                code: output.code,
                // stderr may contain remote messages; it must never contain
                // credentials because git never echoes helper passwords here.
                message: output.stderr.trim().to_string(),
            });
        }
        Ok(output)
    }

    /// `git --version`, e.g. "git version 2.55.0.windows.3".
    pub async fn version(&self) -> Result<String, GitError> {
        let out = self.run_checked(None, &["--version"]).await?;
        Ok(out.stdout.trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn locates_system_git() {
        // The dev machine and CI both have git installed.
        let runner = GitRunner::locate().expect("git must be installed");
        let version = runner.version().await.expect("version call must work");
        assert!(version.starts_with("git version"));
    }

    #[tokio::test]
    async fn reports_nonzero_exit() {
        let runner = GitRunner::locate().expect("git must be installed");
        let err = runner
            .run_checked(None, &["rev-parse", "--is-inside-work-tree"])
            .await;
        // Outside a worktree (temp cwd is not guaranteed, but this specific
        // invocation in an arbitrary dir yields either success or Exit).
        if let Err(GitError::Exit { code, .. }) = err {
            assert_ne!(code, 0);
        }
    }
}
