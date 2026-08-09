// Copyright (c) 2026 Dr Mohamed Shehata. All rights reserved.
// Licensed under the MIT License. See LICENSE in the project root.

//! Tauri bridge.
//!
//! Command handlers here are THIN: they call shehata-core and serialize the
//! result. No business logic lives in this crate, and no secret value ever
//! crosses to the frontend.

use serde::Serialize;
use shehata_core::{
    accounts as core_accounts, actions as core_actions, agents as core_agents,
    assignment as core_assignment, audit as core_audit, prerequisites as core_prerequisites,
    repositories as core_repositories, routing as core_routing, Doctor,
};
use shehata_github::{GhLoginEvent, GhRunner};
use shehata_storage::Database;
use tauri::Manager;
use tokio::sync::oneshot;

const APP_WINDOW_ICON: tauri::image::Image<'_> = tauri::include_image!("./icons/128x128.png");

#[derive(Default)]
struct LoginCancellation(std::sync::Mutex<Option<oneshot::Sender<()>>>);

#[derive(Debug, Serialize)]
struct McpInfo {
    executable_path: Option<String>,
    available: bool,
    config_snippet: String,
    detected_clients: Vec<shehata_core::integrations::AiClientInfo>,
}

fn open_db() -> Result<Database, String> {
    Database::open_default().map_err(|e| shehata_core::redact::redact_secrets(&e.to_string()))
}

#[tauri::command]
async fn doctor_run() -> Result<shehata_core::DoctorReport, String> {
    Ok(Doctor::new().run().await)
}

#[tauri::command]
async fn prerequisites_install(
    request: core_prerequisites::InstallPrerequisitesRequest,
) -> Result<core_prerequisites::InstallPrerequisitesResult, String> {
    core_prerequisites::install_prerequisites(request)
        .await
        .map_err(|error| shehata_core::redact::redact_secrets(&error.to_string()))
}

#[tauri::command]
fn prerequisites_available() -> bool {
    core_prerequisites::package_manager_available()
}

#[tauri::command]
async fn accounts_list() -> Result<Vec<shehata_core::AccountInfo>, String> {
    let gh =
        GhRunner::locate().map_err(|e| shehata_core::redact::redact_secrets(&e.to_string()))?;
    let accounts = core_accounts::list_accounts(&gh)
        .await
        .map_err(|e| shehata_core::redact::redact_secrets(&e.to_string()))?;
    if let Ok(db) = open_db() {
        core_accounts::mirror_accounts(&db, &accounts);
    }
    Ok(accounts)
}

#[tauri::command]
async fn accounts_add(
    cancellation: tauri::State<'_, LoginCancellation>,
    on_event: tauri::ipc::Channel<GhLoginEvent>,
) -> Result<Vec<shehata_core::AccountInfo>, String> {
    let gh =
        GhRunner::locate().map_err(|e| shehata_core::redact::redact_secrets(&e.to_string()))?;
    let progress = on_event.clone();
    let (cancel_tx, cancel_rx) = oneshot::channel();
    if let Ok(mut pending) = cancellation.0.lock() {
        if let Some(previous) = pending.replace(cancel_tx) {
            let _ = previous.send(());
        }
    }
    let login_result = gh
        .login_web_cancellable(
            move |event| {
                // The window may close while gh is waiting. A disconnected progress
                // channel must not turn a successful browser login into a failure.
                let _ = progress.send(event);
            },
            cancel_rx,
        )
        .await;
    if let Ok(mut pending) = cancellation.0.lock() {
        pending.take();
    }
    login_result.map_err(|e| shehata_core::redact::redact_secrets(&e.to_string()))?;

    let accounts = core_accounts::list_accounts(&gh)
        .await
        .map_err(|e| shehata_core::redact::redact_secrets(&e.to_string()))?;
    if let Ok(db) = open_db() {
        core_accounts::mirror_accounts(&db, &accounts);
    }
    Ok(accounts)
}

#[tauri::command]
fn accounts_cancel_login(cancellation: tauri::State<'_, LoginCancellation>) -> bool {
    cancellation
        .0
        .lock()
        .ok()
        .and_then(|mut pending| pending.take())
        .is_some_and(|sender| sender.send(()).is_ok())
}

#[tauri::command]
async fn accounts_remove(
    request: core_accounts::RemoveAccountRequest,
) -> Result<Vec<shehata_core::AccountInfo>, String> {
    let gh =
        GhRunner::locate().map_err(|e| shehata_core::redact::redact_secrets(&e.to_string()))?;
    let accounts = core_accounts::remove_account(&gh, &request)
        .await
        .map_err(|e| shehata_core::redact::redact_secrets(&e.to_string()))?;
    if let Ok(db) = open_db() {
        core_accounts::mirror_accounts(&db, &accounts);
    }
    Ok(accounts)
}

