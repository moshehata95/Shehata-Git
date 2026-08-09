// Copyright (c) 2026 Dr Mohamed Shehata. All rights reserved.
// Licensed under the MIT License. See LICENSE in the project root.

//! `shehata` — safe command-line access to the same core used by the desktop.
//!
//! Human-readable output is the default; `--json` produces stable structured
//! output. Credentials never cross this process boundary or reach stdout.

use std::path::PathBuf;
use std::process::{ExitCode, Stdio};

use clap::{Parser, Subcommand};
use serde::Serialize;
use shehata_core::{
    accounts as core_accounts, actions as core_actions, assignment as core_assignment, redact,
    repositories as core_repositories, routing as core_routing, Doctor, ShehataError,
};
use shehata_github::GhRunner;
use shehata_storage::Database;

const EXIT_FAILURE: u8 = 1;
const EXIT_UNHEALTHY: u8 = 4;

#[derive(Parser)]
#[command(
    name = "shehata",
    version,
    about = "Shehata Git — one repo, one identity, zero switching.",
    long_about = None
)]
struct Cli {
    /// Machine-readable JSON output.
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Check prerequisites and show simple repair guidance.
    Doctor,
    /// Manage GitHub accounts through the official GitHub CLI.
    #[command(subcommand)]
    Accounts(AccountsCommands),
    /// Manage registered repositories and their identity route.
    #[command(subcommand)]
    Repos(ReposCommands),
    /// Show working-tree status for a registered repository.
    Status {
        /// Repository path (defaults to the current directory).
        path: Option<String>,
    },
    /// Test the assigned account using non-mutating `git ls-remote`.
    Test {
        /// Repository path (defaults to the current directory).
        path: Option<String>,
    },
    /// Perform a normal push after full preflight. Force is never available.
    Push {
        /// Repository path (defaults to the current directory).
        path: Option<String>,
        /// Confirm the push explicitly. Kept for existing scripts; a push
        /// from the command line is already a human action.
        #[arg(long)]
        yes: bool,
    },
    /// Run a GitHub CLI command as the account assigned to this repository.
    ///
    /// The GitHub CLI has no per-repository account, so `gh pr create` normally
    /// uses whichever account is the CLI default. This applies the repository's
    /// own identity for one command, then restores the previous default.
    #[command(trailing_var_arg = true)]
    Gh {
        /// Repository path (defaults to the current directory).
        #[arg(long)]
        path: Option<String>,
        /// Arguments passed straight to the GitHub CLI.
        #[arg(allow_hyphen_values = true, required = true)]
        args: Vec<String>,
    },
    /// Start the native Shehata MCP server on stdio.
    Mcp,
    /// Installer-only user PATH maintenance.
    #[command(hide = true, subcommand)]
    Path(PathCommands),
    /// Record an operation performed outside this app. Called by the audit
    /// hooks installed into a routed repository; not meant to be typed.
    #[command(hide = true)]
    HookEvent {
        /// Repository UUID, written into the hook when routing was enabled.
        #[arg(long)]
        repo_id: String,
        /// One of: external_push, external_commit, external_pull.
        #[arg(long)]
        event: String,
        #[arg(long, default_value = "")]
        branch: String,
        #[arg(long, default_value = "")]
        commit: String,
        /// Subject line of the commit at HEAD.
        #[arg(long, default_value = "")]
        subject: String,
        #[arg(long, default_value = "")]
        remote: String,
    },
}

#[derive(Subcommand)]
enum PathCommands {
    /// Add the installation directory to the current user's PATH.
    Install { directory: PathBuf },
    /// Remove the installation directory from the current user's PATH.
    Uninstall { directory: PathBuf },
}

#[derive(Subcommand)]
enum AccountsCommands {
    /// List authenticated GitHub CLI accounts.
    List,
    /// Re-check accounts and refresh the safe local mirror.
    Refresh,
}

