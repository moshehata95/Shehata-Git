//! Policy-checked local Git actions.
//!
//! This surface intentionally exposes only selected-path staging, unstaging,
//! and normal commits. Every dynamic value is passed as an argument, never as
//! shell text, and `--` terminates options before file paths.

use std::path::{Component, Path};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use shehata_git::{
    parse_remote_url, read_local_config_values, GitError, GitRunner, RemoteProtocol,
};
use shehata_github::GhRunner;
use shehata_storage::{queries, AccountRecord, Database, NewAuditEvent, RepositoryRecord};

use crate::error::{Result, ShehataError};
use crate::models::PushPolicy;

const MAX_DIFF_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ChangeEntry {
    pub path: String,
    pub index_status: String,
    pub worktree_status: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RepositoryActionStatus {
    pub repository_id: String,
    pub branch: Option<String>,
    pub detached_head: bool,
    pub changes: Vec<ChangeEntry>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DiffSummary {
    pub repository_id: String,
    pub changed_paths: usize,
    pub staged_paths: usize,
    pub unstaged_paths: usize,
    pub untracked_paths: usize,
    pub conflict_paths: usize,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FileDiffRequest {
    pub repository_id: String,
    pub path: String,
    #[serde(default)]
    pub staged: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct FileDiff {
    pub repository_id: String,
    pub path: String,
    pub staged: bool,
    pub content: String,
    pub truncated: bool,
    pub sensitive: bool,
    pub blocked_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PathsRequest {
    pub repository_id: String,
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CommitRequest {
    pub repository_id: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct GitActionResult {
    pub repository_id: String,
    pub action: String,
    pub changed_paths: usize,
    pub commit: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ActionCaller {
    Desktop,
    Cli,
    Mcp,
}

impl ActionCaller {
    /// How this caller is named in the activity trail.
    ///
    /// Whether a push came from a person or from a coding agent is the single
    /// most important fact in the trail for this tool, and it is known exactly
    /// here - the caller is declared at the boundary, not guessed.
    pub fn label(self) -> &'static str {
        match self {
            Self::Desktop => "from the app",
            Self::Cli => "from the command line",
            Self::Mcp => "by a coding agent",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct RepositoryActionRequest {
    pub repository_id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PushRequest {
    pub repository_id: String,
    pub caller: ActionCaller,
    #[serde(default)]
    pub approved: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SetPushPolicyRequest {
    pub repository_id: String,
    pub push_policy: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PushPolicyResult {
    pub repository_id: String,
    pub push_policy: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct NetworkActionResult {
    pub repository_id: String,
    pub action: String,
    pub remote_name: String,
    pub branch: String,
    pub account_login: String,
    pub head_commit: String,
    pub ahead_before: usize,
    pub behind_before: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SyncPreview {
    pub repository_id: String,
    pub remote_name: String,
    pub branch: String,
    pub account_login: String,
    pub ahead: usize,
    pub behind: usize,
}

#[derive(Debug)]
struct NetworkPlan {
    repository: RepositoryRecord,
    account: AccountRecord,
    remote_name: String,
    remote_branch: String,
    branch: String,
    ahead: usize,
    behind: usize,
    /// True when the branch has never been published: the push must create
    /// the remote branch and record it as upstream in one step.
    set_upstream: bool,
    /// Which surface asked for this. `None` for reads that no one initiated
    /// as a state change, such as a sync preview.
    caller: Option<ActionCaller>,
}

pub async fn status(repository_id: &str) -> Result<RepositoryActionStatus> {
    let db_path = Database::default_path()?;
    status_at(&db_path, repository_id).await
}

pub async fn diff_summary(repository_id: &str) -> Result<DiffSummary> {
    let status = status(repository_id).await?;
    Ok(summarize_status(status))
}

pub async fn file_diff(request: FileDiffRequest) -> Result<FileDiff> {
    let db_path = Database::default_path()?;
    file_diff_at(&db_path, request).await
}

async fn file_diff_at(db_path: &Path, request: FileDiffRequest) -> Result<FileDiff> {
    let repository = load_repository(db_path, &request.repository_id)?;
    let path = validate_paths(vec![request.path])?
        .into_iter()
        .next()
        .expect("one validated path");
    if is_sensitive_diff_path(&path) {
        return Ok(FileDiff {
            repository_id: repository.id,
            path,
            staged: request.staged,
            content: String::new(),
            truncated: false,
            sensitive: true,
            blocked_reason: Some(
                "Preview hidden because this filename may contain credentials or secrets."
                    .to_string(),
            ),
        });
    }

    let repo_path = Path::new(&repository.canonical_path);
    let git = GitRunner::locate()?;
    let output = if request.staged {
        git.run_in(
            Some(repo_path),
            &[
                "diff",
                "--cached",
                "--no-ext-diff",
                "--no-color",
                "--unified=3",
                "--",
                &path,
            ],
        )
        .await?
    } else {
        let tracked = git
            .run_in(
                Some(repo_path),
                &["ls-files", "--error-unmatch", "--", &path],
            )
            .await?
            .success();
        if tracked {
            git.run_in(
                Some(repo_path),
                &[
                    "diff",
                    "--no-ext-diff",
                    "--no-color",
                    "--unified=3",
                    "--",
                    &path,
                ],
            )
            .await?
        } else {
            git.run_in(
                Some(repo_path),
                &[
                    "diff",
                    "--no-index",
                    "--no-ext-diff",
                    "--no-color",
                    "--unified=3",
                    "--",
                    "/dev/null",
                    &path,
                ],
            )
            .await?
        }
    };
    if !matches!(output.code, 0 | 1) {
        return Err(GitError::Exit {
            code: output.code,
            message: output.stderr.trim().to_string(),
        }
        .into());
    }
    let (content, truncated) = truncate_utf8(output.stdout, MAX_DIFF_BYTES);
    // A file can pass the name check and still contain a key. Withhold the
    // whole preview rather than redacting it: a partial view of a secret is
    // still a leak, and the user can always open the file themselves.
    if diff_content_is_sensitive(&content) {
        return Ok(FileDiff {
            repository_id: repository.id,
            path,
            staged: request.staged,
            content: String::new(),
            truncated: false,
            sensitive: true,
            blocked_reason: Some(
                "Preview hidden because the change appears to contain a credential or private key."
                    .to_string(),
            ),
        });
    }
    Ok(FileDiff {
        repository_id: repository.id,
        path,
        staged: request.staged,
        content,
        truncated,
        sensitive: false,
        blocked_reason: None,
    })
}

pub async fn sync_preview(repository_id: &str) -> Result<SyncPreview> {
    let db_path = Database::default_path()?;
    sync_preview_at(&db_path, repository_id, true).await
}

async fn sync_preview_at(
    db_path: &Path,
    repository_id: &str,
    check_token: bool,
) -> Result<SyncPreview> {
    let plan = prepare_network_plan(db_path, repository_id, None, false, check_token, true).await?;
    Ok(SyncPreview {
        repository_id: plan.repository.id,
        remote_name: plan.remote_name,
        branch: plan.branch,
        account_login: plan.account.login,
        ahead: plan.ahead,
        behind: plan.behind,
    })
}

fn summarize_status(status: RepositoryActionStatus) -> DiffSummary {
    let staged_paths = status
        .changes
        .iter()
        .filter(|change| change.index_status != " " && change.index_status != "?")
        .count();
    let unstaged_paths = status
        .changes
        .iter()
        .filter(|change| change.worktree_status != " " && change.worktree_status != "?")
        .count();
    let untracked_paths = status
        .changes
        .iter()
        .filter(|change| change.index_status == "?" && change.worktree_status == "?")
        .count();
    let conflict_paths = status
        .changes
        .iter()
        .filter(|change| {
            matches!(
                (
                    change.index_status.as_str(),
                    change.worktree_status.as_str()
                ),
                ("D", "D")
                    | ("A", "U")
                    | ("U", "D")
                    | ("U", "A")
                    | ("D", "U")
                    | ("A", "A")
                    | ("U", "U")
            )
        })
        .count();
    DiffSummary {
        repository_id: status.repository_id,
        changed_paths: status.changes.len(),
        staged_paths,
        unstaged_paths,
        untracked_paths,
        conflict_paths,
    }
}

pub fn set_push_policy(request: SetPushPolicyRequest) -> Result<PushPolicyResult> {
    let db_path = Database::default_path()?;
    set_push_policy_at(&db_path, request)
}

fn set_push_policy_at(db_path: &Path, request: SetPushPolicyRequest) -> Result<PushPolicyResult> {
    let repository = load_repository(db_path, &request.repository_id)?;
    require_assignment(&repository)?;
    let policy = PushPolicy::parse(request.push_policy.trim()).ok_or_else(|| {
        ShehataError::InvalidInput("unsupported repository push policy".to_string())
    })?;
    let db = Database::open_at(db_path)?;
    queries::update_repository_push_policy(&db, &repository.id, policy.as_str())?;
    queries::insert_audit_event(
        &db,
        &NewAuditEvent {
            event_type: "push_policy_changed",
            repository_id: Some(&repository.id),
            account_login: None,
            summary: "Updated repository push policy",
            detail: None,
            result: "success",
            exit_code: Some(0),
            duration_ms: None,
        },
    )?;
    Ok(PushPolicyResult {
        repository_id: repository.id,
        push_policy: policy.as_str().to_string(),
    })
}

pub async fn status_at(db_path: &Path, repository_id: &str) -> Result<RepositoryActionStatus> {
    let repository = load_repository(db_path, repository_id)?;
    let path = Path::new(&repository.canonical_path);
    let git = GitRunner::locate()?;
    let branch_output = git
        .run_in(path.into(), &["symbolic-ref", "--quiet", "--short", "HEAD"])
        .await?;
    let branch = branch_output
        .success()
        .then(|| branch_output.stdout.trim().to_string())
        .filter(|value| !value.is_empty());
    let head_exists = git
        .run_in(path.into(), &["rev-parse", "--verify", "HEAD"])
        .await?
        .success();
    let output = git
        .run_checked(
            Some(path),
            &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
        )
        .await?;
    Ok(RepositoryActionStatus {
        repository_id: repository.id,
        detached_head: branch.is_none() && head_exists,
        branch,
        changes: parse_porcelain_v1_z(&output.stdout),
    })
}

pub async fn stage(request: PathsRequest) -> Result<GitActionResult> {
    let db_path = Database::default_path()?;
    stage_at(&db_path, request).await
}

pub async fn stage_at(db_path: &Path, request: PathsRequest) -> Result<GitActionResult> {
    run_paths_action(db_path, request, "stage", "add").await
}

pub async fn unstage(request: PathsRequest) -> Result<GitActionResult> {
    let db_path = Database::default_path()?;
    unstage_at(&db_path, request).await
}

pub async fn unstage_at(db_path: &Path, request: PathsRequest) -> Result<GitActionResult> {
    run_paths_action(db_path, request, "unstage", "restore").await
}

pub async fn commit(request: CommitRequest) -> Result<GitActionResult> {
    let db_path = Database::default_path()?;
    commit_at(&db_path, request).await
}

pub async fn commit_at(db_path: &Path, request: CommitRequest) -> Result<GitActionResult> {
    let repository = load_repository(db_path, &request.repository_id)?;
    require_assignment(&repository)?;
    let message = validate_commit_message(&request.message)?;
    let path = Path::new(&repository.canonical_path);
    let git = GitRunner::locate()?;

    let conflicts = git
        .run_checked(
            Some(path),
            &["diff", "--cached", "--name-only", "--diff-filter=U"],
        )
        .await?;
    if !conflicts.stdout.trim().is_empty() {
        return Err(ShehataError::ConflictsPresent);
    }
    let staged = git
        .run_in(Some(path), &["diff", "--cached", "--quiet", "--exit-code"])
        .await?;
    if staged.code == 0 {
        return Err(ShehataError::InvalidInput(
            "there are no staged changes to commit".to_string(),
        ));
    }
    if staged.code != 1 {
        return Err(GitError::Exit {
            code: staged.code,
            message: staged.stderr.trim().to_string(),
        }
        .into());
    }

    let started = Instant::now();
    let output = git
        .run_checked(Some(path), &["commit", "-m", &message])
        .await;
    match output {
        Ok(_) => {
            let commit = git
                .run_checked(Some(path), &["rev-parse", "HEAD"])
                .await?
                .stdout
                .trim()
                .to_string();
            write_audit(
                db_path,
                &repository.id,
                "commit",
                "Created a normal commit",
                "success",
                Some(0),
                started,
            )?;
            Ok(GitActionResult {
                repository_id: repository.id,
                action: "commit".to_string(),
                changed_paths: 0,
                commit: Some(commit),
            })
        }
        Err(error) => {
            write_audit(
                db_path,
                &repository.id,
                "commit",
                "Normal commit failed",
                "failure",
                git_error_code(&error),
                started,
            )?;
            Err(error.into())
        }
    }
}

pub async fn pull_ff_only(request: RepositoryActionRequest) -> Result<NetworkActionResult> {
    let db_path = Database::default_path()?;
    pull_ff_only_at(&db_path, request, true).await
}

async fn pull_ff_only_at(
    db_path: &Path,
    request: RepositoryActionRequest,
    check_token: bool,
) -> Result<NetworkActionResult> {
    let _guard = crate::locking::try_lock_repository(request.repository_id.trim())?;
    let started = Instant::now();
    let plan = prepare_network_plan(
        db_path,
        &request.repository_id,
        None,
        false,
        check_token,
        false,
    )
    .await?;
    let git = GitRunner::locate()?;
    let path = Path::new(&plan.repository.canonical_path);
    let result = execute_pull(&git, path, &plan.remote_name, &plan.remote_branch).await;
    finish_network_action(db_path, plan, "pull_ff_only", result, started).await
}

pub async fn push(request: PushRequest) -> Result<NetworkActionResult> {
    let db_path = Database::default_path()?;
    push_at(&db_path, request, true).await
}

async fn push_at(
    db_path: &Path,
    request: PushRequest,
    check_token: bool,
) -> Result<NetworkActionResult> {
    // Held until this function returns, so a second push arriving from another
    // surface is refused up front instead of colliding inside git.
    let _guard = crate::locking::try_lock_repository(request.repository_id.trim())?;
    let started = Instant::now();
    let plan_result = prepare_network_plan(
        db_path,
        &request.repository_id,
        Some(request.caller),
        request.approved,
        check_token,
        true,
    )
    .await;
    let plan = match plan_result {
        Ok(plan) => plan,
        Err(error) => {
            if uuid::Uuid::parse_str(request.repository_id.trim()).is_ok() {
                write_network_audit(
                    db_path,
                    request.repository_id.trim(),
                    None,
                    "push_preflight",
                    "Push preflight blocked the operation",
                    None,
                    "blocked",
                    None,
                    started,
                )?;
            }
            return Err(error);
        }
    };

    write_network_audit(
        db_path,
        &plan.repository.id,
        Some(&plan.account.login),
        "push_preflight",
        "Push preflight passed",
        Some(format!("{} · {}", plan.repository.display_name, plan.branch).as_str()),
        "success",
        Some(0),
        started,
    )?;
    let git = GitRunner::locate()?;
    let path = Path::new(&plan.repository.canonical_path);
    let result = execute_push(
        &git,
        path,
        &plan.remote_name,
        &plan.remote_branch,
        plan.set_upstream,
    )
    .await;
    finish_network_action(db_path, plan, "push", result, started).await
}

async fn prepare_network_plan(
    db_path: &Path,
    repository_id: &str,
    push_caller: Option<ActionCaller>,
    approved: bool,
    check_token: bool,
    allow_missing_upstream: bool,
) -> Result<NetworkPlan> {
    let (repository, account) = match load_linked_repository(db_path, repository_id) {
        Err(ShehataError::AccountNotAvailable { .. }) => {
            // The stored account state can be stale: a token probe that failed
            // during a network outage stays recorded as unavailable. Re-read
            // live GitHub CLI state once before refusing, so a temporary
            // problem does not require a manual refresh to clear.
            refresh_account_mirror(db_path).await;
            load_linked_repository(db_path, repository_id)?
        }
        other => other?,
    };
    let repo_path = Path::new(&repository.canonical_path);
    let git = GitRunner::locate()?;

    let helper_path = crate::routing::locate_helper()?;
    let expected_helper = crate::routing::helper_config_value(&helper_path, &repository.id)?;
    ensure_routing_configured(&git, repo_path, &expected_helper, &repository.id).await?;

    let expected_url = repository.remote_url.as_deref().ok_or_else(|| {
        ShehataError::InvalidInput("repository has no expected remote URL".to_string())
    })?;
    let expected = parse_remote_url(expected_url)
        .map_err(|_| ShehataError::InvalidInput("expected remote URL is invalid".to_string()))?;
    if expected.protocol != RemoteProtocol::Https
        || !expected.host.eq_ignore_ascii_case(&account.host)
    {
        return Err(ShehataError::AuthenticationFailed);
    }

    let branch_output = git
        .run_in(
            Some(repo_path),
            &["symbolic-ref", "--quiet", "--short", "HEAD"],
        )
        .await?;
    let branch = if branch_output.success() {
        branch_output.stdout.trim().to_string()
    } else {
        return Err(ShehataError::DetachedHead);
    };
    validate_git_ref(&git, repo_path, &branch).await?;

    let conflicts = git
        .run_checked(Some(repo_path), &["diff", "--name-only", "--diff-filter=U"])
        .await?;
    if !conflicts.stdout.trim().is_empty() {
        return Err(ShehataError::ConflictsPresent);
    }

    let upstream_output = git
        .run_in(
            Some(repo_path),
            &[
                "rev-parse",
                "--abbrev-ref",
                "--symbolic-full-name",
                "@{upstream}",
            ],
        )
        .await?;
    let (remote_name, remote_branch, set_upstream) = if upstream_output.success() {
        let upstream = upstream_output.stdout.trim();
        let (remote, tracked) = upstream
            .split_once('/')
            .filter(|(remote, branch)| !remote.is_empty() && !branch.is_empty())
            .ok_or(ShehataError::NoUpstream)?;
        (remote.to_string(), tracked.to_string(), false)
    } else if allow_missing_upstream {
        // First publish: no upstream exists yet, so route through the
        // registered remote and let the push create the tracking branch.
        let remote = repository
            .remote_name
            .clone()
            .ok_or(ShehataError::NoUpstream)?;
        (remote, branch.clone(), true)
    } else {
        return Err(ShehataError::NoUpstream);
    };
    let remote_name = remote_name.as_str();
    let remote_branch = remote_branch.as_str();
    validate_remote_name(remote_name)?;
    validate_git_ref(&git, repo_path, remote_branch).await?;

    let actual_url = git
        .run_checked(Some(repo_path), &["remote", "get-url", remote_name])
        .await?
        .stdout;
    let actual = parse_remote_url(actual_url.trim())
        .map_err(|_| ShehataError::InvalidInput("upstream remote must use HTTPS".to_string()))?;
    if actual.protocol != RemoteProtocol::Https
        || !actual.host.eq_ignore_ascii_case(&expected.host)
        || !actual.owner.eq_ignore_ascii_case(&expected.owner)
        || !actual.repo.eq_ignore_ascii_case(&expected.repo)
    {
        return Err(ShehataError::OperationBlocked(
            "upstream remote does not match the linked repository".to_string(),
        ));
    }

    if check_token {
        let gh = GhRunner::locate().map_err(|_| ShehataError::AuthenticationFailed)?;
        let token = gh
            .token_for(&account.host, &account.login)
            .await
            .map_err(|_| ShehataError::AuthenticationFailed)?;
        drop(token);
    }

    // Refresh only remote-tracking refs. No merge, checkout, or worktree write.
    git.run_checked(
        Some(repo_path),
        &["fetch", "--quiet", "--prune", remote_name],
    )
    .await?;
    let (ahead, behind) = if set_upstream {
        let remote_ref = format!("refs/remotes/{remote_name}/{remote_branch}");
        let remote_exists = git
            .run_in(
                Some(repo_path),
                &["rev-parse", "--quiet", "--verify", &remote_ref],
            )
            .await?
            .success();
        if remote_exists {
            let range = format!("HEAD...{remote_ref}");
            let counts = git
                .run_in(
                    Some(repo_path),
                    &["rev-list", "--left-right", "--count", &range],
                )
                .await?;
            if !counts.success() {
                return Err(ShehataError::NoUpstream);
            }
            parse_ahead_behind(&counts.stdout)?
        } else {
            // The remote branch does not exist yet: every local commit is
            // ahead and nothing can be behind.
            let commits = git
                .run_checked(Some(repo_path), &["rev-list", "--count", "HEAD"])
                .await?;
            let ahead = commits.stdout.trim().parse::<usize>().map_err(|_| {
                ShehataError::InvalidInput("could not count local commits".to_string())
            })?;
            (ahead, 0)
        }
    } else {
        let counts = git
            .run_in(
                Some(repo_path),
                &["rev-list", "--left-right", "--count", "HEAD...@{upstream}"],
            )
            .await?;
        if !counts.success() {
            return Err(ShehataError::NoUpstream);
        }
        parse_ahead_behind(&counts.stdout)?
    };

    if push_caller.is_some() && behind > 0 {
        return Err(ShehataError::NonFastForward);
    }
    if let Some(caller) = push_caller {
        enforce_push_policy(&repository.push_policy, caller, approved)?;
    }

    Ok(NetworkPlan {
        repository,
        account,
        remote_name: remote_name.to_string(),
        remote_branch: remote_branch.to_string(),
        branch,
        ahead,
        behind,
        set_upstream,
        caller: push_caller,
    })
}

async fn finish_network_action(
    db_path: &Path,
    plan: NetworkPlan,
    action: &str,
    result: std::result::Result<shehata_git::CommandOutput, GitError>,
    started: Instant,
) -> Result<NetworkActionResult> {
    match result {
        Ok(_) => {
            let head_commit = GitRunner::locate()?
                .run_checked(
                    Some(Path::new(&plan.repository.canonical_path)),
                    &["rev-parse", "HEAD"],
                )
                .await?
                .stdout
                .trim()
                .to_string();
            let subject = subject_of_head(Path::new(&plan.repository.canonical_path)).await;
            let (summary, detail) = network_action_lines(
                action,
                &plan.repository.display_name,
                &plan.branch,
                &head_commit,
                subject.as_deref(),
                plan.caller,
            );
            write_network_audit(
                db_path,
                &plan.repository.id,
                Some(&plan.account.login),
                action,
                &summary,
                Some(detail.as_str()),
                "success",
                Some(0),
                started,
            )?;
            Ok(NetworkActionResult {
                repository_id: plan.repository.id,
                action: action.to_string(),
                remote_name: plan.remote_name,
                branch: plan.branch,
                account_login: plan.account.login,
                head_commit,
                ahead_before: plan.ahead,
                behind_before: plan.behind,
            })
        }
        Err(error) => {
            write_network_audit(
                db_path,
                &plan.repository.id,
                Some(&plan.account.login),
                action,
                &format!(
                    "{} failed",
                    match (action, plan.caller) {
                        ("push", Some(caller)) => format!("Normal push {}", caller.label()),
                        ("push", None) => "Normal push".to_string(),
                        (_, Some(caller)) => format!("Fast-forward pull {}", caller.label()),
                        _ => "Fast-forward pull".to_string(),
                    }
                ),
                Some(
                    format!(
                        "{} · {} · {}",
                        plan.repository.display_name, plan.branch, plan.remote_name
                    )
                    .as_str(),
                ),
                "failure",
                git_error_code(&error),
                started,
            )?;
            if action == "push" {
                if let GitError::Exit { message, .. } = &error {
                    if message.contains("workflow` scope") {
                        return Err(ShehataError::OperationBlocked(
                            "GitHub rejected the push because this account's token lacks the \
                             `workflow` scope needed to update .github/workflows files. Run \
                             `gh auth refresh -h github.com -s workflow`, approve in the \
                             browser, then push again."
                                .to_string(),
                        ));
                    }
                }
            }
            Err(error.into())
        }
    }
}

async fn ensure_routing_configured(
    git: &GitRunner,
    repo_path: &Path,
    expected_helper: &str,
    repository_id: &str,
) -> Result<()> {
    let helpers = read_local_config_values(git, repo_path, "credential.helper").await?;
    let use_http_path = read_local_config_values(git, repo_path, "credential.useHttpPath").await?;
    let configured = helpers.first().is_some_and(String::is_empty)
        && helpers.iter().any(|helper| helper == expected_helper)
        && use_http_path.iter().any(|value| value == "true");
    if !configured {
        return Err(ShehataError::RepositoryNotLinked(repository_id.to_string()));
    }
    Ok(())
}

/// Re-read live GitHub CLI accounts into the local mirror.
///
/// Failure is deliberately ignored: this only ever tries to clear a stale
/// "unavailable" mark, and the caller re-checks the stored state afterwards.
async fn refresh_account_mirror(db_path: &Path) {
    let Ok(gh) = GhRunner::locate() else {
        return;
    };
    let Ok(accounts) = crate::accounts::list_accounts(&gh).await else {
        return;
    };
    if let Ok(db) = Database::open_at(db_path) {
        crate::accounts::mirror_accounts(&db, &accounts);
    }
}

fn load_linked_repository(
    db_path: &Path,
    repository_id: &str,
) -> Result<(RepositoryRecord, AccountRecord)> {
    let repository = load_repository(db_path, repository_id)?;
    let db = Database::open_at(db_path)?;
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
    Ok((repository, account))
}

async fn validate_git_ref(git: &GitRunner, repo_path: &Path, value: &str) -> Result<()> {
    if value.contains(['\0', '\r', '\n']) {
        return Err(ShehataError::InvalidInput("invalid Git ref".to_string()));
    }
    let output = git
        .run_in(Some(repo_path), &["check-ref-format", "--branch", value])
        .await?;
    if !output.success() {
        return Err(ShehataError::InvalidInput("invalid Git ref".to_string()));
    }
    Ok(())
}

fn validate_remote_name(value: &str) -> Result<()> {
    if value.is_empty()
        || value.starts_with('-')
        || value.len() > 255
        || value.contains(['\0', '\r', '\n'])
    {
        return Err(ShehataError::InvalidInput(
            "invalid upstream remote name".to_string(),
        ));
    }
    Ok(())
}

fn parse_ahead_behind(value: &str) -> Result<(usize, usize)> {
    let mut values = value.split_whitespace();
    let ahead = values
        .next()
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| ShehataError::Internal("invalid ahead/behind output".to_string()))?;
    let behind = values
        .next()
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| ShehataError::Internal("invalid ahead/behind output".to_string()))?;
    if values.next().is_some() {
        return Err(ShehataError::Internal(
            "invalid ahead/behind output".to_string(),
        ));
    }
    Ok((ahead, behind))
}

/// Decide whether this caller may push to this repository.
///
/// `approved` is the caller's own confirmation — the desktop's push dialog or
/// the CLI's `--yes`. It is kept in the signature because a human confirming
/// at their own keyboard is meaningful, but it never grants an agent access
/// that the policy denies.
fn enforce_push_policy(policy: &str, caller: ActionCaller, approved: bool) -> Result<()> {
    let _ = approved;
    let policy = PushPolicy::parse(policy).ok_or_else(|| {
        ShehataError::OperationBlocked("repository push policy is invalid".to_string())
    })?;
    match policy {
        PushPolicy::AllowNormalPush => Ok(()),
        PushPolicy::BlockAiPush if caller == ActionCaller::Mcp => Err(
            ShehataError::OperationBlocked("AI pushes are blocked for this repository".to_string()),
        ),
        PushPolicy::BlockAiPush => Ok(()),
    }
}

/// Subject line of the commit at HEAD, for the activity trail.
///
/// A commit message is author-written text, so it is redacted and truncated
/// before it can reach the audit database.
async fn subject_of_head(repo_path: &Path) -> Option<String> {
    const MAX_SUBJECT: usize = 60;
    let git = GitRunner::locate().ok()?;
    let output = git
        .run_in(Some(repo_path), &["log", "-1", "--format=%s"])
        .await
        .ok()?;
    if !output.success() {
        return None;
    }
    let subject = crate::redact::redact_secrets(output.stdout.trim());
    if subject.is_empty() {
        return None;
    }
    let (clean, truncated) = truncate_utf8(subject, MAX_SUBJECT);
    Some(if truncated {
        format!("{}…", clean.trim_end())
    } else {
        clean
    })
}

/// Split an activity entry into a title and a detail line.
///
/// The title is what the action actually did — the commit subject — so the
/// trail reads like a list of changes rather than a list of identical
/// sentences. The detail line carries the safety label and the context:
/// pushes stay named "Normal push" because this trail is where the
/// never-force-push guarantee has to remain visible.
fn network_action_lines(
    action: &str,
    repository: &str,
    branch: &str,
    head_commit: &str,
    subject: Option<&str>,
    caller: Option<ActionCaller>,
) -> (String, String) {
    let base = if action == "push" {
        "Normal push"
    } else {
        "Fast-forward pull"
    };
    let label = match caller {
        Some(caller) => format!("{base} {}", caller.label()),
        None => base.to_string(),
    };
    let short_commit = head_commit.chars().take(7).collect::<String>();
    let title = subject
        .map(str::to_string)
        .unwrap_or_else(|| format!("{label} completed"));
    let detail = format!("{label} · {repository} · {branch} · {short_commit}");
    (title, detail)
}

async fn execute_pull(
    git: &GitRunner,
    repo_path: &Path,
    remote_name: &str,
    remote_branch: &str,
) -> std::result::Result<shehata_git::CommandOutput, GitError> {
    git.run_checked(
        Some(repo_path),
        &["pull", "--ff-only", "--no-edit", remote_name, remote_branch],
    )
    .await
}

async fn execute_push(
    git: &GitRunner,
    repo_path: &Path,
    remote_name: &str,
    remote_branch: &str,
    set_upstream: bool,
) -> std::result::Result<shehata_git::CommandOutput, GitError> {
    let destination = format!("HEAD:refs/heads/{remote_branch}");
    let mut args = vec!["push", "--porcelain"];
    if set_upstream {
        args.push("--set-upstream");
    }
    args.push(remote_name);
    args.push(&destination);
    git.run_checked(Some(repo_path), &args).await
}

async fn run_paths_action(
    db_path: &Path,
    request: PathsRequest,
    action: &str,
    git_action: &str,
) -> Result<GitActionResult> {
    let repository = load_repository(db_path, &request.repository_id)?;
    require_assignment(&repository)?;
    let paths = validate_paths(request.paths)?;
    let git = GitRunner::locate()?;
    let repository_path = Path::new(&repository.canonical_path);
    let mut args = match git_action {
        "add" => vec!["add".to_string(), "--".to_string()],
        "restore" => {
            let has_head = git
                .run_in(Some(repository_path), &["rev-parse", "--verify", "HEAD"])
                .await?
                .success();
            if has_head {
                vec![
                    "restore".to_string(),
                    "--staged".to_string(),
                    "--".to_string(),
                ]
            } else {
                // Unstage an unborn branch without touching the worktree.
                vec![
                    "rm".to_string(),
                    "--cached".to_string(),
                    "--ignore-unmatch".to_string(),
                    "--".to_string(),
                ]
            }
        }
        _ => {
            return Err(ShehataError::Internal(
                "unsupported safe action".to_string(),
            ))
        }
    };
    args.extend(paths.iter().cloned());
    let refs = args.iter().map(String::as_str).collect::<Vec<_>>();
    let started = Instant::now();
    let result = git.run_checked(Some(repository_path), &refs).await;
    match result {
        Ok(_) => {
            write_audit(
                db_path,
                &repository.id,
                action,
                if action == "stage" {
                    "Staged selected paths"
                } else {
                    "Unstaged selected paths"
                },
                "success",
                Some(0),
                started,
            )?;
            Ok(GitActionResult {
                repository_id: repository.id,
                action: action.to_string(),
                changed_paths: paths.len(),
                commit: None,
            })
        }
        Err(error) => {
            write_audit(
                db_path,
                &repository.id,
                action,
                "Selected-path Git action failed",
                "failure",
                git_error_code(&error),
                started,
            )?;
            Err(error.into())
        }
    }
}

fn load_repository(db_path: &Path, repository_id: &str) -> Result<RepositoryRecord> {
    let repository_id = repository_id.trim();
    uuid::Uuid::parse_str(repository_id)
        .map_err(|_| ShehataError::InvalidInput("invalid repository id".to_string()))?;
    let db = Database::open_at(db_path)?;
    queries::find_repository_by_id(&db, repository_id)?
        .ok_or_else(|| ShehataError::RepositoryNotFound(repository_id.to_string()))
}

fn require_assignment(repository: &RepositoryRecord) -> Result<()> {
    if repository.assigned_account_id.is_none() {
        return Err(ShehataError::RepositoryNotLinked(repository.id.clone()));
    }
    Ok(())
}

fn validate_paths(paths: Vec<String>) -> Result<Vec<String>> {
    if paths.is_empty() || paths.len() > 500 {
        return Err(ShehataError::InvalidInput(
            "select between 1 and 500 paths".to_string(),
        ));
    }
    let mut clean = Vec::with_capacity(paths.len());
    for value in paths {
        if value.is_empty()
            || value.len() > 4096
            || value.contains(['\0', '\r', '\n'])
            || Path::new(&value).is_absolute()
        {
            return Err(ShehataError::InvalidInput(
                "invalid repository path".to_string(),
            ));
        }
        let mut meaningful = false;
        for component in Path::new(&value).components() {
            match component {
                Component::Normal(part) => {
                    meaningful = true;
                    if part.to_string_lossy().eq_ignore_ascii_case(".git") {
                        return Err(ShehataError::InvalidInput(
                            "Git metadata paths cannot be selected".to_string(),
                        ));
                    }
                }
                Component::CurDir => {}
                _ => {
                    return Err(ShehataError::InvalidInput(
                        "paths must stay inside the repository".to_string(),
                    ))
                }
            }
        }
        if !meaningful {
            return Err(ShehataError::InvalidInput(
                "invalid repository path".to_string(),
            ));
        }
        clean.push(value);
    }
    Ok(clean)
}

/// File names whose contents are credentials by convention.
const SENSITIVE_FILE_NAMES: &[&str] = &[
    ".env",
    ".npmrc",
    ".pypirc",
    ".netrc",
    "_netrc",
    "credentials",
    "credentials.ini",
    "credentials.json",
    "service-account.json",
    "id_rsa",
    "id_dsa",
    "id_ecdsa",
    "id_ed25519",
    "kubeconfig",
    "terraform.tfstate",
    "terraform.tfstate.backup",
];

/// Extensions that carry private keys or key stores.
const SENSITIVE_FILE_SUFFIXES: &[&str] = &[
    ".pem",
    ".key",
    ".p12",
    ".pfx",
    ".jks",
    ".keystore",
    ".ppk",
    ".asc",
    ".kdbx",
];

/// Whether a path is one whose contents must never be previewed.
///
/// Diff preview is the one place where file *contents* would reach the UI, the
/// activity trail, or a coding agent, so the check errs toward hiding.
fn is_sensitive_diff_path(path: &str) -> bool {
    let normalized = path.replace('\\', "/").to_ascii_lowercase();
    let name = normalized.rsplit('/').next().unwrap_or(&normalized);

    if SENSITIVE_FILE_NAMES.contains(&name) {
        return true;
    }
    if SENSITIVE_FILE_SUFFIXES
        .iter()
        .any(|suffix| name.ends_with(suffix))
    {
        return true;
    }
    // `.env.production`, `.env.local`, and friends.
    if name.starts_with(".env.") {
        return true;
    }
    // `secrets.yml`, `prod-secrets.json`, `db-password.txt`.
    if name.contains("secret") || name.contains("password") {
        return true;
    }
    // Anything living in a directory that exists to hold keys.
    normalized
        .split('/')
        .any(|segment| matches!(segment, ".ssh" | ".gnupg" | ".aws" | ".kube"))
}

/// Whether diff *content* looks like it carries a credential.
///
/// A file can have an innocent name and still contain a key. Content that
/// trips this check is withheld rather than redacted, because a partial
/// preview of a secret is still a leak.
fn diff_content_is_sensitive(content: &str) -> bool {
    const TOKEN_MARKERS: &[&str] = &[
        "ghp_",
        "gho_",
        "ghu_",
        "ghs_",
        "ghr_",
        "github_pat_",
        "aws_secret_access_key",
        "-----begin",
    ];
    let lower = content.to_ascii_lowercase();

    if lower.contains("private key-----") {
        return true;
    }
    if lower.contains("authorization:") && (lower.contains("bearer ") || lower.contains("basic ")) {
        return true;
    }
    // Only lines the diff actually adds or removes matter.
    lower
        .lines()
        .filter(|line| line.starts_with('+') || line.starts_with('-'))
        .any(|line| TOKEN_MARKERS.iter().any(|marker| line.contains(marker)))
}

fn truncate_utf8(value: String, max_bytes: usize) -> (String, bool) {
    if value.len() <= max_bytes {
        return (value, false);
    }
    let boundary = value
        .char_indices()
        .map(|(index, _)| index)
        .take_while(|index| *index <= max_bytes)
        .last()
        .unwrap_or(0);
    (value[..boundary].to_string(), true)
}

fn validate_commit_message(message: &str) -> Result<String> {
    let message = message.trim();
    if message.is_empty()
        || message.len() > 1_000
        || message.contains('\0')
        || message.chars().any(|character| character == '\r')
    {
        return Err(ShehataError::InvalidInput(
            "commit message must be 1-1000 safe characters".to_string(),
        ));
    }
    Ok(message.to_string())
}

fn parse_porcelain_v1_z(raw: &str) -> Vec<ChangeEntry> {
    let mut records = raw.split_terminator('\0');
    let mut changes = Vec::new();
    while let Some(record) = records.next() {
        if record.len() < 4 {
            continue;
        }
        let mut status = record.chars();
        let index_status = status.next().unwrap_or(' ').to_string();
        let worktree_status = status.next().unwrap_or(' ').to_string();
        let path = record[3..].to_string();
        let renamed = matches!(index_status.as_str(), "R" | "C")
            || matches!(worktree_status.as_str(), "R" | "C");
        if renamed {
            let _old_path = records.next();
        }
        changes.push(ChangeEntry {
            path,
            index_status,
            worktree_status,
        });
    }
    changes
}

fn git_error_code(error: &GitError) -> Option<i64> {
    match error {
        GitError::Exit { code, .. } => Some((*code).into()),
        _ => None,
    }
}

fn write_audit(
    db_path: &Path,
    repository_id: &str,
    event_type: &str,
    summary: &str,
    result: &str,
    exit_code: Option<i64>,
    started: Instant,
) -> Result<()> {
    let db = Database::open_at(db_path)?;
    queries::insert_audit_event(
        &db,
        &NewAuditEvent {
            event_type,
            repository_id: Some(repository_id),
            account_login: None,
            summary,
            detail: None,
            result,
            exit_code,
            duration_ms: Some(started.elapsed().as_millis().min(i64::MAX as u128) as i64),
        },
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn write_network_audit(
    db_path: &Path,
    repository_id: &str,
    account_login: Option<&str>,
    event_type: &str,
    summary: &str,
    detail: Option<&str>,
    result: &str,
    exit_code: Option<i64>,
    started: Instant,
) -> Result<()> {
    let db = Database::open_at(db_path)?;
    queries::insert_audit_event(
        &db,
        &NewAuditEvent {
            event_type,
            repository_id: Some(repository_id),
            account_login,
            summary,
            detail,
            result,
            exit_code,
            duration_ms: Some(started.elapsed().as_millis().min(i64::MAX as u128) as i64),
        },
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;

    use chrono::Utc;
    use tempfile::TempDir;
    use uuid::Uuid;

    use super::*;

    fn git(repo: &Path, args: &[&str]) {
        assert!(Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .status()
            .unwrap()
            .success());
    }

    fn git_output(repo: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }

    fn fixture() -> (TempDir, PathBuf, PathBuf, String) {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        fs::create_dir(&repo).unwrap();
        git(&repo, &["init"]);
        git(&repo, &["config", "user.name", "Test User"]);
        git(&repo, &["config", "user.email", "test@example.com"]);
        let id = Uuid::new_v4().to_string();
        let db_path = temp.path().join("db.sqlite");
        let db = Database::open_at(&db_path).unwrap();
        let account = queries::upsert_account(&db, "github.com", "alice", "valid").unwrap();
        let now = Utc::now().to_rfc3339();
        queries::insert_repository(
            &db,
            &RepositoryRecord {
                id: id.clone(),
                canonical_path: fs::canonicalize(&repo)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                git_dir: Some(repo.join(".git").to_string_lossy().into_owned()),
                git_common_dir: None,
                display_name: "repo".into(),
                host: Some("github.com".into()),
                owner: Some("acme".into()),
                repo_name: Some("repo".into()),
                remote_name: Some("origin".into()),
                remote_url: Some("https://github.com/acme/repo.git".into()),
                current_branch: Some("main".into()),
                assigned_account_id: Some(account),
                commit_name: Some("Test User".into()),
                commit_email: Some("test@example.com".into()),
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
    async fn stages_commits_and_reports_status() {
        let (_temp, repo, db_path, id) = fixture();
        fs::write(repo.join("hello.txt"), "hello").unwrap();
        let before = status_at(&db_path, &id).await.unwrap();
        assert_eq!(before.changes[0].worktree_status, "?");
        let summary = summarize_status(before.clone());
        assert_eq!(summary.untracked_paths, 1);
        stage_at(
            &db_path,
            PathsRequest {
                repository_id: id.clone(),
                paths: vec!["hello.txt".into()],
            },
        )
        .await
        .unwrap();
        unstage_at(
            &db_path,
            PathsRequest {
                repository_id: id.clone(),
                paths: vec!["hello.txt".into()],
            },
        )
        .await
        .unwrap();
        assert_eq!(
            status_at(&db_path, &id).await.unwrap().changes[0].index_status,
            "?"
        );
        stage_at(
            &db_path,
            PathsRequest {
                repository_id: id.clone(),
                paths: vec!["hello.txt".into()],
            },
        )
        .await
        .unwrap();
        let result = commit_at(
            &db_path,
            CommitRequest {
                repository_id: id.clone(),
                message: "feat: add hello".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(result.commit.as_deref().map(str::len), Some(40));
        assert!(status_at(&db_path, &id).await.unwrap().changes.is_empty());
    }

    #[test]
    fn rejects_escaping_and_git_metadata_paths() {
        assert!(validate_paths(vec!["../secret".into()]).is_err());
        assert!(validate_paths(vec![".git/config".into()]).is_err());
        assert!(validate_paths(vec!["-danger".into()]).is_ok());
    }

    #[test]
    fn blocks_sensitive_diff_filenames_and_truncates_on_utf8_boundaries() {
        assert!(is_sensitive_diff_path(".env"));
        for path in [
            ".env.production",
            "app/.npmrc",
            "deploy/id_ed25519",
            "certs/server.p12",
            "infra/terraform.tfstate",
            "home/.ssh/config",
            "config/secrets.yml",
            "db-password.txt",
            "k8s/kubeconfig",
        ] {
            assert!(is_sensitive_diff_path(path), "{path} should be hidden");
        }
        for path in ["src/environment.ts", "docs/README.md", "src/keyboard.rs"] {
            assert!(!is_sensitive_diff_path(path), "{path} should be visible");
        }
        assert!(is_sensitive_diff_path("config/production.pem"));
        assert!(!is_sensitive_diff_path("src/environment.ts"));
        let (value, truncated) = truncate_utf8("aéz".to_string(), 2);
        assert_eq!(value, "a");
        assert!(truncated);
    }

    #[tokio::test]
    async fn returns_tracked_untracked_and_sensitive_file_diffs() {
        let (_temp, repo, db_path, id) = fixture();
        fs::write(repo.join("tracked.txt"), "before\n").unwrap();
        git(&repo, &["add", "--", "tracked.txt"]);
        git(&repo, &["commit", "-m", "feat: baseline"]);
        fs::write(repo.join("tracked.txt"), "after\n").unwrap();
        fs::write(repo.join("new.txt"), "new line\n").unwrap();
        fs::write(repo.join(".env"), "TOKEN=secret\n").unwrap();

        let tracked = file_diff_at(
            &db_path,
            FileDiffRequest {
                repository_id: id.clone(),
                path: "tracked.txt".into(),
                staged: false,
            },
        )
        .await
        .unwrap();
        assert!(tracked.content.contains("-before"));
        assert!(tracked.content.contains("+after"));

        let untracked = file_diff_at(
            &db_path,
            FileDiffRequest {
                repository_id: id.clone(),
                path: "new.txt".into(),
                staged: false,
            },
        )
        .await
        .unwrap();
        assert!(untracked.content.contains("+new line"));

        let sensitive = file_diff_at(
            &db_path,
            FileDiffRequest {
                repository_id: id,
                path: ".env".into(),
                staged: false,
            },
        )
        .await
        .unwrap();
        assert!(sensitive.sensitive);
        assert!(!sensitive.content.contains("secret"));
    }

    #[test]
    fn enforces_push_policies_by_caller_and_approval() {
        assert!(enforce_push_policy("allow_normal_push", ActionCaller::Mcp, false).is_ok());
        // A caller cannot approve its own way past a block.
        assert!(enforce_push_policy("block_ai_push", ActionCaller::Mcp, true).is_err());
        // The retired policy keeps refusing agents after the upgrade.
        assert!(enforce_push_policy("ask_before_push", ActionCaller::Mcp, true).is_err());
        assert!(enforce_push_policy("ask_before_push", ActionCaller::Cli, false).is_ok());
        // A human at their own keyboard is never gated by the policy.
        assert!(enforce_push_policy("ask_before_push", ActionCaller::Desktop, false).is_ok());
        assert!(matches!(
            enforce_push_policy("block_ai_push", ActionCaller::Mcp, true),
            Err(ShehataError::OperationBlocked(_))
        ));
        assert!(enforce_push_policy("block_ai_push", ActionCaller::Cli, false).is_ok());
    }

    #[test]
    fn persists_only_supported_push_policies() {
        let (_temp, _repo, db_path, id) = fixture();
        let result = set_push_policy_at(
            &db_path,
            SetPushPolicyRequest {
                repository_id: id.clone(),
                push_policy: "ask_before_push".into(),
            },
        )
        .unwrap();
        // The retired name is accepted and stored under the name that
        // describes what it actually does.
        assert_eq!(result.push_policy, "block_ai_push");
        let db = Database::open_at(&db_path).unwrap();
        assert_eq!(
            queries::find_repository_by_id(&db, &id)
                .unwrap()
                .unwrap()
                .push_policy,
            "block_ai_push"
        );
        assert!(set_push_policy_at(
            &db_path,
            SetPushPolicyRequest {
                repository_id: id,
                push_policy: "force_push".into(),
            },
        )
        .is_err());
    }

    #[test]
    fn network_lines_title_the_change_and_detail_the_context() {
        let (title, detail) = network_action_lines(
            "push",
            "Shehata Git",
            "main",
            "0545b97a1c2d3e4f",
            Some("docs: mark shipped roadmap items"),
            Some(ActionCaller::Desktop),
        );
        assert_eq!(title, "docs: mark shipped roadmap items");
        // The force-push guarantee has to stay visible in the trail.
        assert_eq!(
            detail,
            "Normal push from the app · Shehata Git · main · 0545b97"
        );

        // A repository with no readable subject still names the action.
        let (title, detail) =
            network_action_lines("pull_ff_only", "site", "master", "abcdef1234", None, None);
        assert_eq!(title, "Fast-forward pull completed");
        assert_eq!(detail, "Fast-forward pull · site · master · abcdef1");
    }

    #[test]
    fn the_trail_says_when_a_coding_agent_pushed() {
        // The whole point of this tool is knowing which identity acted. A push
        // an agent made must never be indistinguishable from one you made.
        let (_, agent) = network_action_lines(
            "push",
            "Landing Page",
            "main",
            "aaaaaaa1111",
            Some("feat: add pricing"),
            Some(ActionCaller::Mcp),
        );
        let (_, human) = network_action_lines(
            "push",
            "Landing Page",
            "main",
            "aaaaaaa1111",
            Some("feat: add pricing"),
            Some(ActionCaller::Desktop),
        );
        assert!(
            agent.starts_with("Normal push by a coding agent"),
            "{agent}"
        );
        assert_ne!(agent, human);
    }

    #[test]
    fn diff_content_with_a_credential_is_withheld() {
        let key_block = ["-----BEGIN OPENSSH ", "PRIVATE KEY-----"].concat();
        assert!(diff_content_is_sensitive(&key_block));
        assert!(diff_content_is_sensitive(
            "+Authorization: Bearer abcdef123456"
        ));
        assert!(diff_content_is_sensitive(
            &["+token = ghp_", "aaaabbbbccccdddd"].concat()
        ));
    }

    #[test]
    fn ordinary_diff_content_stays_visible() {
        let diff = "@@ -1,3 +1,4 @@
 fn main() {
+    println!(\"hello\");
 }";
        assert!(!diff_content_is_sensitive(diff));
    }

    #[test]
    fn a_mention_in_unchanged_context_does_not_hide_the_diff() {
        // Only added and removed lines are evidence; a surrounding context
        // line that merely names a variable must not blank the preview.
        let diff = " let ghp_prefix = \"documented in the readme\";";
        assert!(!diff_content_is_sensitive(diff));
    }

    #[test]
    fn parses_ahead_and_behind_strictly() {
        assert_eq!(parse_ahead_behind("2\t3\n").unwrap(), (2, 3));
        assert!(parse_ahead_behind("2").is_err());
        assert!(parse_ahead_behind("2 3 extra").is_err());
    }

    #[tokio::test]
    async fn fixed_network_commands_pull_ff_only_and_push_normally() {
        let temp = tempfile::tempdir().unwrap();
        let remote = temp.path().join("remote.git");
        assert!(Command::new("git")
            .args(["init", "--bare"])
            .arg(&remote)
            .status()
            .unwrap()
            .success());

        let first = temp.path().join("first");
        fs::create_dir(&first).unwrap();
        git(&first, &["init", "--initial-branch=main"]);
        git(&first, &["config", "user.name", "First User"]);
        git(&first, &["config", "user.email", "first@example.com"]);
        fs::write(first.join("one.txt"), "one").unwrap();
        git(&first, &["add", "--", "one.txt"]);
        git(&first, &["commit", "-m", "feat: first"]);
        git(
            &first,
            &["remote", "add", "origin", &remote.to_string_lossy()],
        );
        git(&first, &["push", "-u", "origin", "main"]);

        let second = temp.path().join("second");
        assert!(Command::new("git")
            .args(["clone", "--branch", "main"])
            .arg(&remote)
            .arg(&second)
            .status()
            .unwrap()
            .success());
        git(&second, &["config", "user.name", "Second User"]);
        git(&second, &["config", "user.email", "second@example.com"]);
        fs::write(second.join("two.txt"), "two").unwrap();
        git(&second, &["add", "--", "two.txt"]);
        git(&second, &["commit", "-m", "feat: second"]);
        git(&second, &["push", "origin", "main"]);

        let runner = GitRunner::locate().unwrap();
        execute_pull(&runner, &first, "origin", "main")
            .await
            .unwrap();
        assert!(first.join("two.txt").is_file());

        fs::write(first.join("three.txt"), "three").unwrap();
        git(&first, &["add", "--", "three.txt"]);
        git(&first, &["commit", "-m", "feat: third"]);
        execute_push(&runner, &first, "origin", "main", false)
            .await
            .unwrap();
        assert_eq!(
            git_output(&first, &["rev-parse", "HEAD"]),
            git_output(&remote, &["rev-parse", "refs/heads/main"])
        );
    }

    #[tokio::test]
    async fn first_push_publishes_branch_and_sets_upstream() {
        let temp = tempfile::tempdir().unwrap();
        let remote = temp.path().join("remote.git");
        assert!(Command::new("git")
            .args(["init", "--bare"])
            .arg(&remote)
            .status()
            .unwrap()
            .success());

        let local = temp.path().join("local");
        fs::create_dir(&local).unwrap();
        git(&local, &["init", "--initial-branch=main"]);
        git(&local, &["config", "user.name", "First User"]);
        git(&local, &["config", "user.email", "first@example.com"]);
        fs::write(local.join("one.txt"), "one").unwrap();
        git(&local, &["add", "--", "one.txt"]);
        git(&local, &["commit", "-m", "feat: first"]);
        git(
            &local,
            &["remote", "add", "origin", &remote.to_string_lossy()],
        );

        let runner = GitRunner::locate().unwrap();
        execute_push(&runner, &local, "origin", "main", true)
            .await
            .unwrap();
        assert_eq!(
            git_output(&local, &["rev-parse", "HEAD"]),
            git_output(&remote, &["rev-parse", "refs/heads/main"])
        );
        assert_eq!(
            git_output(
                &local,
                &[
                    "rev-parse",
                    "--abbrev-ref",
                    "--symbolic-full-name",
                    "@{upstream}"
                ]
            ),
            "origin/main"
        );
    }
}
