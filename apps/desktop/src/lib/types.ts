/**
 * Shared TypeScript types mirroring the Rust backend (serde) output.
 * Keep field names in snake_case to match serde defaults from shehata-core.
 */

export type CheckStatus = "ready" | "missing" | "needs_attention";

export interface SystemCheck {
  id: string;
  label: string;
  status: CheckStatus;
  /** Plain-language explanation of what this check means. */
  detail: string;
  /** Simple repair instruction shown when the check is not ready. */
  repair_hint: string | null;
  version: string | null;
  /** Accounts this check can repair in place, so the UI can offer a button. */
  repairable_accounts?: AccountScopeRepair[];
}

export interface DoctorReport {
  os: string;
  app_version: string;
  healthy: boolean;
  checks: SystemCheck[];
}

export interface GhAccount {
  host: string;
  login: string;
  active: boolean;
  token_available: boolean;
}

export interface AccountScopeRepair {
  host: string;
  login: string;
  scope: string;
}

export type GhLoginEvent =
  | { type: "started" }
  | { type: "waiting_for_browser" }
  | { type: "code"; code: string };

export interface RepositorySummary {
  id: string;
  display_name: string;
  canonical_path: string;
  host: string | null;
  owner: string | null;
  repo_name: string | null;
  remote_name: string | null;
  remote_url: string | null;
  remote_protocol: "https" | "ssh" | null;
  current_branch: string | null;
  assigned_login: string | null;
  commit_name: string | null;
  commit_email: string | null;
  /** What commits would be authored as if no identity is chosen here. Only
   *  set when the repository defines none of its own. */
  inherited_commit_name?: string | null;
  inherited_commit_email?: string | null;
  push_policy: string;
  routing_configured: boolean;
}

export interface AuditEvent {
  id: number;
  timestamp: string;
  repository_id: string | null;
  event_type: string;
  account_login: string | null;
  summary: string;
  /** Repository, branch, and commit context for this entry. */
  detail: string | null;
  result: string;
  exit_code: number | null;
}
