//! Safe execution of the system `gh` binary.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use secrecy::SecretString;
use serde::Serialize;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::oneshot;

use crate::models::GhAuthStatus;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const TOKEN_TIMEOUT: Duration = Duration::from_secs(15);
const LOGIN_TIMEOUT: Duration = Duration::from_secs(10 * 60);

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// `gh` is a console application, but Shehata Git is not. Keep background
/// commands attached only to the pipes we explicitly configure instead of
/// letting Windows create a visible terminal window for them.
fn configure_background_process(command: &mut Command) {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;

        command.as_std_mut().creation_flags(CREATE_NO_WINDOW);
    }
}

/// Curated browser-login progress. Raw gh output never crosses the backend
/// boundary; only a device code matching GitHub's one-time-code shape may be
/// sent to callers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GhLoginEvent {
    Started,
    WaitingForBrowser,
    Code { code: String },
}

#[derive(Debug, Error)]
pub enum GhError {
    #[error("GitHub CLI (gh) executable not found on PATH")]
    NotFound,
    #[error("failed to spawn gh: {0}")]
    Spawn(String),
    #[error("gh command timed out after {0} seconds")]
    Timeout(u64),
    #[error("GitHub browser login was cancelled")]
    Cancelled,
    #[error("gh exited with code {code}: {message}")]
    Exit { code: i32, message: String },
    #[error("gh output was not valid UTF-8")]
    InvalidOutput,
    #[error("could not parse gh auth status JSON")]
    InvalidStatusJson,
    #[error("no token available for user '{login}' on host '{host}'")]
    TokenUnavailable { host: String, login: String },
}

/// A runner bound to a specific `gh` executable path.
#[derive(Debug, Clone)]
pub struct GhRunner {
    gh_path: PathBuf,
}

impl GhRunner {
    /// Locate the system gh executable on PATH.
    pub fn locate() -> Result<Self, GhError> {
        let path = which::which("gh").map_err(|_| GhError::NotFound)?;
        Ok(Self { gh_path: path })
    }

    /// Bind to an explicit executable path (used in tests with fake gh).
    pub fn with_path(path: impl Into<PathBuf>) -> Self {
        Self {
            gh_path: path.into(),
        }
    }

    pub fn executable(&self) -> &Path {
        &self.gh_path
    }

    async fn run(&self, args: &[&str], timeout: Duration) -> Result<(String, i32), GhError> {
        let mut command = Command::new(&self.gh_path);
        command
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        configure_background_process(&mut command);

        // Args contain at most a login name — never tokens.
        tracing::debug!(args = ?args, "running gh");

        let child = command.spawn().map_err(|e| GhError::Spawn(e.to_string()))?;
        let result = tokio::time::timeout(timeout, child.wait_with_output())
            .await
            .map_err(|_| GhError::Timeout(timeout.as_secs()))?
            .map_err(|e| GhError::Spawn(e.to_string()))?;

        let stdout = String::from_utf8(result.stdout).map_err(|_| GhError::InvalidOutput)?;
        Ok((stdout, result.status.code().unwrap_or(-1)))
    }

    /// `gh --version` first line, e.g. "gh version 2.97.0 (2026-07-31)".
    pub async fn version(&self) -> Result<String, GhError> {
        let (stdout, code) = self.run(&["--version"], DEFAULT_TIMEOUT).await?;
        if code != 0 {
            return Err(GhError::Exit {
                code,
                message: "gh --version failed".to_string(),
            });
        }
        Ok(stdout.lines().next().unwrap_or_default().trim().to_string())
    }

    /// All authenticated accounts across all hosts.
    pub async fn auth_status(&self) -> Result<GhAuthStatus, GhError> {
        // gh exits nonzero when no hosts are authenticated but still prints
        // valid JSON — so parse stdout first and only fail on bad JSON.
        let (stdout, _code) = self
            .run(&["auth", "status", "--json", "hosts"], DEFAULT_TIMEOUT)
            .await?;
        let trimmed = stdout.trim();
        if trimmed.is_empty() {
            return Ok(GhAuthStatus {
                hosts: Default::default(),
            });
        }
        serde_json::from_str(trimmed).map_err(|_| GhError::InvalidStatusJson)
    }

    /// Remove one exact account from GitHub CLI's local authentication store.
    /// This does not revoke the OAuth grant on github.com.
    pub async fn logout(&self, host: &str, login: &str) -> Result<(), GhError> {
        validate_host_and_login(host, login)?;
        let (_stdout, code) = self
            .run(
                &["auth", "logout", "--hostname", host, "--user", login],
                DEFAULT_TIMEOUT,
            )
            .await?;
        if code != 0 {
            return Err(GhError::Exit {
                code,
                message: format!("could not remove GitHub account '{login}' from this PC"),
            });
        }
        Ok(())
    }