#[tauri::command]
async fn accounts_switch(
    request: core_accounts::SwitchAccountRequest,
) -> Result<Vec<shehata_core::AccountInfo>, String> {
    let gh =
        GhRunner::locate().map_err(|e| shehata_core::redact::redact_secrets(&e.to_string()))?;
    let accounts = core_accounts::switch_active_account(&gh, &request)
        .await
        .map_err(|e| shehata_core::redact::redact_secrets(&e.to_string()))?;
    if let Ok(db) = open_db() {
        core_accounts::mirror_accounts(&db, &accounts);
    }
    Ok(accounts)
}

#[tauri::command]
async fn accounts_grant_scope(
    cancellation: tauri::State<'_, LoginCancellation>,
    request: core_accounts::GrantScopeRequest,
    on_event: tauri::ipc::Channel<GhLoginEvent>,
) -> Result<Vec<shehata_core::AccountInfo>, String> {
    let gh =
        GhRunner::locate().map_err(|e| shehata_core::redact::redact_secrets(&e.to_string()))?;
    let progress = on_event.clone();
    let (cancel_tx, cancel_rx) = oneshot::channel();
    if let Ok(mut pending) = cancellation.0.lock() {
        if let Some(previous) = pending.replace(cancel_tx) {
            let _ = previous.send(());
        }
    }
    let result = core_accounts::grant_scope(
        &gh,
        &request,
        move |event| {
            let _ = progress.send(event);
        },
        cancel_rx,
    )
    .await;
    if let Ok(mut pending) = cancellation.0.lock() {
        pending.take();
    }
    let accounts = result.map_err(|e| shehata_core::redact::redact_secrets(&e.to_string()))?;
    if let Ok(db) = open_db() {
        core_accounts::mirror_accounts(&db, &accounts);
    }
    Ok(accounts)
}

#[tauri::command]
async fn repositories_list() -> Result<Vec<core_repositories::RepositorySummary>, String> {
    core_repositories::list_repository_summaries_with_routing()
        .await
        .map_err(|e| shehata_core::redact::redact_secrets(&e.to_string()))
}

#[tauri::command]
async fn repositories_add(path: String) -> Result<core_repositories::RepositorySummary, String> {
    let discovered = core_repositories::discover_selected_repository(&path)
        .await
        .map_err(|e| shehata_core::redact::redact_secrets(&e.to_string()))?;
    let db = open_db()?;
    let saved = core_repositories::save_discovered_repository(&db, &discovered)
        .map_err(|e| shehata_core::redact::redact_secrets(&e.to_string()))?;
    core_repositories::discovered_repository_summary(&db, saved, &discovered)
        .map_err(|e| shehata_core::redact::redact_secrets(&e.to_string()))
}

#[tauri::command]
async fn repositories_assign(
    request: core_assignment::AssignRepositoryRequest,
) -> Result<core_assignment::AssignmentResult, String> {
    core_assignment::assign_repository(request)
        .await
        .map_err(|e| shehata_core::redact::redact_secrets(&e.to_string()))
}

#[tauri::command]
async fn repositories_link(
    request: core_routing::LinkRepositoryRequest,
) -> Result<core_routing::RoutingResult, String> {
    core_routing::link_repository(request)
        .await
        .map_err(|e| shehata_core::redact::redact_secrets(&e.to_string()))
}

#[tauri::command]
async fn repositories_test(
    repository_id: String,
) -> Result<core_routing::ConnectionTestResult, String> {
    core_routing::test_connection(&repository_id)
        .await
        .map_err(|e| shehata_core::redact::redact_secrets(&e.to_string()))
}

#[tauri::command]
async fn repositories_unlink(
    request: core_routing::UnlinkRepositoryRequest,
) -> Result<core_routing::UnlinkResult, String> {
    core_routing::unlink_repository(request)
        .await
        .map_err(|e| shehata_core::redact::redact_secrets(&e.to_string()))
}

#[tauri::command]
async fn repositories_status(
    repository_id: String,
) -> Result<core_actions::RepositoryActionStatus, String> {
    core_actions::status(&repository_id)
        .await
        .map_err(|e| shehata_core::redact::redact_secrets(&e.to_string()))
}

#[tauri::command]
async fn repositories_file_diff(
    request: core_actions::FileDiffRequest,
) -> Result<core_actions::FileDiff, String> {
    core_actions::file_diff(request)
        .await
        .map_err(|e| shehata_core::redact::redact_secrets(&e.to_string()))
}

#[tauri::command]
async fn repositories_sync_preview(
    repository_id: String,
) -> Result<core_actions::SyncPreview, String> {
    core_actions::sync_preview(&repository_id)
        .await
        .map_err(|e| shehata_core::redact::redact_secrets(&e.to_string()))
}

#[tauri::command]
async fn repositories_stage(
    request: core_actions::PathsRequest,
) -> Result<core_actions::GitActionResult, String> {
    core_actions::stage(request)
        .await
        .map_err(|e| shehata_core::redact::redact_secrets(&e.to_string()))
}