#[derive(Subcommand)]
enum ReposCommands {
    /// List registered repositories.
    List,
    /// Inspect and register a local Git worktree.
    Add { path: String },
    /// Show one repository by path, path-inside-worktree, or UUID.
    Show { path_or_id: Option<String> },
    /// Assign an account and enable repository-scoped credential routing.
    Assign {
        path_or_id: String,
        #[arg(long)]
        account: String,
        #[arg(long, default_value = "github.com")]
        host: String,
    },
    /// Unlink and restore original local Git configuration and identity.
    Unlink { path_or_id: String },
}

#[derive(Serialize)]
struct AssignmentOutput {
    assignment: core_assignment::AssignmentResult,
    routing: core_routing::RoutingResult,
}

#[tokio::main]
async fn main() -> ExitCode {
    init_tracing();
    let cli = Cli::parse();
    let result = match cli.command {
        Commands::Doctor => cmd_doctor(cli.json).await,
        Commands::Accounts(AccountsCommands::List | AccountsCommands::Refresh) => {
            cmd_accounts_list(cli.json).await
        }
        Commands::Repos(ReposCommands::List) => cmd_repos_list(cli.json).await,
        Commands::Repos(ReposCommands::Add { path }) => cmd_repos_add(cli.json, &path).await,
        Commands::Repos(ReposCommands::Show { path_or_id }) => {
            cmd_repos_show(cli.json, path_or_id.as_deref()).await
        }
        Commands::Repos(ReposCommands::Assign {
            path_or_id,
            account,
            host,
        }) => cmd_repos_assign(cli.json, &path_or_id, &host, &account).await,
        Commands::Repos(ReposCommands::Unlink { path_or_id }) => {
            cmd_repos_unlink(cli.json, &path_or_id).await
        }
        Commands::Status { path } => cmd_status(cli.json, path.as_deref()).await,
        Commands::Test { path } => cmd_test(cli.json, path.as_deref()).await,
        Commands::Push { path, yes } => cmd_push(cli.json, path.as_deref(), yes).await,
        Commands::Gh { path, args } => cmd_gh(cli.json, path.as_deref(), &args).await,
        Commands::Mcp => cmd_mcp(cli.json).await,
        Commands::Path(PathCommands::Install { directory }) => {
            cmd_user_path(cli.json, &directory, true)
        }
        Commands::Path(PathCommands::Uninstall { directory }) => {
            cmd_user_path(cli.json, &directory, false)
        }
        Commands::HookEvent {
            repo_id,
            event,
            branch,
            commit,
            subject,
            remote,
        } => cmd_hook_event(&repo_id, &event, &branch, &commit, &subject, &remote),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(code) => ExitCode::from(code),
    }
}

async fn cmd_doctor(json: bool) -> Result<(), u8> {
    let report = Doctor::new().run().await;
    if json {
        print_json(&report)?;
    } else {
        println!("Shehata Git system check ({})\n", safe_text(&report.os));
        for check in &report.checks {
            let symbol = match check.status {
                shehata_core::CheckStatus::Ready => "  OK  ",
                shehata_core::CheckStatus::Missing => " MISS ",
                shehata_core::CheckStatus::NeedsAttention => " WARN ",
            };
            println!("[{symbol}] {}", safe_text(&check.label));
            if let Some(version) = &check.version {
                println!("         {}", safe_text(version));
            }
            println!("         {}", safe_text(&check.detail));
            if let Some(hint) = &check.repair_hint {
                println!("         fix: {}", safe_text(hint));
            }
        }
        println!();
        println!(
            "{}",
            if report.healthy {
                "Everything Shehata Git needs is in place."
            } else {
                "Some checks need attention — see the fix lines above."
            }
        );
    }
    if report.healthy {
        Ok(())
    } else {
        Err(EXIT_UNHEALTHY)
    }
}