    /// Make one already-authenticated account the GitHub CLI default for its
    /// host. Repository-scoped Shehata Git routing does not depend on this.
    pub async fn switch_active_account(&self, host: &str, login: &str) -> Result<(), GhError> {
        validate_host_and_login(host, login)?;
        let (_, code) = self
            .run(
                &["auth", "switch", "--hostname", host, "--user", login],
                DEFAULT_TIMEOUT,
            )
            .await?;
        if code != 0 {
            return Err(GhError::Exit {
                code,
                message: format!("could not make GitHub account '{login}' the CLI default"),
            });
        }
        Ok(())
    }

    /// Start the official GitHub CLI browser login for github.com.
    ///
    /// The GitHub CLI remains the credential source of truth. This method
    /// never receives or persists the resulting token and never forwards raw
    /// command output to the frontend.
    pub async fn login_web<F>(&self, on_event: F) -> Result<(), GhError>
    where
        F: Fn(GhLoginEvent) + Send + Sync + 'static,
    {
        self.browser_auth(
            &[
                "auth",
                "login",
                "--hostname",
                "github.com",
                "--git-protocol",
                "https",
                "--web",
                // Asked for up front, because a token without it cannot push a
                // change to `.github/workflows`. Requesting it afterwards
                // means every newly signed-in account is reported as needing
                // attention and sends the user back through the browser for a
                // second approval they were never told about.
                "--scopes",
                "workflow",
            ],
            on_event,
            std::future::pending(),
        )
        .await
    }

    /// Start browser login and stop the spawned GitHub CLI process when the
    /// caller sends a cancellation signal.
    pub async fn login_web_cancellable<F>(
        &self,
        on_event: F,
        cancel: oneshot::Receiver<()>,
    ) -> Result<(), GhError>
    where
        F: Fn(GhLoginEvent) + Send + Sync + 'static,
    {
        self.browser_auth(
            &[
                "auth",
                "login",
                "--hostname",
                "github.com",
                "--git-protocol",
                "https",
                "--web",
                // Asked for up front, because a token without it cannot push a
                // change to `.github/workflows`. Requesting it afterwards
                // means every newly signed-in account is reported as needing
                // attention and sends the user back through the browser for a
                // second approval they were never told about.
                "--scopes",
                "workflow",
            ],
            on_event,
            async move {
                let _ = cancel.await;
            },
        )
        .await
    }

    /// Add one OAuth scope to the currently active account through the same
    /// browser device flow used for sign-in.
    ///
    /// `gh auth refresh` always acts on the active account for the host, so
    /// callers that target a specific account must make it active first and
    /// restore the previous default afterwards.
    pub async fn refresh_scope_cancellable<F>(
        &self,
        host: &str,
        scope: &str,
        on_event: F,
        cancel: oneshot::Receiver<()>,
    ) -> Result<(), GhError>
    where
        F: Fn(GhLoginEvent) + Send + Sync + 'static,
    {
        validate_host(host)?;
        validate_scope(scope)?;
        self.browser_auth(
            &["auth", "refresh", "--hostname", host, "--scopes", scope],
            on_event,
            async move {
                let _ = cancel.await;
            },
        )
        .await
    }

    async fn browser_auth<F, C>(&self, args: &[&str], on_event: F, cancel: C) -> Result<(), GhError>
    where
        F: Fn(GhLoginEvent) + Send + Sync + 'static,
        C: std::future::Future<Output = ()> + Send,
    {
        let on_event: Arc<dyn Fn(GhLoginEvent) + Send + Sync> = Arc::new(on_event);
        on_event(GhLoginEvent::Started);

        let mut command = Command::new(&self.gh_path);
        command
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        configure_background_process(&mut command);

        tracing::debug!(args = ?args, "starting GitHub CLI browser authorization");
        let mut child = command.spawn().map_err(|e| GhError::Spawn(e.to_string()))?;

        // GitHub CLI deliberately pauses after printing/copying the device
        // code and waits for Enter before it opens the browser and starts
        // polling for completion. The desktop app has no terminal for the
        // user to press Enter in, so acknowledge that prompt over stdin.
        let mut stdin = child.stdin.take().ok_or_else(|| {
            GhError::Spawn("could not connect to GitHub CLI login input".to_string())
        })?;
        stdin
            .write_all(b"\n")
            .await
            .map_err(|e| GhError::Spawn(e.to_string()))?;
        stdin
            .shutdown()
            .await
            .map_err(|e| GhError::Spawn(e.to_string()))?;
        on_event(GhLoginEvent::WaitingForBrowser);

        let stdout = child.stdout.take().ok_or_else(|| {
            GhError::Spawn("could not capture GitHub CLI login output".to_string())
        })?;
        let stderr = child.stderr.take().ok_or_else(|| {
            GhError::Spawn("could not capture GitHub CLI login diagnostics".to_string())
        })?;
        let stdout_task = tokio::spawn(read_login_stream(stdout, Arc::clone(&on_event)));
        let stderr_task = tokio::spawn(read_login_stream(stderr, Arc::clone(&on_event)));

        tokio::pin!(cancel);
        let status = tokio::select! {
            result = child.wait() => result.map_err(|e| GhError::Spawn(e.to_string()))?,
            _ = &mut cancel => {
                let _ = child.kill().await;
                let _ = child.wait().await;
                stdout_task.abort();
                stderr_task.abort();
                return Err(GhError::Cancelled);
            }
            _ = tokio::time::sleep(LOGIN_TIMEOUT) => {
                let _ = child.kill().await;
                let _ = child.wait().await;
                stdout_task.abort();
                stderr_task.abort();
                return Err(GhError::Timeout(LOGIN_TIMEOUT.as_secs()));
            }
        };
        let _ = stdout_task.await;
        let _ = stderr_task.await;

        if !status.success() {
            return Err(GhError::Exit {
                code: status.code().unwrap_or(-1),
                message: "GitHub browser authorization was cancelled or failed".to_string(),
            });
        }
        Ok(())
    }