#[tauri::command]
async fn repositories_unstage(
    request: core_actions::PathsRequest,
) -> Result<core_actions::GitActionResult, String> {
    core_actions::unstage(request)
        .await
        .map_err(|e| shehata_core::redact::redact_secrets(&e.to_string()))
}

#[tauri::command]
async fn repositories_commit(
    request: core_actions::CommitRequest,
) -> Result<core_actions::GitActionResult, String> {
    core_actions::commit(request)
        .await
        .map_err(|e| shehata_core::redact::redact_secrets(&e.to_string()))
}

#[tauri::command]
async fn repositories_pull(
    request: core_actions::RepositoryActionRequest,
) -> Result<core_actions::NetworkActionResult, String> {
    core_actions::pull_ff_only(request)
        .await
        .map_err(|e| shehata_core::redact::redact_secrets(&e.to_string()))
}

#[tauri::command]
async fn repositories_push(
    request: core_actions::PushRequest,
) -> Result<core_actions::NetworkActionResult, String> {
    core_actions::push(request)
        .await
        .map_err(|e| shehata_core::redact::redact_secrets(&e.to_string()))
}

#[tauri::command]
fn repositories_set_push_policy(
    request: core_actions::SetPushPolicyRequest,
) -> Result<core_actions::PushPolicyResult, String> {
    core_actions::set_push_policy(request)
        .map_err(|e| shehata_core::redact::redact_secrets(&e.to_string()))
}

#[tauri::command]
fn audit_list() -> Result<Vec<shehata_storage::AuditEventRecord>, String> {
    let db = open_db()?;
    shehata_storage::queries::list_audit_events(&db, 200)
        .map_err(|e| shehata_core::redact::redact_secrets(&e.to_string()))
}

#[tauri::command]
fn audit_delete(id: i64) -> Result<bool, String> {
    let db = open_db()?;
    core_audit::delete_event(&db, id)
        .map_err(|e| shehata_core::redact::redact_secrets(&e.to_string()))
}

#[tauri::command]
fn audit_clear() -> Result<usize, String> {
    let db = open_db()?;
    core_audit::clear_history(&db).map_err(|e| shehata_core::redact::redact_secrets(&e.to_string()))
}

#[tauri::command]
fn mcp_info() -> McpInfo {
    let exe = locate_mcp_binary();
    let command_path = exe
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "shehata-mcp".to_string());
    // Escape backslashes for JSON embedding.
    let escaped = command_path.replace('\\', "\\\\");
    let config_snippet = format!(
        "{{\n  \"mcpServers\": {{\n    \"shehata-git\": {{\n      \"command\": \"{escaped}\"\n    }}\n  }}\n}}"
    );
    McpInfo {
        available: exe.is_some(),
        executable_path: exe.map(|p| p.display().to_string()),
        config_snippet,
        detected_clients: shehata_core::integrations::detect_ai_clients(),
    }
}

#[tauri::command]
async fn diagnostics_report() -> Result<shehata_core::diagnostics::SafeDiagnosticReport, String> {
    shehata_core::diagnostics::safe_diagnostic_report()
        .await
        .map_err(|e| shehata_core::redact::redact_secrets(&e.to_string()))
}

#[tauri::command]
fn repositories_generate_agents(
    request: core_agents::GenerateAgentsRequest,
) -> Result<core_agents::GenerateAgentsResult, String> {
    core_agents::generate_agents(request)
        .map_err(|e| shehata_core::redact::redact_secrets(&e.to_string()))
}

fn locate_mcp_binary() -> Option<std::path::PathBuf> {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join("shehata-mcp.exe");
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }
    which::which("shehata-mcp").ok()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    init_tracing();
    tauri::Builder::default()
        .manage(LoginCancellation::default())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            // Windows caches taskbar icons by AppUserModelID across upgrades.
            // Setting the live window icon keeps rebranded installs correct
            // immediately, without asking users to clear Explorer caches.
            if let Some(window) = app.get_webview_window("main") {
                window.set_icon(APP_WINDOW_ICON.clone())?;
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            doctor_run,
            prerequisites_install,
            prerequisites_available,
            accounts_list,
            accounts_add,
            accounts_cancel_login,
            accounts_remove,
            accounts_switch,
            accounts_grant_scope,
            repositories_list,
            repositories_add,
            repositories_assign,
            repositories_link,
            repositories_test,
            repositories_unlink,
            repositories_status,
            repositories_file_diff,
            repositories_sync_preview,
            repositories_stage,
            repositories_unstage,
            repositories_commit,
            repositories_pull,
            repositories_push,
            repositories_set_push_policy,
            audit_list,
            audit_delete,
            audit_clear,
            mcp_info,
            diagnostics_report,
            repositories_generate_agents,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Shehata Git");
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
