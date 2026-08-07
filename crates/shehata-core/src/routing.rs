//! Repository-scoped HTTPS credential routing.
//!
//! Tokens never enter this module. Git invokes `git-credential-shehata`, and
//! that helper obtains the assigned account's token just in time from `gh`.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use shehata_git::{
    parse_remote_url, read_local_config_values, replace_local_config_values, GitRunner,
    RemoteProtocol,
};
use shehata_storage::{queries, AccountRecord, Database, NewAuditEvent, RepositoryRecord};

use crate::error::{Result, ShehataError};

const HELPER_KEY: &str = "credential.helper";
const HTTP_PATH_KEY: &str = "credential.useHttpPath";

#[derive(Debug, Clone, Deserialize)]
pub struct LinkRepositoryRequest {
    pub repository_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RoutingResult {
    pub repository_id: String,
    pub helper_path: String,
    pub configured: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConnectionTestResult {
    pub repository_id: String,
    pub remote_name: String,
    pub account_login: String,
    pub success: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UnlinkRepositoryRequest {
    pub repository_id: String,
    #[serde(default)]
    pub restore_identity: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct UnlinkResult {
    pub repository_id: String,
    pub restored_keys: Vec<String>,
    pub identity_preserved: bool,
}

#[derive(Debug)]
struct RoutingPlan {
    repository: RepositoryRecord,
    account: AccountRecord,
    remote_name: String,
}

pub async fn link_repository(request: LinkRepositoryRequest) -> Result<RoutingResult> {
    let db_path = Database::default_path()?;
    let helper_path = locate_helper()?;
    link_repository_at(&db_path, &helper_path, request).await
}

pub async fn link_repository_at(
    db_path: &Path,
    helper_path: &Path,
    request: LinkRepositoryRequest,
) -> Result<RoutingResult> {
    let plan = load_plan(db_path, &request.repository_id)?;
    let repo_path = PathBuf::from(&plan.repository.canonical_path);
    let git = GitRunner::locate()?;
    let helper = helper_config_value(helper_path, &plan.repository.id)?;

    let previous_helpers = read_local_config_values(&git, &repo_path, HELPER_KEY).await?;
    let previous_http_path = read_local_config_values(&git, &repo_path, HTTP_PATH_KEY).await?;
    {
        let db = Database::open_at(db_path)?;
        ensure_backup(&db, &plan.repository.id, HELPER_KEY, &previous_helpers)?;
        ensure_backup(&db, &plan.repository.id, HTTP_PATH_KEY, &previous_http_path)?;
    }

    let expected_helpers = vec![String::new(), helper.clone()];
    if let Err(error) =
        replace_local_config_values(&git, &repo_path, HELPER_KEY, &expected_helpers).await
    {
        return Err(error.into());
    }
    if let Err(error) =
        replace_local_config_values(&git, &repo_path, HTTP_PATH_KEY, &["true".to_string()]).await
    {
        restore_routing_config(&git, &repo_path, &previous_helpers, &previous_http_path).await;
        return Err(error.into());
    }

    let actual_helpers = read_local_config_values(&git, &repo_path, HELPER_KEY).await?;
    let actual_http_path = read_local_config_values(&git, &repo_path, HTTP_PATH_KEY).await?;
    if actual_helpers != expected_helpers || actual_http_path != ["true"] {
        restore_routing_config(&git, &repo_path, &previous_helpers, &previous_http_path).await;
        return Err(ShehataError::Internal(
            "Git credential routing verification failed".to_string(),
        ));
    }

    audit(
        db_path,
        &plan.repository.id,
        Some(&plan.account.login),
        "credential_routing_enabled",
        "Configured repository-scoped credential routing",
        "success",
        Some(0),
        None,
    )?;

    install_audit_hooks(&git, &plan.repository).await;

    Ok(RoutingResult {
        repository_id: plan.repository.id,
        helper_path: helper_path.to_string_lossy().into_owned(),
        configured: true,
    })
}

/// Why a non-mutating `git ls-remote` probe failed.
///
/// Classification reads git's own stderr. Anything unrecognised stays
/// `Unclassified` rather than being reported as an authentication problem,
/// because sending a user to re-authenticate over a DNS outage wastes their
/// time and can push them to rotate a working token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionFailure {
    AuthenticationFailed,
    RepositoryNotFound,
    NetworkUnavailable,
    DnsFailure,
    TlsFailure,
    Timeout,
    Unclassified,
}

impl ConnectionFailure {
    fn audit_summary(self) -> &'static str {
        match self {
            Self::AuthenticationFailed => "Remote rejected the assigned account",
            Self::RepositoryNotFound => "Remote repository was not found",
            Self::NetworkUnavailable => "Remote was unreachable",
            Self::DnsFailure => "Remote host could not be resolved",
            Self::TlsFailure => "Secure connection to the remote failed",
            Self::Timeout => "Remote connection timed out",
            Self::Unclassified => "Remote authentication test failed",
        }
    }

    fn into_error(self) -> ShehataError {
        match self {
            Self::AuthenticationFailed => ShehataError::AuthenticationFailed,
            Self::RepositoryNotFound => ShehataError::OperationBlocked(
                "the remote repository was not found, or this account cannot see it".to_string(),
            ),
            Self::NetworkUnavailable => ShehataError::OperationBlocked(
                "the remote could not be reached — check the network connection".to_string(),
            ),
            Self::DnsFailure => ShehataError::OperationBlocked(
                "the remote host name could not be resolved — check DNS or the remote URL"
                    .to_string(),
            ),
            Self::TlsFailure => ShehataError::OperationBlocked(
                "the secure connection to the remote failed — check TLS interception or proxy                  settings"
                    .to_string(),
            ),
            Self::Timeout => ShehataError::OperationBlocked(
                "the remote connection timed out".to_string(),
            ),
            Self::Unclassified => ShehataError::AuthenticationFailed,
        }
    }
}

/// Map git's stderr onto a cause. Order matters: an authentication message is
/// only trusted when no transport problem is present, because git reports a
/// failed proxy handshake using authentication wording too.
pub fn classify_connection_failure(stderr: &str) -> ConnectionFailure {
    let text = stderr.to_ascii_lowercase();

    if text.contains("could not resolve host") || text.contains("name or service not known") {
        return ConnectionFailure::DnsFailure;
    }
    if text.contains("ssl certificate problem")
        || text.contains("tls")
        || text.contains("certificate verify failed")
        || text.contains("unable to get local issuer certificate")
    {
        return ConnectionFailure::TlsFailure;
    }
    if text.contains("timed out") || text.contains("timeout") {
        return ConnectionFailure::Timeout;
    }
    if text.contains("could not connect")
        || text.contains("failed to connect")
        || text.contains("network is unreachable")
        || text.contains("connection refused")
        || text.contains("connection reset")
    {
        return ConnectionFailure::NetworkUnavailable;
    }
    if text.contains("repository not found")
        || text.contains("does not appear to be a git repository")
        || text.contains("remote: not found")
    {
        return ConnectionFailure::RepositoryNotFound;
    }
    if text.contains("authentication failed")
        || text.contains("invalid username or password")
        || text.contains("permission denied")
        || text.contains("403")
        || text.contains("terminal prompts disabled")
    {
        return ConnectionFailure::AuthenticationFailed;
    }
    ConnectionFailure::Unclassified
}

pub async fn test_connection(repository_id: &str) -> Result<ConnectionTestResult> {
    let db_path = Database::default_path()?;
    test_connection_at(&db_path, repository_id).await
}

pub async fn test_connection_at(
    db_path: &Path,
    repository_id: &str,
) -> Result<ConnectionTestResult> {
    let plan = load_plan(db_path, repository_id)?;
    let repo_path = PathBuf::from(&plan.repository.canonical_path);
    let git = GitRunner::locate()?;
    let started = Instant::now();
    let output = git
        .run_in(Some(&repo_path), &["ls-remote", &plan.remote_name, "HEAD"])
        .await?;
    let duration = started.elapsed().as_millis().min(i64::MAX as u128) as i64;

    if !output.success() {
        // "Authentication failed" was the answer for every failure, including
        // an unplugged network cable. Say what actually went wrong so the user
        // knows whether to re-authenticate or check their connection.
        let failure = classify_connection_failure(&output.stderr);
        audit(
            db_path,
            &plan.repository.id,
            Some(&plan.account.login),
            "credential_connection_test",
            failure.audit_summary(),
            "failure",
            Some(output.code.into()),
            Some(duration),
        )?;
        return Err(failure.into_error());
    }

    audit(
        db_path,
        &plan.repository.id,
        Some(&plan.account.login),
        "credential_connection_test",
        "Remote authentication test succeeded",
        "success",
        Some(0),
        Some(duration),
    )?;
    Ok(ConnectionTestResult {
        repository_id: plan.repository.id,
        remote_name: plan.remote_name,
        account_login: plan.account.login,
        success: true,
    })
}

pub async fn unlink_repository(request: UnlinkRepositoryRequest) -> Result<UnlinkResult> {
    let db_path = Database::default_path()?;
    unlink_repository_at(&db_path, request).await
}

pub async fn unlink_repository_at(
    db_path: &Path,
    request: UnlinkRepositoryRequest,
) -> Result<UnlinkResult> {
    let repository = {
        let db = Database::open_at(db_path)?;
        queries::find_repository_by_id(&db, request.repository_id.trim())?
            .ok_or_else(|| ShehataError::RepositoryNotFound(request.repository_id.clone()))?
    };
    let repo_path = PathBuf::from(&repository.canonical_path);
    let backups = {
        let db = Database::open_at(db_path)?;
        queries::pending_backups(&db, &repository.id)?
    };
    if !backups.iter().any(|backup| backup.config_key == HELPER_KEY) {
        return Err(ShehataError::RepositoryNotLinked(repository.id));
    }

    let git = GitRunner::locate()?;
    let mut restored_keys = Vec::new();
    for backup in &backups {
        let is_identity = matches!(backup.config_key.as_str(), "user.name" | "user.email");
        if is_identity && !request.restore_identity {
            continue;
        }
        if !matches!(
            backup.config_key.as_str(),
            HELPER_KEY | HTTP_PATH_KEY | "user.name" | "user.email"
        ) {
            continue;
        }
        let values: Vec<String> = serde_json::from_str(&backup.previous_values_json)
            .map_err(|error| ShehataError::Internal(error.to_string()))?;
        replace_local_config_values(&git, &repo_path, &backup.config_key, &values).await?;
        restored_keys.push(backup.config_key.clone());
    }

    remove_marker(&repository)?;
    // Take the audit hooks back out; anything the user wrote stays.
    if let Some(hooks_root) = hooks_root_of(&repository) {
        if let Err(error) = crate::hooks::remove_hooks(Path::new(hooks_root)) {
            tracing::warn!("could not remove audit hooks: {error}");
        }
    }
    {
        let db = Database::open_at(db_path)?;
        // Only mark backups whose config keys were actually restored.
        // Identity backups skipped by the user remain pending so they can
        // be recovered later if needed.
        for backup in &backups {
            if restored_keys.contains(&backup.config_key) {
                queries::mark_backup_restored(&db, backup.id)?;
            }
        }
        queries::clear_repository_assignment(&db, &repository.id)?;
        queries::insert_audit_event(
            &db,
            &NewAuditEvent {
                event_type: "repository_unlinked",
                repository_id: Some(&repository.id),
                account_login: None,
                summary: "Restored repository Git configuration and removed routing",
                detail: None,
                result: "success",
                exit_code: Some(0),
                duration_ms: None,
            },
        )?;
    }

    Ok(UnlinkResult {
        repository_id: repository.id,
        restored_keys,
        identity_preserved: !request.restore_identity,
    })
}

fn load_plan(db_path: &Path, repository_id: &str) -> Result<RoutingPlan> {
    let db = Database::open_at(db_path)?;
    let repository = queries::find_repository_by_id(&db, repository_id.trim())?
        .ok_or_else(|| ShehataError::RepositoryNotFound(repository_id.to_string()))?;
    let account_id = repository
        .assigned_account_id
        .ok_or_else(|| ShehataError::RepositoryNotLinked(repository.id.clone()))?;
    let account = queries::find_account_by_id(&db, account_id)?
        .ok_or_else(|| ShehataError::RepositoryNotLinked(repository.id.clone()))?;
    if account.status != "valid" {
        return Err(ShehataError::AccountNotAvailable {
            host: account.host,
            login: account.login,
        });
    }
    let remote_name = repository.remote_name.clone().ok_or_else(|| {
        ShehataError::InvalidInput("repository has no primary remote".to_string())
    })?;
    let remote_url = repository.remote_url.as_deref().ok_or_else(|| {
        ShehataError::InvalidInput("repository has no primary remote URL".to_string())
    })?;
    let parsed = parse_remote_url(remote_url)
        .map_err(|_| ShehataError::InvalidInput("repository remote URL is invalid".to_string()))?;
    if parsed.protocol != RemoteProtocol::Https {
        return Err(ShehataError::InvalidInput(
            "credential routing requires an HTTPS remote".to_string(),
        ));
    }
    if !parsed.host.eq_ignore_ascii_case(&account.host) {
        return Err(ShehataError::InvalidInput(
            "repository host does not match assigned account".to_string(),
        ));
    }
    Ok(RoutingPlan {
        repository,
        account,
        remote_name,
    })
}

/// File name the credential helper must have, on every discovery path.
pub(crate) const HELPER_FILE_NAME: &str = if cfg!(windows) {
    "git-credential-shehata.exe"
} else {
    "git-credential-shehata"
};

pub(crate) fn locate_helper() -> Result<PathBuf> {
    // The resolved path is written into a repository's git config as a `!`
    // command, so whatever wins here runs on every authenticated git
    // operation. Release builds therefore refuse the environment override.
    locate_helper_with(cfg!(debug_assertions))
}

pub(crate) fn locate_helper_with(allow_env_override: bool) -> Result<PathBuf> {
    if allow_env_override {
        if let Some(path) = std::env::var_os("SHEHATA_HELPER_PATH") {
            let path = PathBuf::from(path);
            if has_helper_file_name(&path) && path.is_file() {
                return Ok(path);
            }
        }
    }

    // Preferred: the helper shipped beside this executable by the installer.
    if let Ok(current) = std::env::current_exe() {
        let sibling = current.with_file_name(HELPER_FILE_NAME);
        if sibling.is_file() {
            return Ok(sibling);
        }
    }

    match which::which("git-credential-shehata") {
        Ok(path) if has_helper_file_name(&path) => {
            tracing::warn!("credential helper resolved from PATH, not from the install directory");
            Ok(path)
        }
        _ => Err(ShehataError::CredentialHelperMissing),
    }
}

fn has_helper_file_name(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case(HELPER_FILE_NAME))
}

pub(crate) fn helper_config_value(path: &Path, repository_id: &str) -> Result<String> {
    if !path.is_file() {
        return Err(ShehataError::CredentialHelperMissing);
    }
    let canonical = fs::canonicalize(path).map_err(|_| ShehataError::CredentialHelperMissing)?;
    let raw = canonical.to_string_lossy();
    let path = raw.strip_prefix(r"\\?\").unwrap_or(&raw).replace('\\', "/");
    if path.contains(['\r', '\n']) || uuid::Uuid::parse_str(repository_id).is_err() {
        return Err(ShehataError::InvalidInput(
            "unsafe credential helper path or repository id".to_string(),
        ));
    }
    // `!` tells Git to execute the following fixed command as-is. This is
    // required for absolute Windows paths, which otherwise get rewritten to
    // `git-credential-<path>`. Both path and UUID are validated above.
    Ok(format!("!{} --repo-id {repository_id}", shell_quote(&path)))
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn ensure_backup(db: &Database, repository_id: &str, key: &str, values: &[String]) -> Result<()> {
    if !queries::pending_backups(db, repository_id)?
        .iter()
        .any(|backup| backup.config_key == key)
    {
        let json = serde_json::to_string(values)
            .map_err(|error| ShehataError::Internal(error.to_string()))?;
        queries::insert_config_backup(db, repository_id, key, &json)?;
    }
    Ok(())
}

async fn restore_routing_config(
    git: &GitRunner,
    repo_path: &Path,
    helpers: &[String],
    http_path: &[String],
) {
    let _ = replace_local_config_values(git, repo_path, HELPER_KEY, helpers).await;
    let _ = replace_local_config_values(git, repo_path, HTTP_PATH_KEY, http_path).await;
}

fn remove_marker(repository: &RepositoryRecord) -> Result<()> {
    let Some(git_dir) = repository.git_dir.as_deref() else {
        return Ok(());
    };
    let marker = Path::new(git_dir).join("shehata-git").join("repository-id");
    if !marker.exists() {
        return Ok(());
    }
    let existing = fs::read_to_string(&marker)
        .map_err(|error| ShehataError::RepositoryMarker(error.to_string()))?;
    if existing.trim() != repository.id {
        return Err(ShehataError::RepositoryMarker(
            "marker belongs to a different repository record".to_string(),
        ));
    }
    fs::remove_file(marker).map_err(|error| ShehataError::RepositoryMarker(error.to_string()))?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn audit(
    db_path: &Path,
    repository_id: &str,
    account_login: Option<&str>,
    event_type: &str,
    summary: &str,
    result: &str,
    exit_code: Option<i64>,
    duration_ms: Option<i64>,
) -> Result<()> {
    let db = Database::open_at(db_path)?;
    queries::insert_audit_event(
        &db,
        &NewAuditEvent {
            event_type,
            repository_id: Some(repository_id),
            account_login,
            summary,
            detail: None,
            result,
            exit_code,
            duration_ms,
        },
    )?;
    Ok(())
}

/// Where git will look for this repository's hooks.
///
/// A linked worktree keeps its own `git_dir` but shares the common directory,
/// and that is where git reads hooks from — so both must resolve to one set.
fn hooks_root_of(repository: &RepositoryRecord) -> Option<&str> {
    repository
        .git_common_dir
        .as_deref()
        .or(repository.git_dir.as_deref())
}

/// Install the hooks that record operations performed outside this app.
///
/// Failure here never fails linking: routing is what the user asked for, and
/// the trail is a supporting feature. Every outcome is logged, because a hook
/// that silently does not run would make the trail claim a completeness it
/// does not have.
async fn install_audit_hooks(git: &GitRunner, repository: &RepositoryRecord) {
    let Some(hooks_root) = hooks_root_of(repository) else {
        tracing::warn!("no git directory recorded; audit hooks not installed");
        return;
    };

    // `core.hooksPath` redirects hooks elsewhere - husky and several company
    // setups use it. Writing into `.git/hooks` there would look successful and
    // record nothing at all.
    let configured =
        read_local_config_values(git, Path::new(&repository.canonical_path), "core.hooksPath")
            .await
            .unwrap_or_default();
    if !crate::hooks::hooks_directory_is_active(configured.first().map(String::as_str)) {
        tracing::warn!(
            "core.hooksPath is set for this repository; audit hooks were not installed and \
             operations outside the app will not be recorded"
        );
        return;
    }

    match crate::hooks::install_hooks(Path::new(hooks_root), &repository.id) {
        Ok(()) => tracing::info!("audit hooks installed"),
        Err(error) => tracing::warn!("could not install audit hooks: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use shehata_storage::RepositoryRecord;
    use tempfile::TempDir;
    use uuid::Uuid;

    #[test]
    fn connection_failures_are_classified_by_cause() {
        use ConnectionFailure::*;
        let cases = [
            ("fatal: could not resolve host: github.com", DnsFailure),
            (
                "fatal: unable to access ...: SSL certificate problem",
                TlsFailure,
            ),
            (
                "ssh: connect to host github.com port 22: Connection timed out",
                Timeout,
            ),
            (
                "fatal: unable to access ...: Failed to connect to github.com",
                NetworkUnavailable,
            ),
            ("remote: Repository not found.", RepositoryNotFound),
            (
                "fatal: Authentication failed for 'https://github.com/o/r.git/'",
                AuthenticationFailed,
            ),
            ("fatal: something nobody has seen before", Unclassified),
        ];
        for (stderr, expected) in cases {
            assert_eq!(classify_connection_failure(stderr), expected, "{stderr}");
        }
    }

    #[test]
    fn a_transport_problem_is_never_reported_as_bad_credentials() {
        // git wording can mention authentication while the real cause is the
        // proxy in front of it; the transport signal has to win.
        let stderr = "fatal: unable to access: SSL certificate problem; authentication failed";
        assert_eq!(
            classify_connection_failure(stderr),
            ConnectionFailure::TlsFailure
        );
    }

    /// Helper discovery is one test, not two: both cases drive the same
    /// process-wide environment variable, and parallel tests would race.
    #[test]
    fn helper_discovery_refuses_impostors_and_release_overrides() {
        let temp = TempDir::new().unwrap();

        let impostor = temp.path().join("evil.exe");
        std::fs::write(&impostor, b"not the helper").unwrap();
        with_env_var("SHEHATA_HELPER_PATH", impostor.to_str().unwrap(), || {
            let resolved = locate_helper_with(true);
            assert!(
                resolved.as_ref().map(|p| p != &impostor).unwrap_or(true),
                "a binary named evil.exe must never be accepted as the helper"
            );
        });

        let planted = temp.path().join(HELPER_FILE_NAME);
        std::fs::write(&planted, b"planted").unwrap();
        with_env_var("SHEHATA_HELPER_PATH", planted.to_str().unwrap(), || {
            // Release behaviour: the override is not consulted at all.
            let resolved = locate_helper_with(false);
            assert!(
                resolved.as_ref().map(|p| p != &planted).unwrap_or(true),
                "release builds must ignore SHEHATA_HELPER_PATH"
            );
            // Debug behaviour: a correctly named helper is accepted.
            assert_eq!(locate_helper_with(true).unwrap(), planted);
        });
    }

    fn with_env_var(key: &str, value: &str, body: impl FnOnce()) {
        let previous = std::env::var_os(key);
        // SAFETY: only this test touches the variable, and the previous value
        // is restored before returning.
        unsafe { std::env::set_var(key, value) };
        body();
        match previous {
            Some(value) => unsafe { std::env::set_var(key, value) },
            None => unsafe { std::env::remove_var(key) },
        }
    }

    fn git(repo: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .status()
            .unwrap();
        assert!(status.success());
    }

    fn fixture() -> (TempDir, PathBuf, PathBuf, String) {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        fs::create_dir(&repo).unwrap();
        git(&repo, &["init"]);
        git(
            &repo,
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/acme/demo.git",
            ],
        );
        git(
            &repo,
            &["config", "--local", "credential.helper", "manager"],
        );
        let git_dir = repo.join(".git");
        let id = Uuid::new_v4().to_string();
        fs::create_dir_all(git_dir.join("shehata-git")).unwrap();
        fs::write(git_dir.join("shehata-git/repository-id"), &id).unwrap();
        let db_path = temp.path().join("db.sqlite");
        let db = Database::open_at(&db_path).unwrap();
        let account_id = queries::upsert_account(&db, "github.com", "alice", "valid").unwrap();
        let now = Utc::now().to_rfc3339();
        queries::insert_repository(
            &db,
            &RepositoryRecord {
                id: id.clone(),
                canonical_path: fs::canonicalize(&repo)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                git_dir: Some(
                    fs::canonicalize(&git_dir)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                ),
                git_common_dir: None,
                display_name: "demo".into(),
                host: Some("github.com".into()),
                owner: Some("acme".into()),
                repo_name: Some("demo".into()),
                remote_name: Some("origin".into()),
                remote_url: Some("https://github.com/acme/demo.git".into()),
                current_branch: Some("main".into()),
                assigned_account_id: Some(account_id),
                commit_name: None,
                commit_email: None,
                push_policy: "allow_normal_push".into(),
                created_at: now.clone(),
                updated_at: now.clone(),
                last_seen_at: Some(now),
            },
        )
        .unwrap();
        (temp, repo, db_path, id)
    }

    #[tokio::test]
    async fn links_and_unlinks_with_exact_restore() {
        let (_temp, repo, db_path, id) = fixture();
        let helper = std::env::current_exe().unwrap();
        link_repository_at(
            &db_path,
            &helper,
            LinkRepositoryRequest {
                repository_id: id.clone(),
            },
        )
        .await
        .unwrap();

        let runner = GitRunner::locate().unwrap();
        let helpers = read_local_config_values(&runner, &repo, HELPER_KEY)
            .await
            .unwrap();
        assert_eq!(helpers.len(), 2);
        assert_eq!(helpers[0], "");
        assert!(helpers[1].contains("--repo-id"));
        assert_eq!(
            read_local_config_values(&runner, &repo, HTTP_PATH_KEY)
                .await
                .unwrap(),
            ["true"]
        );

        let result = unlink_repository_at(
            &db_path,
            UnlinkRepositoryRequest {
                repository_id: id.clone(),
                restore_identity: false,
            },
        )
        .await
        .unwrap();
        assert!(result.restored_keys.contains(&HELPER_KEY.to_string()));
        assert_eq!(
            read_local_config_values(&runner, &repo, HELPER_KEY)
                .await
                .unwrap(),
            ["manager"]
        );
        assert!(read_local_config_values(&runner, &repo, HTTP_PATH_KEY)
            .await
            .unwrap()
            .is_empty());
        let db = Database::open_at(&db_path).unwrap();
        assert!(queries::find_repository_by_id(&db, &id)
            .unwrap()
            .unwrap()
            .assigned_account_id
            .is_none());
    }
}