    /// Run the GitHub CLI with caller-supplied arguments, attached directly to
    /// this process's terminal, and return its exit code.
    ///
    /// Arguments are passed as an array and never through a shell. This is a
    /// deliberate command-line-only escape hatch: it is not reachable from the
    /// desktop app or the MCP server, so an agent cannot use it to run
    /// arbitrary GitHub CLI commands.
    pub async fn run_passthrough(&self, args: &[String]) -> Result<i32, GhError> {
        let mut command = Command::new(&self.gh_path);
        command
            .args(args)
            .stdin(std::process::Stdio::inherit())
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit());

        let mut child = command.spawn().map_err(|e| GhError::Spawn(e.to_string()))?;
        let status = child
            .wait()
            .await
            .map_err(|e| GhError::Spawn(e.to_string()))?;
        Ok(status.code().unwrap_or(-1))
    }

    /// Fetch a token for one exact account. The token is returned as a secret
    /// and must be dropped by the caller as soon as possible.
    ///
    /// This never logs the token, never writes it to disk, and never includes
    /// it in errors.
    pub async fn token_for(&self, host: &str, login: &str) -> Result<SecretString, GhError> {
        validate_host_and_login(host, login)?;
        let (stdout, code) = self
            .run(
                &["auth", "token", "--hostname", host, "--user", login],
                TOKEN_TIMEOUT,
            )
            .await?;
        if code != 0 {
            return Err(GhError::TokenUnavailable {
                host: host.to_string(),
                login: login.to_string(),
            });
        }
        // Trim newline characters only, per spec.
        let token = stdout.trim_end_matches(['\r', '\n']).to_string();
        if token.is_empty() {
            return Err(GhError::TokenUnavailable {
                host: host.to_string(),
                login: login.to_string(),
            });
        }
        Ok(SecretString::from(token))
    }
}

async fn read_login_stream<R>(reader: R, on_event: Arc<dyn Fn(GhLoginEvent) + Send + Sync>)
where
    R: AsyncRead + Unpin,
{
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if let Some(code) = extract_device_code(&line) {
            on_event(GhLoginEvent::Code { code });
        }
    }
}