async fn cmd_accounts_list(json: bool) -> Result<(), u8> {
    let gh = GhRunner::locate().map_err(|error| fail_message(json, "github_cli_error", &error))?;
    let accounts = core_accounts::list_accounts(&gh)
        .await
        .map_err(|error| fail(json, &error))?;
    if let Ok(db) = Database::open_default() {
        core_accounts::mirror_accounts(&db, &accounts);
    }
    if json {
        print_json(&accounts)
    } else if accounts.is_empty() {
        println!("No GitHub accounts are signed in.");
        println!("Sign in from the desktop app or run: gh auth login");
        Ok(())
    } else {
        for account in &accounts {
            println!(
                "@{} on {}{} — {}",
                safe_text(&account.login),
                safe_text(&account.host),
                if account.active { " (CLI default)" } else { "" },
                if account.token_available {
                    "ready"
                } else {
                    "NEEDS SIGN-IN"
                }
            );
        }
        Ok(())
    }
}

async fn cmd_repos_list(json: bool) -> Result<(), u8> {
    let repositories = core_repositories::list_repository_summaries_with_routing()
        .await
        .map_err(|error| fail(json, &error))?;
    if json {
        print_json(&repositories)
    } else if repositories.is_empty() {
        println!("No repositories registered yet.");
        Ok(())
    } else {
        for repository in repositories {
            let state = if repository.routing_configured {
                "routed"
            } else if repository.assigned_login.is_some() {
                "assigned"
            } else {
                "NO ACCOUNT"
            };
            println!(
                "{} — {} [{}]",
                safe_text(&repository.display_name),
                safe_text(&repository.canonical_path),
                state
            );
        }
        Ok(())
    }
}

async fn cmd_repos_add(json: bool, path: &str) -> Result<(), u8> {
    let discovered = core_repositories::discover_selected_repository(path)
        .await
        .map_err(|error| fail(json, &error))?;
    let db =
        Database::open_default().map_err(|error| fail_message(json, "storage_error", &error))?;
    let record = core_repositories::save_discovered_repository(&db, &discovered)
        .map_err(|error| fail(json, &error))?;
    let summary = core_repositories::discovered_repository_summary(&db, record, &discovered)
        .map_err(|error| fail(json, &error))?;
    if json {
        print_json(&summary)
    } else {
        println!("Registered {}.", safe_text(&summary.display_name));
        println!("Path: {}", safe_text(&summary.canonical_path));
        // Say what commits would be authored as when the repository sets
        // nothing itself, so the default is seen rather than discovered later
        // in a commit that carries the wrong name.
        if summary.inherited_commit_name.is_some() || summary.inherited_commit_email.is_some() {
            let name = summary
                .inherited_commit_name
                .as_deref()
                .unwrap_or("(unset)");
            let email = summary
                .inherited_commit_email
                .as_deref()
                .unwrap_or("(unset)");
            println!(
                "Warning: this repository sets no author, so commits would be made as {} <{}>, inherited from your global Git configuration.",
                safe_text(name),
                safe_text(email)
            );
            println!("Set one for this repository with `shehata repos assign`.");
        }
        println!("Next: assign an account with `shehata repos assign`. ");
        Ok(())
    }
}

async fn cmd_repos_show(json: bool, reference: Option<&str>) -> Result<(), u8> {
    let repository = core_repositories::resolve_repository_reference(reference)
        .await
        .map_err(|error| fail(json, &error))?;
    let summary = core_repositories::list_repository_summaries_with_routing()
        .await
        .map_err(|error| fail(json, &error))?
        .into_iter()
        .find(|summary| summary.id == repository.id)
        .ok_or_else(|| {
            fail_message_text(
                json,
                "repository_not_found",
                "repository disappeared while it was being read",
            )
        })?;
    if json {
        print_json(&summary)
    } else {
        println!("{}", safe_text(&summary.display_name));
        println!("  id: {}", safe_text(&summary.id));
        println!("  path: {}", safe_text(&summary.canonical_path));
        println!(
            "  remote: {}",
            safe_text(summary.remote_url.as_deref().unwrap_or("not configured"))
        );
        println!(
            "  account: {}",
            safe_text(summary.assigned_login.as_deref().unwrap_or("not assigned"))
        );
        println!(
            "  routing: {}",
            if summary.routing_configured {
                "configured"
            } else {
                "not configured"
            }
        );
        println!("  push policy: {}", safe_text(&summary.push_policy));
        Ok(())
    }
}

