// Copyright (c) 2026 Dr Mohamed Shehata. All rights reserved.
// Licensed under the MIT License. See LICENSE in the project root.

//! shehata-mcp — stdio MCP server.
//!
//! Exposes safe, structured Shehata Git tools to AI coding agents.
//!
//! Hard rules enforced here:
//! - No arbitrary shell execution tool. Ever.
//! - No force push, remote deletion, reset --hard, or clean.
//! - No tokens in any tool result — credentials never cross this boundary.
//! - Every result is a structured envelope: { ok, code, summary, data }.
//!   `code` is stable and machine-readable; `summary` is for humans.

use rmcp::{
    handler::server::wrapper::Parameters, schemars, tool, tool_router, transport::stdio,
    ErrorData as McpError, Json, ServiceExt,
};
use serde::{Deserialize, Serialize};
use shehata_core::{
    accounts as core_accounts, actions as core_actions, redact, repositories as core_repositories,
    routing as core_routing, Doctor, ShehataError,
};
use shehata_github::GhRunner;
use shehata_storage::Database;

// ------------------------------------------------------------------ envelope

/// Uniform tool result envelope. `code` values match ShehataError::code().
#[derive(Debug, Serialize, schemars::JsonSchema)]
struct Envelope {
    ok: bool,
    code: String,
    summary: String,
    data: Option<serde_json::Value>,
}

impl Envelope {
    fn success<T: Serialize>(summary: impl Into<String>, data: T) -> Result<Self, McpError> {
        let data = serde_json::to_value(data)
            .map_err(|_| McpError::internal_error("could not serialize tool result", None))?;
        Ok(Self {
            ok: true,
            code: "ok".to_string(),
            summary: summary.into(),
            data: Some(data),
        })
    }

    fn failure(error: &ShehataError) -> Self {
        Self::failure_with(error, None)
    }

    /// Every failure that leaves this server is redacted first. Coding agents
    /// forward tool output into their own context and logs, so a secret that
    /// escapes here escapes much further than one shown in the desktop app.
    fn failure_with(error: &ShehataError, data: Option<serde_json::Value>) -> Self {
        Self {
            ok: false,
            code: error.code().to_string(),
            summary: redact::redact_secrets(&error.to_string()),
            data,
        }
    }
}

/// What a coding agent is allowed to learn about a repository.
///
/// The desktop app may show absolute paths, remote URLs, and the local commit
/// email, but an MCP client forwards everything it receives into its own model
/// context and logs. This projection keeps the local filesystem layout (which
/// carries the Windows user name), the raw remote URL (which is where legacy
/// embedded credentials live), and the author email out of that flow.
#[derive(Debug, Serialize, schemars::JsonSchema)]
struct McpRepository {
    repository_id: String,
    display_name: String,
    host: Option<String>,
    owner: Option<String>,
    repo_name: Option<String>,
    remote_name: Option<String>,
    current_branch: Option<String>,
    assigned_login: Option<String>,
    push_policy: String,
    routing_configured: bool,
}

impl From<core_repositories::RepositorySummary> for McpRepository {
    fn from(value: core_repositories::RepositorySummary) -> Self {
        Self {
            repository_id: value.id,
            display_name: value.display_name,
            host: value.host,
            owner: value.owner,
            repo_name: value.repo_name,
            remote_name: value.remote_name,
            current_branch: value.current_branch,
            assigned_login: value.assigned_login,
            push_policy: value.push_policy,
            routing_configured: value.routing_configured,
        }
    }
}