fn extract_device_code(line: &str) -> Option<String> {
    line.split_whitespace()
        .map(|part| part.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-'))
        .find(|part| {
            let bytes = part.as_bytes();
            bytes.len() == 9
                && bytes[4] == b'-'
                && bytes[..4]
                    .iter()
                    .chain(&bytes[5..])
                    .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
        })
        .map(str::to_string)
}

/// Hostnames and logins become command arguments — validate them so a
/// malicious value can never become a flag (e.g. "--help") or contain
/// whitespace surprises.
fn sane_argument(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('-')
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn validate_host(host: &str) -> Result<(), GhError> {
    if sane_argument(host) {
        Ok(())
    } else {
        Err(GhError::TokenUnavailable {
            host: host.to_string(),
            login: String::new(),
        })
    }
}

/// Only the exact scopes Shehata Git asks for may reach the GitHub CLI. This
/// keeps a future caller from widening an account's permissions by accident.
fn validate_scope(scope: &str) -> Result<(), GhError> {
    const ALLOWED: [&str; 1] = ["workflow"];
    if ALLOWED.contains(&scope) {
        Ok(())
    } else {
        Err(GhError::Exit {
            code: -1,
            message: format!("unsupported GitHub scope request: {scope}"),
        })
    }
}

fn validate_host_and_login(host: &str, login: &str) -> Result<(), GhError> {
    fn sane(value: &str) -> bool {
        sane_argument(value)
    }
    if !sane(host) || !sane(login) {
        return Err(GhError::TokenUnavailable {
            host: host.to_string(),
            login: login.to_string(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_dangerous_logins() {
        assert!(validate_host_and_login("github.com", "--help").is_err());
        assert!(validate_host_and_login("github.com", "evil user").is_err());
        assert!(validate_host_and_login("", "user").is_err());
        assert!(validate_host_and_login("github.com", "ok-user_1").is_ok());
    }

    #[test]
    fn extracts_only_github_shaped_device_codes() {
        assert_eq!(
            extract_device_code("First copy your one-time code: ABCD-1EFG"),
            Some("ABCD-1EFG".to_string())
        );
        assert_eq!(extract_device_code("https://github.com/login/device"), None);
        assert_eq!(extract_device_code("token ghp_not-a-device-code"), None);
        assert_eq!(extract_device_code("abcd-1efg"), None);
    }

    #[cfg(target_os = "windows")]
    #[tokio::test]
    async fn browser_login_streams_only_curated_events_from_fake_gh() {
        use std::sync::Mutex;

        let dir = tempfile::tempdir().unwrap();
        let fake_gh = dir.path().join("gh.cmd");
        std::fs::write(
            &fake_gh,
            "@echo off\r\npowershell -NoProfile -Command \"$line = [Console]::In.ReadLine(); if ($null -eq $line) { exit 7 }; Write-Output 'First copy your one-time code: TEST-1234'; [Console]::Error.WriteLine('raw diagnostic that must stay private')\"\r\n",
        )
        .unwrap();

        let events = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&events);
        GhRunner::with_path(fake_gh)
            .login_web(move |event| captured.lock().unwrap().push(event))
            .await
            .unwrap();

        let events = events.lock().unwrap();
        assert_eq!(events.first(), Some(&GhLoginEvent::Started));
        assert!(events.contains(&GhLoginEvent::WaitingForBrowser));
        assert!(events.contains(&GhLoginEvent::Code {
            code: "TEST-1234".to_string(),
        }));
        assert_eq!(events.len(), 3, "raw gh lines must never become events");
    }

    #[cfg(target_os = "windows")]
    #[tokio::test]
    async fn browser_login_can_be_cancelled_without_leaving_gh_running() {
        let dir = tempfile::tempdir().unwrap();
        let fake_gh = dir.path().join("gh.cmd");
        std::fs::write(
            &fake_gh,
            "@echo off\r\npowershell -NoProfile -Command \"$line = [Console]::In.ReadLine(); Start-Sleep -Seconds 30\"\r\n",
        )
        .unwrap();

        let (cancel_tx, cancel_rx) = oneshot::channel();
        let runner = GhRunner::with_path(fake_gh);
        let login =
            tokio::spawn(async move { runner.login_web_cancellable(|_| {}, cancel_rx).await });
        tokio::time::sleep(Duration::from_millis(150)).await;
        cancel_tx.send(()).unwrap();

        let result = tokio::time::timeout(Duration::from_secs(3), login)
            .await
            .expect("cancel should stop the child promptly")
            .unwrap();
        assert!(matches!(result, Err(GhError::Cancelled)));
    }

    #[cfg(target_os = "windows")]
    #[tokio::test]
    async fn logout_targets_one_exact_account_without_a_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let fake_gh = dir.path().join("gh.cmd");
        std::fs::write(
            &fake_gh,
            "@echo off\r\nif not \"%1\"==\"auth\" exit /b 2\r\nif not \"%2\"==\"logout\" exit /b 3\r\nif not \"%3\"==\"--hostname\" exit /b 4\r\nif not \"%4\"==\"github.com\" exit /b 5\r\nif not \"%5\"==\"--user\" exit /b 6\r\nif not \"%6\"==\"alice\" exit /b 7\r\nexit /b 0\r\n",
        )
        .unwrap();

        GhRunner::with_path(fake_gh)
            .logout("github.com", "alice")
            .await
            .unwrap();
    }

    #[cfg(target_os = "windows")]
    #[tokio::test]
    async fn switch_targets_one_exact_account_without_a_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let fake_gh = dir.path().join("gh.cmd");
        std::fs::write(
            &fake_gh,
            "@echo off\r\nif not \"%1\"==\"auth\" exit /b 2\r\nif not \"%2\"==\"switch\" exit /b 3\r\nif not \"%3\"==\"--hostname\" exit /b 4\r\nif not \"%4\"==\"github.com\" exit /b 5\r\nif not \"%5\"==\"--user\" exit /b 6\r\nif not \"%6\"==\"alice\" exit /b 7\r\nexit /b 0\r\n",
        )
        .unwrap();

        GhRunner::with_path(fake_gh)
            .switch_active_account("github.com", "alice")
            .await
            .unwrap();
    }
}