async fn cmd_repos_assign(json: bool, reference: &str, host: &str, login: &str) -> Result<(), u8> {
    let repository = core_repositories::resolve_repository_reference(Some(reference))
        .await
        .map_err(|error| fail(json, &error))?;
    let assignment = core_assignment::assign_repository(core_assignment::AssignRepositoryRequest {
        repository_id: repository.id.clone(),
        host: host.to_string(),
        login: login.to_string(),
        commit_name: None,
        commit_email: None,
    })
    .await
    .map_err(|error| fail(json, &error))?;
    let routing = core_routing::link_repository(core_routing::LinkRepositoryRequest {
        repository_id: repository.id,
    })
    .await
    .map_err(|error| fail(json, &error))?;
    let output = AssignmentOutput {
        assignment,
        routing,
    };
    if json {
        print_json(&output)
    } else {
        println!(
            "Assigned @{} and enabled credential routing for {}.",
            safe_text(login),
            safe_text(&output.assignment.repository.display_name)
        );
        Ok(())
    }
}

async fn cmd_repos_unlink(json: bool, reference: &str) -> Result<(), u8> {
    let repository = core_repositories::resolve_repository_reference(Some(reference))
        .await
        .map_err(|error| fail(json, &error))?;
    let result = core_routing::unlink_repository(core_routing::UnlinkRepositoryRequest {
        repository_id: repository.id,
        restore_identity: true,
    })
    .await
    .map_err(|error| fail(json, &error))?;
    if json {
        print_json(&result)
    } else {
        println!("Repository unlinked; original local Git configuration was restored.");
        Ok(())
    }
}

async fn cmd_gh(json: bool, reference: Option<&str>, args: &[String]) -> Result<(), u8> {
    let repository = core_repositories::resolve_repository_reference(reference)
        .await
        .map_err(|error| fail(json, &error))?;
    let gh = GhRunner::locate().map_err(|_| {
        fail_message_text(
            json,
            "github_cli_error",
            "GitHub CLI (gh) was not found on PATH",
        )
    })?;
    let code = core_accounts::run_gh_for_repository(&gh, &repository.id, args)
        .await
        .map_err(|error| fail(json, &error))?;
    // The GitHub CLI has already written its own output to the terminal, so
    // only its exit code is propagated here.
    if code == 0 {
        Ok(())
    } else {
        Err(u8::try_from(code).unwrap_or(1))
    }
}

async fn cmd_status(json: bool, reference: Option<&str>) -> Result<(), u8> {
    let repository = core_repositories::resolve_repository_reference(reference)
        .await
        .map_err(|error| fail(json, &error))?;
    let status = core_actions::status(&repository.id)
        .await
        .map_err(|error| fail(json, &error))?;
    if json {
        print_json(&status)
    } else {
        println!(
            "Branch: {}{}",
            safe_text(status.branch.as_deref().unwrap_or("none")),
            if status.detached_head {
                " (detached)"
            } else {
                ""
            }
        );
        if status.changes.is_empty() {
            println!("Working tree clean.");
        } else {
            for change in status.changes {
                println!(
                    "{}{} {}",
                    safe_text(&change.index_status),
                    safe_text(&change.worktree_status),
                    safe_text(&change.path)
                );
            }
        }
        Ok(())
    }
}

async fn cmd_test(json: bool, reference: Option<&str>) -> Result<(), u8> {
    let repository = core_repositories::resolve_repository_reference(reference)
        .await
        .map_err(|error| fail(json, &error))?;
    let result = core_routing::test_connection(&repository.id)
        .await
        .map_err(|error| fail(json, &error))?;
    if json {
        print_json(&result)
    } else {
        println!(
            "Connection verified through @{} on {}.",
            safe_text(&result.account_login),
            safe_text(&result.remote_name)
        );
        Ok(())
    }
}