// ---------------------------------------------------------------------- args

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct RepositoryArgs {
    /// The Shehata Git repository id (UUID). Provide this or `path`.
    #[serde(default)]
    repository_id: Option<String>,
    /// The repository's canonical path. Provide this or `repository_id`.
    #[serde(default)]
    path: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct CommitArgs {
    /// The Shehata Git repository id (UUID). Provide this or `path`.
    #[serde(default)]
    repository_id: Option<String>,
    /// The repository's canonical path. Provide this or `repository_id`.
    #[serde(default)]
    path: Option<String>,
    /// Normal commit message. Amend is never supported.
    message: String,
    /// Repository-relative paths to stage before committing.
    paths: Vec<String>,
}

fn repository_reference(args: &RepositoryArgs) -> Result<&str, McpError> {
    match (&args.repository_id, &args.path) {
        (Some(id), None) if !id.trim().is_empty() => Ok(id),
        (None, Some(path)) if !path.trim().is_empty() => Ok(path),
        _ => Err(McpError::invalid_params(
            "provide exactly one of repository_id or path",
            None,
        )),
    }
}

async fn resolve_repository_id(
    args: &RepositoryArgs,
) -> Result<std::result::Result<String, ShehataError>, McpError> {
    let reference = repository_reference(args)?;
    Ok(
        core_repositories::resolve_repository_reference(Some(reference))
            .await
            .map(|repository| repository.id),
    )
}

// --------------------------------------------------------------------- state

#[derive(Clone)]
struct ShehataMcp {
    db_path: Option<std::path::PathBuf>,
}

impl ShehataMcp {
    fn new() -> Self {
        Self {
            db_path: Database::default_path().ok(),
        }
    }

    fn open_db(&self) -> Result<Database, McpError> {
        let path = self.db_path.clone().ok_or_else(|| {
            McpError::internal_error("application data directory unavailable", None)
        })?;
        Database::open_at(&path).map_err(|e| McpError::internal_error(e.to_string(), None))
    }
}

// --------------------------------------------------------------------- tools

#[tool_router(server_handler)]
impl ShehataMcp {
    /// Run the full Shehata Git system check: git, GitHub CLI, accounts,
    /// database, credential helper, WebView, PATH, and this MCP server.
    #[tool(
        name = "shehata_git_doctor",
        description = "Check everything Shehata Git needs and how to fix what is missing"
    )]
    async fn doctor(&self) -> Result<Json<Envelope>, McpError> {
        let report = Doctor::new().run().await;
        let summary = if report.healthy {
            "system healthy".to_string()
        } else {
            let problems: Vec<&str> = report
                .checks
                .iter()
                .filter(|c| c.status != shehata_core::CheckStatus::Ready)
                .map(|c| c.label.as_str())
                .collect();
            format!("needs attention: {}", problems.join(", "))
        };
        Ok(Json(Envelope::success(summary, report)?))
    }

    /// List GitHub accounts authenticated in the GitHub CLI, with token
    /// availability per account. Never returns tokens.
    #[tool(
        name = "shehata_git_list_accounts",
        description = "List GitHub accounts available on this machine (no tokens, ever)"
    )]
    async fn list_accounts(&self) -> Result<Json<Envelope>, McpError> {
        let gh = match GhRunner::locate() {
            Ok(gh) => gh,
            Err(e) => {
                let err = ShehataError::Github(e);
                return Ok(Json(Envelope::failure_with(
                    &err,
                    Some(serde_json::json!([])),
                )));
            }
        };
        match core_accounts::list_accounts(&gh).await {
            Ok(accounts) => {
                if let Ok(db) = self.open_db() {
                    core_accounts::mirror_accounts(&db, &accounts);
                }
                let summary = format!("{} account(s) available", accounts.len());
                Ok(Json(Envelope::success(summary, accounts)?))
            }
            Err(e) => Ok(Json(Envelope::failure_with(
                &e,
                Some(serde_json::json!([])),
            ))),
        }
    }

    /// List repositories linked to Shehata Git.
    #[tool(
        name = "shehata_git_list_repositories",
        description = "List repositories linked to Shehata Git with their assigned accounts"
    )]
    async fn list_repositories(&self) -> Result<Json<Envelope>, McpError> {
        match core_repositories::list_repository_summaries_with_routing().await {
            Ok(repos) => {
                let summary = format!("{} linked repositorie(s)", repos.len());
                let repos: Vec<McpRepository> =
                    repos.into_iter().map(McpRepository::from).collect();
                Ok(Json(Envelope::success(summary, repos)?))
            }
            Err(error) => Ok(Json(Envelope::failure(&error))),
        }
    }

    /// Get one linked repository by id or canonical path.
    #[tool(
        name = "shehata_git_get_repository",
        description = "Get one linked repository by Shehata id or canonical path"
    )]
    async fn get_repository(
        &self,
        Parameters(args): Parameters<RepositoryArgs>,
    ) -> Result<Json<Envelope>, McpError> {
        let repository_id = match resolve_repository_id(&args).await? {
            Ok(id) => id,
            Err(error) => return Ok(Json(Envelope::failure(&error))),
        };
        match core_repositories::list_repository_summaries_with_routing().await {
            Ok(repositories) => match repositories
                .into_iter()
                .find(|repository| repository.id == repository_id)
            {
                Some(repository) => Ok(Json(Envelope::success(
                    format!("repository {}", repository.display_name),
                    McpRepository::from(repository),
                )?)),
                None => Ok(Json(Envelope::failure(&ShehataError::RepositoryNotFound(
                    repository_id,
                )))),
            },
            Err(error) => Ok(Json(Envelope::failure(&error))),
        }
    }

    /// Read identity state for a linked repository: assigned account plus the
    /// repository-local git user.name / user.email. Read-only.
    #[tool(
        name = "shehata_git_check_identity",
        description = "Show which account and commit identity a linked repository will use"
    )]
    async fn check_identity(
        &self,
        Parameters(args): Parameters<RepositoryArgs>,
    ) -> Result<Json<Envelope>, McpError> {
        let repository_id = match resolve_repository_id(&args).await? {
            Ok(id) => id,
            Err(error) => return Ok(Json(Envelope::failure(&error))),
        };
        let repositories = match core_repositories::list_repository_summaries_with_routing().await {
            Ok(repositories) => repositories,
            Err(error) => return Ok(Json(Envelope::failure(&error))),
        };
        let Some(repository) = repositories
            .into_iter()
            .find(|repository| repository.id == repository_id)
        else {
            return Ok(Json(Envelope::failure(&ShehataError::RepositoryNotFound(
                repository_id,
            ))));
        };
        let data = serde_json::json!({
            "repository": repository.display_name,
            "assigned_account": repository.assigned_login,
            "local_user_name": repository.commit_name,
            "local_user_email": repository.commit_email,
            "push_policy": repository.push_policy,
            "routing_configured": repository.routing_configured,
        });
        let summary = match &repository.assigned_login {
            Some(account) => format!("pushes authenticate as {account}"),
            None => "no account assigned yet".to_string(),
        };
        Ok(Json(Envelope::success(summary, data)?))
    }

    #[tool(
        name = "shehata_git_status",
        description = "Working-tree status of a linked repository without file contents"
    )]
    async fn status(
        &self,
        Parameters(args): Parameters<RepositoryArgs>,
    ) -> Result<Json<Envelope>, McpError> {
        let repository_id = match resolve_repository_id(&args).await? {
            Ok(id) => id,
            Err(error) => return Ok(Json(Envelope::failure(&error))),
        };
        match core_actions::status(&repository_id).await {
            Ok(status) => {
                let summary = format!("{} changed path(s)", status.changes.len());
                Ok(Json(Envelope::success(summary, status)?))
            }
            Err(error) => Ok(Json(Envelope::failure(&error))),
        }
    }

    #[tool(
        name = "shehata_git_diff_summary",
        description = "Counts of staged, unstaged, untracked, and conflicting paths; no file contents"
    )]
    async fn diff_summary(
        &self,
        Parameters(args): Parameters<RepositoryArgs>,
    ) -> Result<Json<Envelope>, McpError> {
        let repository_id = match resolve_repository_id(&args).await? {
            Ok(id) => id,
            Err(error) => return Ok(Json(Envelope::failure(&error))),
        };
        match core_actions::diff_summary(&repository_id).await {
            Ok(summary) => Ok(Json(Envelope::success(
                format!("{} changed path(s)", summary.changed_paths),
                summary,
            )?)),
            Err(error) => Ok(Json(Envelope::failure(&error))),
        }
    }

    #[tool(
        name = "shehata_git_test_connection",
        description = "Non-mutating connection test through the assigned account"
    )]
    async fn test_connection(
        &self,
        Parameters(args): Parameters<RepositoryArgs>,
    ) -> Result<Json<Envelope>, McpError> {
        let repository_id = match resolve_repository_id(&args).await? {
            Ok(id) => id,
            Err(error) => return Ok(Json(Envelope::failure(&error))),
        };
        match core_routing::test_connection(&repository_id).await {
            Ok(result) => Ok(Json(Envelope::success(
                format!("connection verified through @{}", result.account_login),
                result,
            )?)),
            Err(error) => Ok(Json(Envelope::failure(&error))),
        }
    }

    #[tool(
        name = "shehata_git_commit",
        description = "Stage explicit repository-relative paths and create a normal commit; amend is unavailable"
    )]
    async fn commit(
        &self,
        Parameters(args): Parameters<CommitArgs>,
    ) -> Result<Json<Envelope>, McpError> {
        let repository_args = RepositoryArgs {
            repository_id: args.repository_id,
            path: args.path,
        };
        let repository_id = match resolve_repository_id(&repository_args).await? {
            Ok(id) => id,
            Err(error) => return Ok(Json(Envelope::failure(&error))),
        };
        if let Err(error) = core_actions::stage(core_actions::PathsRequest {
            repository_id: repository_id.clone(),
            paths: args.paths,
        })
        .await
        {
            return Ok(Json(Envelope::failure(&error)));
        }
        match core_actions::commit(core_actions::CommitRequest {
            repository_id,
            message: args.message,
        })
        .await
        {
            Ok(result) => Ok(Json(Envelope::success("normal commit created", result)?)),
            Err(error) => Ok(Json(Envelope::failure(&error))),
        }
    }

    #[tool(
        name = "shehata_git_pull_ff_only",
        description = "Pull the existing upstream with --ff-only after identity preflight"
    )]
    async fn pull_ff_only(
        &self,
        Parameters(args): Parameters<RepositoryArgs>,
    ) -> Result<Json<Envelope>, McpError> {
        let repository_id = match resolve_repository_id(&args).await? {
            Ok(id) => id,
            Err(error) => return Ok(Json(Envelope::failure(&error))),
        };
        match core_actions::pull_ff_only(core_actions::RepositoryActionRequest { repository_id })
            .await
        {
            Ok(result) => Ok(Json(Envelope::success(
                "fast-forward pull completed",
                result,
            )?)),
            Err(error) => Ok(Json(Envelope::failure(&error))),
        }
    }

    #[tool(
        name = "shehata_git_push",
        description = "Normal push with full preflight; force is unavailable and approval policies are never bypassed"
    )]
    async fn push(
        &self,
        Parameters(args): Parameters<RepositoryArgs>,
    ) -> Result<Json<Envelope>, McpError> {
        let repository_id = match resolve_repository_id(&args).await? {
            Ok(id) => id,
            Err(error) => return Ok(Json(Envelope::failure(&error))),
        };
        match core_actions::push(core_actions::PushRequest {
            repository_id,
            caller: core_actions::ActionCaller::Mcp,
            approved: false,
        })
        .await
        {
            Ok(result) => Ok(Json(Envelope::success("normal push completed", result)?)),
            Err(error) => Ok(Json(Envelope::failure(&error))),
        }
    }
}

