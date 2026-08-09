//! Read-only repository discovery built on the system Git executable.
//!
//! Discovery never changes repository configuration. Paths are canonicalized
//! before Git is invoked, and every command uses [`GitRunner`] argument arrays.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::remote::{parse_remote_url, RemoteProtocol};
use crate::{GitError, GitRunner};

#[derive(Debug, Error)]
pub enum RepositoryDiscoveryError {
    #[error("repository folder does not exist or cannot be read: {0}")]
    InvalidPath(String),
    #[error("selected folder is not a Git worktree")]
    NotWorktree,
    #[error(transparent)]
    Git(#[from] GitError),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepositoryRemoteProtocol {
    Https,
    Ssh,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepositoryRemote {
    pub name: String,
    pub url: String,
    pub host: Option<String>,
    pub owner: Option<String>,
    pub repo_name: Option<String>,
    pub protocol: RepositoryRemoteProtocol,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeStatus {
    pub changed_files: usize,
    pub conflicts: usize,
    pub untracked_files: usize,
    pub ahead: usize,
    pub behind: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveredRepository {
    pub canonical_path: PathBuf,
    pub git_dir: PathBuf,
    pub git_common_dir: PathBuf,
    pub display_name: String,
    pub current_branch: Option<String>,
    pub detached_head: bool,
    pub head_commit: Option<String>,
    pub upstream: Option<String>,
    pub remotes: Vec<RepositoryRemote>,
    pub primary_remote_name: Option<String>,
    pub status: WorktreeStatus,
    /// The identity this repository sets for itself, if it sets one.
    pub commit_name: Option<String>,
    pub commit_email: Option<String>,
    /// The identity commits would otherwise be authored with, inherited from
    /// wider Git configuration. Only present when the repository sets none of
    /// its own, because that is the case where it is about to apply silently.
    pub inherited_commit_name: Option<String>,
    pub inherited_commit_email: Option<String>,
    pub credential_helpers: Vec<String>,
    pub credential_use_http_path: Option<bool>,
}

pub async fn discover_repository(
    git: &GitRunner,
    selected_path: &Path,
) -> Result<DiscoveredRepository, RepositoryDiscoveryError> {
    let selected = std::fs::canonicalize(selected_path)
        .map_err(|_| RepositoryDiscoveryError::InvalidPath(selected_path.display().to_string()))?;
    if !selected.is_dir() {
        return Err(RepositoryDiscoveryError::InvalidPath(
            selected.display().to_string(),
        ));
    }

    let is_worktree = git
        .run_in(Some(&selected), &["rev-parse", "--is-inside-work-tree"])
        .await?;
    if !is_worktree.success() || is_worktree.stdout.trim() != "true" {
        return Err(RepositoryDiscoveryError::NotWorktree);
    }

    let canonical_path = canonical_git_path(
        git.run_checked(Some(&selected), &["rev-parse", "--show-toplevel"])
            .await?
            .stdout
            .trim(),
    )?;
    let git_dir = canonical_git_path(
        git.run_checked(
            Some(&canonical_path),
            &["rev-parse", "--path-format=absolute", "--git-dir"],
        )
        .await?
        .stdout
        .trim(),
    )?;
    let git_common_dir = canonical_git_path(
        git.run_checked(
            Some(&canonical_path),
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )
        .await?
        .stdout
        .trim(),
    )?;

    let branch_output = git
        .run_in(
            Some(&canonical_path),
            &["symbolic-ref", "--quiet", "--short", "HEAD"],
        )
        .await?;
    let current_branch = successful_value(branch_output.success(), &branch_output.stdout);
    let detached_head = current_branch.is_none()
        && git
            .run_in(Some(&canonical_path), &["rev-parse", "--verify", "HEAD"])
            .await?
            .success();

    let head_output = git
        .run_in(Some(&canonical_path), &["rev-parse", "--verify", "HEAD"])
        .await?;
    let head_commit = successful_value(head_output.success(), &head_output.stdout);

    let upstream_output = git
        .run_in(
            Some(&canonical_path),
            &[
                "rev-parse",
                "--abbrev-ref",
                "--symbolic-full-name",
                "@{upstream}",
            ],
        )
        .await?;
    let upstream = successful_value(upstream_output.success(), &upstream_output.stdout);

    let remote_names_output = git.run_checked(Some(&canonical_path), &["remote"]).await?;
    let mut remotes = Vec::new();
    for name in remote_names_output.stdout.lines().map(str::trim) {
        if name.is_empty() {
            continue;
        }
        let url_output = git
            .run_in(Some(&canonical_path), &["remote", "get-url", name])
            .await?;
        if let Some(url) = successful_value(url_output.success(), &url_output.stdout) {
            remotes.push(remote_details(name, &url));
        }
    }
    let primary_remote_name = choose_primary_remote(&remotes, upstream.as_deref());

    let status_output = git
        .run_checked(
            Some(&canonical_path),
            &["status", "--porcelain=v2", "--branch", "-z"],
        )
        .await?;
    let status = parse_porcelain_v2(&status_output.stdout);

    // Kept apart on purpose. What a repository sets for itself was chosen for
    // it; what it inherits is whatever happened to be configured globally and
    // may belong to an unrelated account. Presenting the second as though it
    // were the first is how a repository ends up committing as someone else.
    let commit_name = read_local_config(git, &canonical_path, "user.name").await?;
    let commit_email = read_local_config(git, &canonical_path, "user.email").await?;
    let inherited_commit_name = match commit_name {
        Some(_) => None,
        None => read_inherited_config(git, &canonical_path, "user.name").await?,
    };
    let inherited_commit_email = match commit_email {
        Some(_) => None,
        None => read_inherited_config(git, &canonical_path, "user.email").await?,
    };
    let credential_helpers =
        read_local_config_values(git, &canonical_path, "credential.helper").await?;
    let credential_use_http_path =
        read_local_config(git, &canonical_path, "credential.useHttpPath")
            .await?
            .and_then(|value| match value.to_ascii_lowercase().as_str() {
                "true" | "yes" | "on" | "1" => Some(true),
                "false" | "no" | "off" | "0" => Some(false),
                _ => None,
            });

    let display_name = canonical_path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("Repository")
        .to_string();

    Ok(DiscoveredRepository {
        canonical_path,
        git_dir,
        git_common_dir,
        display_name,
        current_branch,
        detached_head,
        head_commit,
        upstream,
        remotes,
        primary_remote_name,
        status,
        commit_name,
        commit_email,
        inherited_commit_name,
        inherited_commit_email,
        credential_helpers,
        credential_use_http_path,
    })
}

fn canonical_git_path(raw: &str) -> Result<PathBuf, RepositoryDiscoveryError> {
    let path = PathBuf::from(raw);
    std::fs::canonicalize(&path)
        .map_err(|_| RepositoryDiscoveryError::InvalidPath(path.display().to_string()))
}

fn successful_value(success: bool, stdout: &str) -> Option<String> {
    success
        .then(|| stdout.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn remote_details(name: &str, url: &str) -> RepositoryRemote {
    match parse_remote_url(url) {
        Ok(parsed) => RepositoryRemote {
            name: name.to_string(),
            url: url.to_string(),
            host: Some(parsed.host),
            owner: Some(parsed.owner),
            repo_name: Some(parsed.repo),
            protocol: match parsed.protocol {
                RemoteProtocol::Https => RepositoryRemoteProtocol::Https,
                RemoteProtocol::Ssh => RepositoryRemoteProtocol::Ssh,
            },
        },
        Err(_) => RepositoryRemote {
            name: name.to_string(),
            url: url.to_string(),
            host: None,
            owner: None,
            repo_name: None,
            protocol: RepositoryRemoteProtocol::Other,
        },
    }
}

fn choose_primary_remote(remotes: &[RepositoryRemote], upstream: Option<&str>) -> Option<String> {
    let upstream_remote = upstream.and_then(|value| value.split_once('/').map(|(name, _)| name));
    upstream_remote
        .and_then(|name| remotes.iter().find(|remote| remote.name == name))
        .or_else(|| remotes.iter().find(|remote| remote.name == "origin"))
        .or_else(|| remotes.first())
        .map(|remote| remote.name.clone())
}

fn parse_porcelain_v2(output: &str) -> WorktreeStatus {
    let mut status = WorktreeStatus {
        changed_files: 0,
        conflicts: 0,
        untracked_files: 0,
        ahead: 0,
        behind: 0,
    };

    for record in output.split('\0').flat_map(str::lines) {
        if let Some(ab) = record.strip_prefix("# branch.ab ") {
            for part in ab.split_whitespace() {
                if let Some(value) = part.strip_prefix('+') {
                    status.ahead = value.parse().unwrap_or(0);
                } else if let Some(value) = part.strip_prefix('-') {
                    status.behind = value.parse().unwrap_or(0);
                }
            }
        } else if record.starts_with("u ") {
            status.changed_files += 1;
            status.conflicts += 1;
        } else if record.starts_with("? ") {
            status.untracked_files += 1;
        } else if record.starts_with("1 ") || record.starts_with("2 ") {
            status.changed_files += 1;
        }
    }
    status
}

async fn read_local_config(
    git: &GitRunner,
    repo: &Path,
    key: &str,
) -> Result<Option<String>, GitError> {
    let output = git
        .run_in(Some(repo), &["config", "--local", "--get", key])
        .await?;
    Ok(successful_value(output.success(), &output.stdout))
}

/// What git would fall back to for a repository that sets nothing itself.
///
/// Reported so the caller can say what will happen by default, never so it can
/// be presented as the repository's own choice.
async fn read_inherited_config(
    git: &GitRunner,
    repo: &Path,
    key: &str,
) -> Result<Option<String>, GitError> {
    let output = git.run_in(Some(repo), &["config", "--get", key]).await?;
    Ok(successful_value(output.success(), &output.stdout))
}

pub async fn read_local_config_values(
    git: &GitRunner,
    repo: &Path,
    key: &str,
) -> Result<Vec<String>, GitError> {
    let output = git
        .run_in(Some(repo), &["config", "--local", "--get-all", key])
        .await?;
    if !output.success() {
        return Ok(Vec::new());
    }
    // Empty `credential.helper` values are meaningful: they reset helpers
    // inherited from broader Git config scopes. Preserve them exactly.
    Ok(output
        .stdout
        .split_terminator('\n')
        .map(|line| line.trim_end_matches('\r').to_string())
        .collect())
}

/// Replace every local value for a known-safe Git configuration key.
/// Callers must validate values before invoking this function.
pub async fn replace_local_config_values(
    git: &GitRunner,
    repo: &Path,
    key: &str,
    values: &[String],
) -> Result<(), GitError> {
    let unset = git
        .run_in(Some(repo), &["config", "--local", "--unset-all", key])
        .await?;
    // Git exits nonzero when the key did not exist. That is the desired state.
    if !unset.success() && !unset.stderr.trim().is_empty() {
        return Err(GitError::Exit {
            code: unset.code,
            message: unset.stderr.trim().to_string(),
        });
    }

    for value in values {
        git.run_checked(Some(repo), &["config", "--local", "--add", key, value])
            .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_status_counts_and_ahead_behind() {
        let raw = concat!(
            "# branch.oid abc\0",
            "# branch.head main\0",
            "# branch.ab +2 -1\0",
            "1 .M N... file.txt\0",
            "u UU N... conflict.txt\0",
            "? new.txt\0"
        );
        let status = parse_porcelain_v2(raw);
        assert_eq!(status.changed_files, 2);
        assert_eq!(status.conflicts, 1);
        assert_eq!(status.untracked_files, 1);
        assert_eq!(status.ahead, 2);
        assert_eq!(status.behind, 1);
    }

    #[test]
    fn chooses_upstream_then_origin_then_first() {
        let remotes = vec![
            remote_details("backup", "https://github.com/acme/backup.git"),
            remote_details("origin", "https://github.com/acme/main.git"),
        ];
        assert_eq!(
            choose_primary_remote(&remotes, Some("backup/main")).as_deref(),
            Some("backup")
        );
        assert_eq!(
            choose_primary_remote(&remotes, None).as_deref(),
            Some("origin")
        );
    }

    #[tokio::test]
    async fn discovers_real_temporary_worktree() {
        let dir = tempfile::tempdir().unwrap();
        let git = GitRunner::locate().unwrap();
        git.run_checked(Some(dir.path()), &["init", "-b", "main"])
            .await
            .unwrap();
        git.run_checked(
            Some(dir.path()),
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/acme/example.git",
            ],
        )
        .await
        .unwrap();
        git.run_checked(
            Some(dir.path()),
            &["config", "--local", "user.name", "Test User"],
        )
        .await
        .unwrap();
        std::fs::write(dir.path().join("new.txt"), "hello").unwrap();

        let repo = discover_repository(&git, dir.path()).await.unwrap();
        assert_eq!(repo.current_branch.as_deref(), Some("main"));
        assert_eq!(repo.primary_remote_name.as_deref(), Some("origin"));
        assert_eq!(repo.remotes[0].host.as_deref(), Some("github.com"));
        assert_eq!(repo.commit_name.as_deref(), Some("Test User"));
        assert_eq!(repo.status.untracked_files, 1);
        // The repository set this itself, so there is nothing pending.
        assert_eq!(repo.inherited_commit_name, None);
    }

    #[tokio::test]
    async fn an_inherited_identity_is_never_reported_as_the_repository_own() {
        // A repository that sets no identity inherits one that may belong to
        // an unrelated account. Reporting it in `commit_name` would let the
        // connect form present someone else's identity as this repository's
        // choice, and one confirmation would then write it in — which is the
        // exact mistake this application exists to prevent.
        let dir = tempfile::tempdir().unwrap();
        let git = GitRunner::locate().unwrap();
        git.run_checked(Some(dir.path()), &["init", "-b", "main"])
            .await
            .unwrap();

        let repo = discover_repository(&git, dir.path()).await.unwrap();
        assert_eq!(
            repo.commit_name, None,
            "a repository that sets no identity must not claim one"
        );
        assert_eq!(repo.commit_email, None);
        // Whatever it would fall back to is reported separately, so a caller
        // can say what will happen instead of hiding it.
        assert_eq!(
            repo.inherited_commit_name.is_some(),
            git.run_in(Some(dir.path()), &["config", "--get", "user.name"])
                .await
                .unwrap()
                .success(),
        );
    }

    #[tokio::test]
    async fn rejects_non_repository_folder() {
        let dir = tempfile::tempdir().unwrap();
        let git = GitRunner::locate().unwrap();
        let error = discover_repository(&git, dir.path()).await.unwrap_err();
        assert!(matches!(error, RepositoryDiscoveryError::NotWorktree));
    }
}