async fn cmd_push(json: bool, reference: Option<&str>, approved: bool) -> Result<(), u8> {
    let repository = core_repositories::resolve_repository_reference(reference)
        .await
        .map_err(|error| fail(json, &error))?;
    let result = core_actions::push(core_actions::PushRequest {
        repository_id: repository.id,
        caller: core_actions::ActionCaller::Cli,
        approved,
    })
    .await
    .map_err(|error| fail(json, &error))?;
    if json {
        print_json(&result)
    } else {
        println!(
            "Normal push completed: {}/{} through @{}.",
            safe_text(&result.remote_name),
            safe_text(&result.branch),
            safe_text(&result.account_login)
        );
        Ok(())
    }
}

async fn cmd_mcp(json: bool) -> Result<(), u8> {
    let executable = locate_mcp_binary().ok_or_else(|| {
        fail_message_text(
            json,
            "mcp_not_found",
            "shehata-mcp was not found beside the CLI or on PATH",
        )
    })?;
    let status = tokio::process::Command::new(executable)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .await
        .map_err(|error| fail_message(json, "mcp_spawn_error", &error))?;
    if status.success() {
        Ok(())
    } else {
        Err(status.code().unwrap_or(EXIT_FAILURE.into()).clamp(1, 255) as u8)
    }
}

fn locate_mcp_binary() -> Option<PathBuf> {
    let filename = if cfg!(windows) {
        "shehata-mcp.exe"
    } else {
        "shehata-mcp"
    };
    if let Ok(current) = std::env::current_exe() {
        if let Some(directory) = current.parent() {
            let sibling = directory.join(filename);
            if sibling.is_file() {
                return Some(sibling);
            }
        }
    }
    which::which("shehata-mcp").ok()
}

/// Called by git hooks to record external operations in the audit log.
/// Runs silently — errors are swallowed so hooks never block git.
/// Record an operation git performed outside this application.
///
/// Only the audit hooks call this. Every input arrives from a shell script, so
/// nothing is trusted: the repository id must be a UUID, the event must be one
/// this application installed a hook for, and free text is redacted, stripped
/// of control characters, and truncated before it reaches the database.
///
/// Entries are shaped like the app's own so the trail reads as one history:
/// the change is the title, and the context sits on the line beneath it.
fn cmd_hook_event(
    repo_id: &str,
    event: &str,
    branch: &str,
    commit: &str,
    subject: &str,
    remote: &str,
) -> Result<(), u8> {
    let repo_id = repo_id.trim();
    if !uuid_shaped(repo_id) {
        return Err(EXIT_FAILURE);
    }
    let label = match event {
        "external_push" => "Push outside the app",
        "external_commit" => "Commit outside the app",
        "external_pull" => "Pull outside the app",
        _ => return Err(EXIT_FAILURE),
    };

    let db_path = Database::default_path().map_err(|_| EXIT_FAILURE)?;
    let db = Database::open_at(&db_path).map_err(|_| EXIT_FAILURE)?;

    // Name the repository the way the rest of the trail does, and record the
    // identity the operation authenticated as.
    //
    // Without this the trail needs two rows to answer one question: the hook
    // row says what happened, and a separate credential row says which account
    // it happened as. An entry that only answers half of that is the kind of
    // record that looks complete while hiding the part that matters here.
    let repository = shehata_storage::queries::find_repository_by_id(&db, repo_id)
        .ok()
        .flatten();
    let display_name = repository
        .as_ref()
        .map(|repository| repository.display_name.clone())
        .unwrap_or_else(|| "unknown repository".to_string());
    let account_login = repository
        .as_ref()
        .and_then(|repository| repository.assigned_account_id)
        .and_then(|account_id| {
            shehata_storage::queries::find_account_by_id(&db, account_id)
                .ok()
                .flatten()
        })
        .map(|account| account.login);

    let subject = clean(subject, 60);
    let summary = if subject.is_empty() {
        label.to_string()
    } else {
        subject
    };

    let mut detail = vec![label.to_string(), display_name];
    for (value, limit) in [(remote, 40), (branch, 60)] {
        let value = clean(value, limit);
        if !value.is_empty() {
            detail.push(value);
        }
    }
    let commit = clean(commit, 40);
    if !commit.is_empty() {
        detail.push(commit.chars().take(7).collect());
    }

    shehata_storage::queries::insert_audit_event(
        &db,
        &shehata_storage::NewAuditEvent {
            event_type: event,
            repository_id: Some(repo_id),
            account_login: account_login.as_deref(),
            summary: &summary,
            detail: Some(&detail.join(" \u{b7} ")),
            result: "success",
            exit_code: Some(0),
            duration_ms: None,
        },
    )
    .map_err(|_| EXIT_FAILURE)?;
    Ok(())
}