// ---------------------------------------------------------------------- main

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // stderr logging only — stdout is the MCP transport.
    let filter = tracing_subscriber::EnvFilter::try_from_env("SHEHATA_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    let service = ShehataMcp::new().serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary() -> core_repositories::RepositorySummary {
        core_repositories::RepositorySummary {
            id: "cd27fa83-54b9-40e4-bd22-9015e85998d9".into(),
            display_name: "site".into(),
            canonical_path: r"\?\D:\Clients\360 dental website\site".into(),
            host: Some("github.com".into()),
            owner: Some("acme".into()),
            repo_name: Some("site".into()),
            remote_name: Some("origin".into()),
            remote_url: Some("https://github.com/acme/site.git".into()),
            remote_protocol: Some("https".into()),
            current_branch: Some("master".into()),
            assigned_login: Some("acme-dev".into()),
            commit_name: Some("Acme Dev".into()),
            commit_email: Some("dev@acme.example".into()),
            inherited_commit_name: None,
            inherited_commit_email: None,
            push_policy: "allow_normal_push".into(),
            routing_configured: true,
        }
    }

    fn serialized() -> String {
        serde_json::to_string(&McpRepository::from(summary())).unwrap()
    }

    #[test]
    fn mcp_dto_excludes_absolute_paths() {
        let json = serialized();
        assert!(!json.contains("D:"), "{json}");
        assert!(!json.contains("Clients"), "{json}");
        assert!(!json.contains("canonical_path"), "{json}");
    }

    #[test]
    fn mcp_dto_excludes_raw_remote_url() {
        let json = serialized();
        assert!(!json.contains("remote_url"), "{json}");
        assert!(!json.contains("https://"), "{json}");
    }

    #[test]
    fn mcp_dto_excludes_email() {
        let json = serialized();
        assert!(!json.contains("dev@acme.example"), "{json}");
        assert!(!json.contains("commit_email"), "{json}");
    }

    #[test]
    fn mcp_dto_keeps_what_an_agent_needs_to_act() {
        let json = serialized();
        for expected in [
            "cd27fa83-54b9-40e4-bd22-9015e85998d9",
            "acme",
            "master",
            "acme-dev",
            "allow_normal_push",
        ] {
            assert!(json.contains(expected), "missing {expected} in {json}");
        }
    }

    #[test]
    fn mcp_failures_are_redacted() {
        let leaked = ShehataError::Internal(
            "remote https://octocat:hunter2@github.com/o/r.git rejected".into(),
        );
        let envelope = Envelope::failure(&leaked);
        assert!(!envelope.summary.contains("hunter2"));
        assert!(envelope.summary.contains("github.com/o/r.git"));
        assert!(!envelope.ok);
    }
}
