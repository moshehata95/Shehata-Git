import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  AlertCircle,
  ArrowDown,
  ArrowLeft,
  ArrowUp,
  Check,
  CheckCircle2,
  CloudCog,
  ExternalLink,
  EyeOff,
  FileCode2,
  FileDiff,
  GitBranch,
  GitCommit,
  History,
  KeyRound,
  Loader2,
  RefreshCw,
  Route,
  ShieldCheck,
  Sparkles,
} from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/ui/ConfirmDialog";
import { SearchField } from "@/components/ui/SearchField";
import {
  decideSmartSync,
  displayRepositoryPath,
  filterWorkspaceChanges,
  isStagedChange,
  type WorkspaceFileFilter,
} from "@/lib/repository-workspace";
import {
  commitRepository,
  getRepositoryFileDiff,
  getRepositoryStatus,
  listAuditEvents,
  listRepositories,
  type PushPolicy,
  previewRepositorySync,
  pullRepository,
  pushRepository,
  type SyncPreview,
  setRepositoryPushPolicy,
  stageRepositoryPaths,
  unstageRepositoryPaths,
} from "@/lib/tauri";
import { cn } from "@/lib/utils";

interface RepositoryDetailPageProps {
  repositoryId: string;
  onBack: () => void;
}

const WORKSPACE_FILE_FILTERS: Array<{
  value: WorkspaceFileFilter;
  label: string;
}> = [
  { value: "all", label: "All" },
  { value: "changed", label: "Changed" },
  { value: "staged", label: "Staged" },
  { value: "untracked", label: "Untracked" },
];