/// Accept only a canonical UUID: the value is written into the audit trail and
/// used to look a repository up, and it arrives from a shell script.
fn uuid_shaped(value: &str) -> bool {
    value.len() == 36
        && value.matches('-').count() == 4
        && value.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

/// Shell-supplied text: redact, drop control characters, and bound the length.
fn clean(value: &str, max_chars: usize) -> String {
    redact::redact_secrets(value.trim())
        .chars()
        .filter(|c| !c.is_control())
        .take(max_chars)
        .collect::<String>()
        .trim()
        .to_string()
}

fn cmd_user_path(json: bool, directory: &std::path::Path, install: bool) -> Result<(), u8> {
    #[cfg(windows)]
    {
        update_windows_user_path(directory, install)
            .map_err(|error| fail_message(json, "path_update_error", &error))?;
        if json {
            print_json(&serde_json::json!({
                "updated": true,
                "operation": if install { "install" } else { "uninstall" },
                "directory": directory,
            }))
        } else {
            println!(
                "User PATH {} for {}.",
                if install { "updated" } else { "cleaned" },
                safe_text(&directory.display().to_string())
            );
            Ok(())
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (directory, install);
        Err(fail_message_text(
            json,
            "unsupported_platform",
            "installer PATH maintenance is only supported on Windows",
        ))
    }
}

#[cfg(windows)]
fn update_windows_user_path(directory: &std::path::Path, install: bool) -> std::io::Result<()> {
    use std::io::{Error, ErrorKind};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SendMessageTimeoutW, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE,
    };
    use winreg::enums::{HKEY_CURRENT_USER, REG_EXPAND_SZ, REG_SZ};
    use winreg::types::{FromRegValue, ToRegValue};
    use winreg::RegKey;

    if !directory.is_absolute() {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            "installation directory must be absolute",
        ));
    }
    let directory = directory.as_os_str().to_string_lossy();
    if directory.contains(';') || directory.contains('\0') {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            "installation directory contains an invalid PATH character",
        ));
    }

    let environment = RegKey::predef(HKEY_CURRENT_USER)
        .create_subkey("Environment")?
        .0;
    let existing = match environment.get_raw_value("Path") {
        Ok(value) => Some(value),
        Err(error) if error.kind() == ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    let current = match &existing {
        Some(value) if matches!(value.vtype, REG_SZ | REG_EXPAND_SZ) => {
            String::from_reg_value(value)?
        }
        Some(_) => {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "the current user PATH has an unsupported registry type",
            ));
        }
        None => String::new(),
    };

    let (updated, changed) = if install {
        add_path_entry(&current, &directory)
    } else {
        remove_path_entry(&current, &directory)
    };
    if !changed {
        return Ok(());
    }

    let mut value = updated.to_reg_value();
    value.vtype = existing.map_or(REG_EXPAND_SZ, |existing| existing.vtype);
    environment.set_raw_value("Path", &value)?;

    let environment_wide: Vec<u16> = "Environment\0".encode_utf16().collect();
    let mut ignored = 0usize;
    // SAFETY: all values are fixed Win32 constants and `environment_wide` is a
    // live, NUL-terminated UTF-16 buffer for the duration of this synchronous call.
    unsafe {
        SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            0,
            environment_wide.as_ptr() as isize,
            SMTO_ABORTIFHUNG,
            5_000,
            &mut ignored,
        );
    }
    Ok(())
}

fn add_path_entry(current: &str, directory: &str) -> (String, bool) {
    if current
        .split(';')
        .any(|entry| normalized_path_entry(entry) == normalized_path_entry(directory))
    {
        return (current.to_string(), false);
    }
    if current.is_empty() {
        (directory.to_string(), true)
    } else if current.ends_with(';') {
        (format!("{current}{directory}"), true)
    } else {
        (format!("{current};{directory}"), true)
    }
}

fn remove_path_entry(current: &str, directory: &str) -> (String, bool) {
    let expected = normalized_path_entry(directory);
    let entries: Vec<&str> = current.split(';').collect();
    let retained: Vec<&str> = entries
        .iter()
        .copied()
        .filter(|entry| normalized_path_entry(entry) != expected)
        .collect();
    if retained.len() == entries.len() {
        (current.to_string(), false)
    } else {
        (retained.join(";"), true)
    }
}

fn normalized_path_entry(entry: &str) -> String {
    entry
        .trim()
        .trim_matches('"')
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase()
}

fn print_json<T: Serialize>(value: &T) -> Result<(), u8> {
    let text = serde_json::to_string_pretty(value).map_err(|_| EXIT_FAILURE)?;
    println!("{text}");
    Ok(())
}

fn fail(json: bool, error: &ShehataError) -> u8 {
    let message = redact::redact_secrets(&error.to_string());
    if json {
        println!(
            "{}",
            serde_json::json!({ "error": { "code": error.code(), "message": message } })
        );
    } else {
        eprintln!("error [{}]: {}", error.code(), safe_text(&message));
    }
    EXIT_FAILURE
}

fn fail_message(json: bool, code: &str, error: &impl std::fmt::Display) -> u8 {
    fail_message_text(json, code, &error.to_string())
}

fn fail_message_text(json: bool, code: &str, message: &str) -> u8 {
    let message = redact::redact_secrets(message);
    if json {
        println!(
            "{}",
            serde_json::json!({ "error": { "code": code, "message": message } })
        );
    } else {
        eprintln!("error [{}]: {}", safe_text(code), safe_text(&message));
    }
    EXIT_FAILURE
}

/// Escape control characters so a hostile remote message cannot rewrite the
/// terminal. Secret redaction happens before this, never instead of it.
fn safe_text(value: &str) -> String {
    let mut safe = String::with_capacity(value.len());
    for character in value.chars() {
        if character.is_control() {
            safe.extend(character.escape_default());
        } else {
            safe.push(character);
        }
    }
    safe
}

fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_env("SHEHATA_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_shape_is_stable() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }

    #[test]
    fn terminal_text_escapes_control_characters() {
        assert_eq!(safe_text("ok\u{1b}[31m"), "ok\\u{1b}[31m");
    }

    #[test]
    fn path_entry_update_is_idempotent_and_case_insensitive() {
        let (added, changed) = add_path_entry(
            r"C:\Windows;C:\Tools",
            r"C:\Users\Me\AppData\Local\Shehata Git",
        );
        assert!(changed);
        assert_eq!(
            added,
            r"C:\Windows;C:\Tools;C:\Users\Me\AppData\Local\Shehata Git"
        );
        let (same, changed) = add_path_entry(&added, r"c:/users/me/appdata/local/shehata git\");
        assert!(!changed);
        assert_eq!(same, added);
    }

    #[test]
    fn path_entry_removal_preserves_unrelated_entries() {
        let (updated, changed) = remove_path_entry(
            r"C:\Windows;C:\Shehata Git;C:\Tools;C:\SHEHATA GIT\",
            r"c:/shehata git",
        );
        assert!(changed);
        assert_eq!(updated, r"C:\Windows;C:\Tools");
    }
}