export function RepositoryDetailPage({ repositoryId, onBack }: RepositoryDetailPageProps) {
  const queryClient = useQueryClient();
  const [selectedPaths, setSelectedPaths] = useState<Set<string>>(new Set());
  const [activePath, setActivePath] = useState<string | null>(null);
  const [showStagedDiff, setShowStagedDiff] = useState(false);
  const [message, setMessage] = useState("");
  const [fileSearch, setFileSearch] = useState("");
  const [fileFilter, setFileFilter] = useState<WorkspaceFileFilter>("all");
  const [pendingSyncPush, setPendingSyncPush] = useState<SyncPreview | null>(null);
  const [notice, setNotice] = useState<{
    tone: "success" | "warning";
    message: string;
  } | null>(null);

  const repositories = useQuery({ queryKey: ["repositories"], queryFn: listRepositories });
  const repository = repositories.data?.find((item) => item.id === repositoryId);
  const status = useQuery({
    queryKey: ["repository-status", repositoryId],
    queryFn: () => getRepositoryStatus(repositoryId),
  });
  const activity = useQuery({ queryKey: ["audit"], queryFn: listAuditEvents });
  const activeChange = status.data?.changes.find((change) => change.path === activePath);
  const changes = status.data?.changes ?? [];
  const visibleChanges = useMemo(
    () => filterWorkspaceChanges(changes, fileSearch, fileFilter),
    [changes, fileFilter, fileSearch],
  );
  const filterCounts = useMemo(
    () => ({
      all: changes.length,
      changed: filterWorkspaceChanges(changes, "", "changed").length,
      staged: filterWorkspaceChanges(changes, "", "staged").length,
      untracked: filterWorkspaceChanges(changes, "", "untracked").length,
    }),
    [changes],
  );
  const allVisibleSelected =
    visibleChanges.length > 0 && visibleChanges.every((change) => selectedPaths.has(change.path));
  const canShowStaged = activeChange ? isStagedChange(activeChange) : false;
  const canShowWorking = activeChange ? activeChange.worktree_status !== " " : false;
  const diff = useQuery({
    queryKey: ["repository-diff", repositoryId, activePath, showStagedDiff],
    queryFn: () => getRepositoryFileDiff(repositoryId, activePath ?? "", showStagedDiff),
    enabled: Boolean(activePath),
  });

  useEffect(() => {
    if (changes.length === 0 || visibleChanges.length === 0) {
      setActivePath(null);
      return;
    }
    if (!activePath || !visibleChanges.some((change) => change.path === activePath)) {
      const first = visibleChanges[0];
      setActivePath(first.path);
      setShowStagedDiff(isStagedChange(first));
    }
  }, [activePath, changes.length, visibleChanges]);

  useEffect(() => {
    if (!activeChange) return;
    if (showStagedDiff && !canShowStaged) setShowStagedDiff(false);
    if (!showStagedDiff && !canShowWorking && canShowStaged) setShowStagedDiff(true);
  }, [activeChange, canShowStaged, canShowWorking, showStagedDiff]);

  const refresh = async () => {
    setSelectedPaths(new Set());
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: ["repository-status", repositoryId] }),
      queryClient.invalidateQueries({ queryKey: ["repository-diff", repositoryId] }),
      queryClient.invalidateQueries({ queryKey: ["repositories"] }),
      queryClient.invalidateQueries({ queryKey: ["audit"] }),
    ]);
  };

  const stage = useMutation({
    mutationFn: (paths: string[]) => stageRepositoryPaths(repositoryId, paths),
    onSuccess: async () => {
      setNotice({ tone: "success", message: "Selected files are staged and ready to commit." });
      await refresh();
    },
  });
  const unstage = useMutation({
    mutationFn: (paths: string[]) => unstageRepositoryPaths(repositoryId, paths),
    onSuccess: async () => {
      setNotice({
        tone: "success",
        message: "Selected files were removed from staging. Worktree files are untouched.",
      });
      await refresh();
    },
  });
  const commit = useMutation({
    mutationFn: () => commitRepository(repositoryId, message),
    onSuccess: async (result) => {
      setMessage("");
      setNotice({
        tone: "success",
        message: `Commit created: ${result.commit?.slice(0, 8) ?? "complete"}.`,
      });
      await refresh();
    },
  });
  const pull = useMutation({
    mutationFn: () => pullRepository(repositoryId),
    onSuccess: async (result) => {
      setNotice({
        tone: "success",
        message: `Fast-forwarded ${result.branch} through @${result.account_login}.`,
      });
      await refresh();
    },
  });
  const push = useMutation({
    mutationFn: () => pushRepository(repositoryId),
    onSuccess: async (result) => {
      setNotice({
        tone: "success",
        message: `Pushed ${result.branch} normally through @${result.account_login}.`,
      });
      await refresh();
    },
  });
  const inspectSync = useMutation({ mutationFn: () => previewRepositorySync(repositoryId) });
  const policy = useMutation({
    mutationFn: (value: PushPolicy) => setRepositoryPushPolicy(repositoryId, value),
    onSuccess: async () => {
      setNotice({ tone: "success", message: "Push policy updated for this repository." });
      await queryClient.invalidateQueries({ queryKey: ["repositories"] });
    },
  });

  const pending =
    stage.isPending ||
    unstage.isPending ||
    commit.isPending ||
    pull.isPending ||
    push.isPending ||
    inspectSync.isPending ||
    policy.isPending;
  const error =
    repositories.error ??
    status.error ??
    diff.error ??
    stage.error ??
    unstage.error ??
    commit.error ??
    pull.error ??
    push.error ??
    inspectSync.error ??
    policy.error;
  const stagedCount = status.data?.changes.filter(isStagedChange).length ?? 0;
  const recentActivity = useMemo(
    () => activity.data?.filter((event) => event.repository_id === repositoryId).slice(0, 6) ?? [],
    [activity.data, repositoryId],
  );

  function togglePath(path: string) {
    setSelectedPaths((current) => {
      const next = new Set(current);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  }

  function toggleVisiblePaths() {
    setSelectedPaths((current) => {
      const next = new Set(current);
      if (allVisibleSelected) {
        for (const change of visibleChanges) next.delete(change.path);
      } else {
        for (const change of visibleChanges) next.add(change.path);
      }
      return next;
    });
  }

  async function smartSync() {
    setNotice(null);
    const preview = await inspectSync.mutateAsync();
    const decision = decideSmartSync(preview.ahead, preview.behind);
    if (decision === "in_sync") {
      setNotice({
        tone: "success",
        message: `${preview.branch} is already in sync with ${preview.remote_name}.`,
      });
      return;
    }
    if (decision === "diverged") {
      setNotice({
        tone: "warning",
        message:
          "Sync paused: local and remote both have commits. Shehata Git will not merge or rebase automatically.",
      });
      return;
    }
    if (decision === "pull") {
      await pull.mutateAsync();
      inspectSync.reset();
      return;
    }
    setPendingSyncPush(preview);
  }

  async function confirmSmartPush() {
    try {
      await push.mutateAsync();
      inspectSync.reset();
    } finally {
      setPendingSyncPush(null);
    }
  }

  if (!repository && repositories.isLoading) {
    return <LoadingState label="Opening repository workspace…" />;
  }
  if (!repository) {
    return (
      <div className="liquid-panel mx-auto max-w-xl p-6">
        <p className="text-sm text-destructive">This repository is no longer registered.</p>
        <Button className="mt-4" variant="outline" onClick={onBack}>
          <ArrowLeft aria-hidden /> Back to repositories
        </Button>
      </div>
    );
  }

  const accountMismatch =
    repository.owner &&
    repository.assigned_login &&
    !repository.owner.toLowerCase().includes(repository.assigned_login.toLowerCase());

  return (
    <div className="mx-auto w-full max-w-[92rem] space-y-4">
      <button
        type="button"
        onClick={onBack}
        className="inline-flex min-h-11 items-center gap-2 text-sm font-medium text-muted-foreground transition-colors hover:text-foreground"
      >
        <ArrowLeft className="h-4 w-4" aria-hidden /> Repository registry
      </button>

      <section className="liquid-hero overflow-hidden rounded-[1.25rem]">
        <div className="grid gap-6 p-5 sm:p-7 lg:grid-cols-[minmax(0,1fr)_auto] lg:items-end">
          <div className="min-w-0">
            <div className="flex flex-wrap items-center gap-2">
              <Badge variant={repository.routing_configured ? "success" : "warning"}>
                {repository.routing_configured ? "Route active" : "Route incomplete"}
              </Badge>
              <span className="data-label">
                {repository.remote_protocol?.toUpperCase() ?? "LOCAL"}
              </span>
            </div>
            <h2 className="mt-4 truncate font-display text-3xl font-semibold tracking-[-0.04em] sm:text-4xl">
              {repository.display_name}
            </h2>
            <p
              className="mt-2 truncate font-mono text-xs text-muted-foreground"
              title={repository.canonical_path}
            >
              {displayRepositoryPath(repository.canonical_path)}
            </p>
          </div>
          <div className="flex flex-wrap gap-2">
            <Button variant="outline" onClick={() => refresh()} disabled={pending}>
              <RefreshCw className={status.isFetching ? "animate-spin" : undefined} aria-hidden />
              Refresh
            </Button>
            {repository.host === "github.com" && repository.owner && repository.repo_name && (
              <Button
                variant="outline"
                onClick={() =>
                  void openUrl(
                    `https://${repository.host}/${repository.owner}/${repository.repo_name}`,
                  )
                }
                title={`Open ${repository.owner}/${repository.repo_name} on ${repository.host}`}
              >
                <ExternalLink aria-hidden />
                Open on GitHub
              </Button>
            )}
            {repository.routing_configured ? (
              <Button onClick={smartSync} disabled={pending}>
                {inspectSync.isPending || pull.isPending || push.isPending ? (
                  <Loader2 className="animate-spin" aria-hidden />
                ) : (
                  <Sparkles aria-hidden />
                )}
                Smart Sync
              </Button>
            ) : (
              <Button onClick={onBack}>
                <Route aria-hidden /> Finish setup
              </Button>
            )}
          </div>
        </div>
        <div className="grid border-t border-white/10 bg-background/10 sm:grid-cols-2 lg:grid-cols-4">
          <HeroMetric icon={GitBranch} label="BRANCH" value={status.data?.branch ?? "No branch"} />
          <HeroMetric
            icon={FileDiff}
            label="CHANGES"
            value={String(status.data?.changes.length ?? 0).padStart(2, "0")}
          />
          <HeroMetric
            icon={GitCommit}
            label="STAGED"
            value={String(stagedCount).padStart(2, "0")}
          />
          <HeroMetric
            icon={KeyRound}
            label="IDENTITY"
            value={repository.assigned_login ? `@${repository.assigned_login}` : "Unassigned"}
          />
        </div>
      </section>

      {!repository.routing_configured && (
        <section className="liquid-panel overflow-hidden rounded-[1rem] border-warning/25">
          <div className="flex flex-col gap-5 p-5 lg:flex-row lg:items-center lg:justify-between">
            <div className="flex min-w-0 items-start gap-3">
              <span className="flex h-10 w-10 shrink-0 items-center justify-center rounded-[0.7rem] border border-warning/25 bg-warning/10 text-warning">
                <AlertCircle className="h-5 w-5" aria-hidden />
              </span>
              <div>
                <p className="eyebrow text-warning">Smart Sync is safely locked</p>
                <h3 className="mt-1 font-display text-lg font-semibold">
                  Finish the identity route first.
                </h3>
                <p className="mt-1 max-w-2xl text-sm leading-6 text-muted-foreground">
                  Shehata Git will not fetch, pull, or push until the remote, account, and
                  credential route are all explicit and verified.
                </p>
              </div>
            </div>
            <Button className="shrink-0" variant="outline" onClick={onBack}>
              <ArrowLeft aria-hidden /> Back to registry
            </Button>
          </div>
          <div className="grid border-t border-white/10 bg-background/15 md:grid-cols-3 md:divide-x md:divide-white/10">
            <ReadinessStep
              label="GitHub remote"
              ready={Boolean(repository.remote_name && repository.remote_url)}
              detail={repository.remote_name ? repository.remote_name : "No remote detected"}
            />
            <ReadinessStep
              label="Identity assigned"
              ready={Boolean(repository.assigned_login)}
              detail={
                repository.assigned_login ? `@${repository.assigned_login}` : "Choose an account"
              }
            />
            <ReadinessStep
              label="Route verified"
              ready={repository.routing_configured}
              detail={
                repository.routing_configured ? "Credential helper ready" : "Connection required"
              }
            />
          </div>
        </section>
      )}

      {accountMismatch && (
        <InlineNotice tone="warning">
          Remote owner is {repository.owner}, while pushes are routed through @
          {repository.assigned_login}. This can be valid for organizations—verify access before
          pushing.
        </InlineNotice>
      )}
      {notice && <InlineNotice tone={notice.tone}>{notice.message}</InlineNotice>}
      {error && <InlineNotice tone="error">{errorMessage(error)}</InlineNotice>}

      <div className="grid gap-4 lg:grid-cols-[19rem_minmax(0,1fr)_20rem]">
        <section className="liquid-panel min-h-[32rem] overflow-hidden rounded-[1rem]">
          <header className="space-y-3 border-b border-white/10 p-4">
            <div className="flex items-center justify-between">
              <div>
                <p className="eyebrow">Working tree</p>
                <h3 className="mt-1 text-sm font-semibold">Changed files</h3>
              </div>
              <span className="font-mono text-xs text-muted-foreground">
                {status.data?.changes.length ?? 0}
              </span>
            </div>
            <SearchField
              value={fileSearch}
              onChange={setFileSearch}
              label="Search changed files"
              placeholder="Search changed files…"
              resultCount={visibleChanges.length}
              className="min-h-11 rounded-[0.7rem]"
            />
            <fieldset className="grid grid-cols-2 gap-1.5">
              <legend className="sr-only">Changed file filters</legend>
              {WORKSPACE_FILE_FILTERS.map((filter) => (
                <button
                  key={filter.value}
                  type="button"
                  onClick={() => setFileFilter(filter.value)}
                  className={cn(
                    "flex min-h-9 items-center justify-between rounded-[0.55rem] border px-2.5 text-[0.68rem] font-semibold transition-colors",
                    fileFilter === filter.value
                      ? "border-primary/35 bg-primary/[0.1] text-primary"
                      : "border-white/[0.07] bg-background/20 text-muted-foreground hover:border-white/15 hover:text-foreground",
                  )}
                  aria-pressed={fileFilter === filter.value}
                >
                  <span>{filter.label}</span>
                  <span className="font-mono text-[0.625rem]">
                    {String(filterCounts[filter.value]).padStart(2, "0")}
                  </span>
                </button>
              ))}
            </fieldset>
            {visibleChanges.length > 0 && (
              <button
                type="button"
                onClick={toggleVisiblePaths}
                className="text-xs font-medium text-muted-foreground transition-colors hover:text-primary"
              >
                {allVisibleSelected ? "Clear visible selection" : "Select visible files"}
              </button>
            )}
          </header>
          <div className="scrollbar-thin max-h-[38rem] overflow-y-auto p-2">
            {status.isLoading && <LoadingState label="Reading changes…" compact />}
            {status.data?.changes.length === 0 && (
              <div className="p-5 text-center">
                <CheckCircle2 className="mx-auto h-6 w-6 text-success" aria-hidden />
                <p className="mt-2 text-sm font-medium">Working tree clean</p>
                <p className="mt-1 text-xs text-muted-foreground">Nothing waiting to commit.</p>
              </div>
            )}
            {changes.length > 0 && visibleChanges.length === 0 && (
              <div className="p-5 text-center">
                <FileCode2 className="mx-auto h-6 w-6 text-muted-foreground/45" aria-hidden />
                <p className="mt-2 text-sm font-medium">No files match this view</p>
                <p className="mt-1 text-xs text-muted-foreground">
                  Clear the search or choose another filter.
                </p>
              </div>
            )}
            {visibleChanges.map((change) => (
              <div
                key={`${change.index_status}${change.worktree_status}:${change.path}`}
                className={cn(
                  "group grid grid-cols-[2rem_minmax(0,1fr)] rounded-[0.7rem] border transition-colors",
                  activePath === change.path
                    ? "border-primary/30 bg-primary/[0.09]"
                    : "border-transparent hover:border-white/10 hover:bg-white/[0.035]",
                )}
              >
                <label className="flex min-h-12 cursor-pointer items-center justify-center">
                  <input
                    type="checkbox"
                    checked={selectedPaths.has(change.path)}
                    onChange={() => togglePath(change.path)}
                    aria-label={`Select ${change.path}`}
                    className="h-4 w-4 accent-primary"
                  />
                </label>
                <button
                  type="button"
                  onClick={() => {
                    setActivePath(change.path);
                    setShowStagedDiff(isStagedChange(change));
                  }}
                  className="min-w-0 py-2.5 pr-3 text-left"
                >
                  <span className="block truncate font-mono text-xs">{change.path}</span>
                  <span
                    className={cn(
                      "mt-1 block text-[0.65rem] font-semibold uppercase tracking-wider",
                      isStagedChange(change) ? "text-success" : "text-warning",
                    )}
                  >
                    {changeLabel(change)} · {change.index_status}
                    {change.worktree_status}
                  </span>
                </button>
              </div>
            ))}
          </div>
          {selectedPaths.size > 0 && (
            <footer className="grid grid-cols-2 gap-2 border-t border-white/10 p-3">
              <Button
                size="sm"
                variant="outline"
                disabled={pending}
                onClick={() => stage.mutate([...selectedPaths])}
              >
                Stage {selectedPaths.size}
              </Button>
              <Button
                size="sm"
                variant="ghost"
                disabled={pending}
                onClick={() => unstage.mutate([...selectedPaths])}
              >
                Unstage
              </Button>
            </footer>
          )}
        </section>

        <section className="liquid-panel min-w-0 overflow-hidden rounded-[1rem]">
          <header className="flex min-h-[4.75rem] flex-col gap-3 border-b border-white/10 p-4 sm:flex-row sm:items-center sm:justify-between">
            <div className="min-w-0">
              <p className="eyebrow">Safe diff preview</p>
              <h3 className="mt-1 truncate font-mono text-sm font-semibold">
                {activePath ?? "Select a changed file"}
              </h3>
            </div>
            {activeChange && (
              <div className="flex rounded-[0.65rem] border border-white/10 bg-background/20 p-1">
                <button
                  type="button"
                  disabled={!canShowWorking}
                  onClick={() => setShowStagedDiff(false)}
                  className={cn(
                    "min-h-9 rounded-[0.45rem] px-3 text-xs font-medium disabled:opacity-35",
                    !showStagedDiff && "bg-white/10 text-foreground",
                  )}
                >
                  Working tree
                </button>
                <button
                  type="button"
                  disabled={!canShowStaged}
                  onClick={() => setShowStagedDiff(true)}
                  className={cn(
                    "min-h-9 rounded-[0.45rem] px-3 text-xs font-medium disabled:opacity-35",
                    showStagedDiff && "bg-white/10 text-foreground",
                  )}
                >
                  Staged
                </button>
              </div>
            )}
          </header>
          <DiffViewer diff={diff.data} loading={diff.isFetching} />
          <div className="border-t border-white/10 p-4">
            <label className="text-sm font-medium" htmlFor="detail-commit-message">
              Commit message
            </label>
            <textarea
              id="detail-commit-message"
              value={message}
              onChange={(event) => setMessage(event.target.value)}
              maxLength={1000}
              rows={3}
              placeholder="feat: describe the change"
              className="glass-input mt-2 w-full resize-none p-3 text-sm"
            />
            <div className="mt-3 flex items-center justify-between gap-3">
              <span className="text-xs text-muted-foreground">
                {stagedCount} staged change{stagedCount === 1 ? "" : "s"}
              </span>
              <Button
                disabled={pending || stagedCount === 0 || message.trim().length === 0}
                onClick={() => commit.mutate()}
              >
                {commit.isPending ? (
                  <Loader2 className="animate-spin" aria-hidden />
                ) : (
                  <GitCommit aria-hidden />
                )}
                Commit staged
              </Button>
            </div>
          </div>
        </section>

        <aside className="space-y-4">
          <section className="liquid-panel rounded-[1rem] p-4">
            <div className="flex items-center gap-2">
              <CloudCog className="h-4 w-4 text-primary" aria-hidden />
              <h3 className="text-sm font-semibold">Remote sync</h3>
            </div>
            <p className="mt-2 text-xs leading-5 text-muted-foreground">
              Smart Sync fetches first, then chooses only a safe fast-forward pull or normal push.
            </p>
            {repository.routing_configured && inspectSync.data && (
              <div className="mt-4 grid grid-cols-2 gap-2">
                <SyncMetric icon={ArrowUp} label="AHEAD" value={inspectSync.data.ahead} />
                <SyncMetric icon={ArrowDown} label="BEHIND" value={inspectSync.data.behind} />
              </div>
            )}
            <div
              className={cn(
                "mt-4 rounded-[0.7rem] border p-3 text-xs leading-5",
                repository.routing_configured
                  ? "border-success/20 bg-success/[0.06] text-success"
                  : "border-warning/20 bg-warning/[0.06] text-warning",
              )}
            >
              {repository.routing_configured
                ? inspectSync.data
                  ? "Inspection complete. Counts reflect the latest fetched remote state."
                  : "Ready. Use Smart Sync above when you want to inspect the remote."
                : "Locked until the remote and identity route are verified."}
            </div>
          </section>

          <section className="liquid-panel rounded-[1rem] p-4">
            <div className="flex items-center gap-2">
              <Route className="h-4 w-4 text-primary" aria-hidden />
              <h3 className="text-sm font-semibold">Identity route</h3>
            </div>
            <dl className="mt-4 space-y-3 text-xs">
              <MetaRow
                label="Account"
                value={repository.assigned_login ? `@${repository.assigned_login}` : "None"}
              />
              <MetaRow label="Remote" value={repository.remote_name ?? "None"} />
              <MetaRow
                label="Repository"
                value={
                  repository.owner && repository.repo_name
                    ? `${repository.owner}/${repository.repo_name}`
                    : "Local only"
                }
              />
              <MetaRow label="Commit author" value={repository.commit_name ?? "Git default"} />
            </dl>
            <label
              className="mt-4 block text-xs text-muted-foreground"
              htmlFor="detail-push-policy"
            >
              Push policy
            </label>
            <select
              id="detail-push-policy"
              value={normalizePushPolicy(repository.push_policy)}
              disabled={pending || !repository.assigned_login}
              onChange={(event) => policy.mutate(event.target.value as PushPolicy)}
              className="glass-input mt-2 h-10 w-full px-3 text-xs font-medium"
            >
              <option value="allow_normal_push">Allow normal push</option>
              <option value="block_ai_push">Block AI push</option>
            </select>
          </section>

          <section className="liquid-panel rounded-[1rem] p-4">
            <div className="flex items-center gap-2">
              <History className="h-4 w-4 text-primary" aria-hidden />
              <h3 className="text-sm font-semibold">Recent activity</h3>
            </div>
            <div className="mt-3 space-y-2">
              {recentActivity.length === 0 && (
                <p className="text-xs text-muted-foreground">No recorded actions yet.</p>
              )}
              {recentActivity.map((event) => (
                <div key={event.id} className="border-l border-white/10 pl-3">
                  <p className="line-clamp-2 text-xs font-medium">{event.summary}</p>
                  <p className="mt-1 font-mono text-[0.625rem] text-muted-foreground">
                    {new Date(event.timestamp).toLocaleString()}
                  </p>
                </div>
              ))}
            </div>
          </section>
        </aside>
      </div>

      {pendingSyncPush && (
        <ConfirmDialog
          eyebrow="Smart Sync / outgoing commits"
          title={`Push ${pendingSyncPush.ahead} local commit${pendingSyncPush.ahead === 1 ? "" : "s"}?`}
          description={
            <>
              Push normally to{" "}
              <strong className="text-foreground">{pendingSyncPush.remote_name}</strong> through{" "}
              <strong className="text-foreground">@{pendingSyncPush.account_login}</strong>.
            </>
          }
          detail="Only committed changes are pushed. Working-tree files stay local, force push is unavailable, and the repository identity route does not change."
          confirmLabel="Push safely"
          cancelLabel="Review first"
          pendingLabel="Pushing…"
          pending={push.isPending}
          tone="primary"
          onCancel={() => setPendingSyncPush(null)}
          onConfirm={() => void confirmSmartPush()}
        />
      )}
    </div>
  );
}

function DiffViewer({
  diff,
  loading,
}: {
  diff:
    | {
        content: string;
        truncated: boolean;
        sensitive: boolean;
        blocked_reason: string | null;
      }
    | undefined;
  loading: boolean;
}) {
  if (loading) return <LoadingState label="Rendering diff…" />;
  if (!diff) {
    return (
      <div className="flex min-h-[24rem] flex-col items-center justify-center p-8 text-center">
        <FileCode2 className="h-7 w-7 text-muted-foreground/50" aria-hidden />
        <p className="mt-3 text-sm font-medium">Choose a file to inspect</p>
        <p className="mt-1 max-w-sm text-xs leading-5 text-muted-foreground">
          The preview is read-only and bounded before it reaches the interface.
        </p>
      </div>
    );
  }
  if (diff.sensitive) {
    return (
      <div className="flex min-h-[24rem] flex-col items-center justify-center p-8 text-center">
        <EyeOff className="h-7 w-7 text-warning" aria-hidden />
        <p className="mt-3 text-sm font-semibold">Preview intentionally hidden</p>
        <p className="mt-2 max-w-md text-xs leading-5 text-muted-foreground">
          {diff.blocked_reason}
        </p>
      </div>
    );
  }
  if (!diff.content) {
    return (
      <div className="flex min-h-[24rem] items-center justify-center p-8 text-sm text-muted-foreground">
        No diff in this view.
      </div>
    );
  }
  let offset = 0;
  let number = 0;
  const lines = diff.content.split("\n").map((content) => {
    number += 1;
    const line = { content, number, key: `${offset}:${content.slice(0, 32)}` };
    offset += content.length + 1;
    return line;
  });

  return (
    <div className="scrollbar-thin max-h-[38rem] min-h-[24rem] overflow-auto bg-background/25 py-3 font-mono text-[0.72rem] leading-5">
      {lines.map((line) => (
        <div
          key={line.key}
          className={cn(
            "grid min-w-max grid-cols-[3.5rem_minmax(40rem,1fr)] px-3",
            line.content.startsWith("+") &&
              !line.content.startsWith("+++") &&
              "bg-success/[0.09] text-success",
            line.content.startsWith("-") &&
              !line.content.startsWith("---") &&
              "bg-destructive/[0.08] text-destructive",
            line.content.startsWith("@@") && "bg-primary/[0.08] text-primary",
            (line.content.startsWith("diff ") || line.content.startsWith("index ")) &&
              "text-muted-foreground",
          )}
        >
          <span className="select-none border-r border-white/5 pr-3 text-right text-muted-foreground/40">
            {line.number}
          </span>
          <span className="whitespace-pre px-3">{line.content || " "}</span>
        </div>
      ))}
      {diff.truncated && (
        <p className="sticky bottom-0 border-t border-warning/20 bg-warning/10 p-3 text-warning">
          Preview truncated at 256 KB. The file itself was not changed.
        </p>
      )}
    </div>
  );
}

function ReadinessStep({
  label,
  ready,
  detail,
}: {
  label: string;
  ready: boolean;
  detail: string;
}) {
  return (
    <div className="flex min-h-20 items-center gap-3 border-b border-white/10 px-5 py-3 last:border-b-0 md:border-b-0">
      <span
        className={cn(
          "flex h-7 w-7 shrink-0 items-center justify-center rounded-full border",
          ready
            ? "border-success/30 bg-success/10 text-success"
            : "border-warning/25 bg-warning/10 text-warning",
        )}
      >
        {ready ? (
          <Check className="h-3.5 w-3.5" aria-hidden />
        ) : (
          <span className="h-1.5 w-1.5 rounded-full bg-current" />
        )}
      </span>
      <div className="min-w-0">
        <p className="data-label">{label}</p>
        <p className="mt-1 truncate text-xs font-medium text-foreground">{detail}</p>
      </div>
    </div>
  );
}

function HeroMetric({
  icon: Icon,
  label,
  value,
}: {
  icon: typeof GitBranch;
  label: string;
  value: string;
}) {
  return (
    <div className="flex min-h-20 items-center gap-3 border-b border-white/10 px-5 py-3 last:border-b-0 sm:border-b-0 sm:border-r sm:last:border-r-0">
      <Icon className="h-4 w-4 text-primary" aria-hidden />
      <div className="min-w-0">
        <p className="data-label">{label}</p>
        <p className="mt-1 truncate text-sm font-semibold">{value}</p>
      </div>
    </div>
  );
}

function SyncMetric({
  icon: Icon,
  label,
  value,
}: {
  icon: typeof ArrowUp;
  label: string;
  value: number;
}) {
  return (
    <div className="rounded-[0.7rem] border border-white/10 bg-background/20 p-3">
      <div className="flex items-center gap-1.5 text-muted-foreground">
        <Icon className="h-3 w-3" aria-hidden />
        <span className="data-label">{label}</span>
      </div>
      <p className="mt-2 font-mono text-xl text-foreground">{String(value).padStart(2, "0")}</p>
    </div>
  );
}

function MetaRow({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex items-start justify-between gap-3">
      <dt className="text-muted-foreground">{label}</dt>
      <dd className="max-w-[12rem] truncate text-right font-medium text-foreground">{value}</dd>
    </div>
  );
}

function InlineNotice({
  tone,
  children,
}: {
  tone: "success" | "warning" | "error";
  children: React.ReactNode;
}) {
  const Icon = tone === "success" ? Check : tone === "warning" ? ShieldCheck : AlertCircle;
  return (
    <div
      role={tone === "error" ? "alert" : "status"}
      className={cn(
        "liquid-panel flex items-start gap-3 rounded-[0.85rem] px-4 py-3 text-sm",
        tone === "success" && "border-success/25 text-success",
        tone === "warning" && "border-warning/25 text-warning",
        tone === "error" && "border-destructive/30 text-destructive",
      )}
    >
      <Icon className="mt-0.5 h-4 w-4 shrink-0" aria-hidden />
      <span>{children}</span>
    </div>
  );
}

function LoadingState({ label, compact = false }: { label: string; compact?: boolean }) {
  return (
    <div
      className={cn(
        "flex items-center justify-center gap-2 text-sm text-muted-foreground",
        compact ? "p-5" : "min-h-[24rem] p-8",
      )}
    >
      <Loader2 className="h-4 w-4 animate-spin" aria-hidden /> {label}
    </div>
  );
}

function changeLabel(change: { index_status: string; worktree_status: string }): string {
  if (change.index_status === "?" && change.worktree_status === "?") return "Untracked";
  if (isStagedChange(change) && change.worktree_status !== " ") return "Staged + changed";
  if (isStagedChange(change)) return "Staged";
  return "Changed";
}

function normalizePushPolicy(value: string): PushPolicy {
  // `ask_before_push` was retired: it described asking but always refused an
  // agent, which is what blocking does. Existing repositories keep that
  // behaviour under the name that is true.
  if (value === "block_ai_push" || value === "ask_before_push") return "block_ai_push";
  return "allow_normal_push";
}

function errorMessage(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (typeof error === "string") return error;
  return "An unknown error occurred.";
}
