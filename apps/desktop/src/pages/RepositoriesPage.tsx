import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { open } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  AlertCircle,
  ArrowRight,
  Check,
  CheckCircle2,
  ChevronDown,
  ExternalLink,
  FileDiff,
  FolderGit2,
  FolderOpen,
  GitBranch,
  GitCommit,
  Globe,
  KeyRound,
  Loader2,
  PlugZap,
  RefreshCw,
  Search,
  ShieldCheck,
  Unplug,
  UserRound,
  X,
} from "lucide-react";
import { useMemo, useState } from "react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/ui/ConfirmDialog";
import { SearchField } from "@/components/ui/SearchField";
import { displayRepositoryPath } from "@/lib/repository-workspace";
import {
  addRepository,
  assignRepository,
  commitRepository,
  getRepositoryStatus,
  linkRepository,
  listAccounts,
  listRepositories,
  type PushPolicy,
  pullRepository,
  pushRepository,
  type RepositoryActionStatus,
  setRepositoryPushPolicy,
  stageRepositoryPaths,
  testRepositoryConnection,
  unlinkRepository,
  unstageRepositoryPaths,
} from "@/lib/tauri";
import type { GhAccount, RepositorySummary } from "@/lib/types";
import { cn } from "@/lib/utils";

export function RepositoriesPage({
  onOpenRepository,
}: {
  onOpenRepository?: (repositoryId: string) => void;
}) {
  const queryClient = useQueryClient();
  const [selectedRepo, setSelectedRepo] = useState<RepositorySummary | null>(null);
  const [guidedRepositoryId, setGuidedRepositoryId] = useState<string | null>(null);
  const [changesRepo, setChangesRepo] = useState<RepositorySummary | null>(null);
  const [repoToUnlink, setRepoToUnlink] = useState<RepositorySummary | null>(null);
  const [assignmentNotice, setAssignmentNotice] = useState<string | null>(null);
  const [search, setSearch] = useState("");
  const repos = useQuery({ queryKey: ["repositories"], queryFn: listRepositories });
  const accounts = useQuery({ queryKey: ["accounts"], queryFn: listAccounts });
  const addRepo = useMutation({
    mutationFn: addRepository,
    onSuccess: async (repository) => {
      await queryClient.invalidateQueries({ queryKey: ["repositories"] });
      setGuidedRepositoryId(repository.id);
      setSelectedRepo(repository);
    },
  });
  const assign = useMutation({
    mutationFn: async (request: Parameters<typeof assignRepository>[0]) => {
      const assignment = await assignRepository(request);
      if (
        guidedRepositoryId === request.repository_id &&
        assignment.repository.remote_protocol === "https"
      ) {
        await linkRepository(request.repository_id);
        const connection = await testRepositoryConnection(request.repository_id);
        return { assignment, connection };
      }
      return { assignment, connection: null };
    },
    onSuccess: async (result) => {
      await queryClient.invalidateQueries({ queryKey: ["repositories"] });
      setAssignmentNotice(
        result.connection
          ? `${result.assignment.repository.display_name} is connected, routed, and verified through @${result.connection.account_login}.`
          : `${result.assignment.repository.display_name} is now locked to @${result.assignment.repository.assigned_login}.`,
      );
      setGuidedRepositoryId(null);
      setSelectedRepo(null);
    },
    onSettled: async () => {
      await queryClient.invalidateQueries({ queryKey: ["repositories"] });
    },
  });
  const link = useMutation({
    mutationFn: linkRepository,
    onSuccess: async (result) => {
      await queryClient.invalidateQueries({ queryKey: ["repositories"] });
      setAssignmentNotice(`Credential routing is active for repository ${result.repository_id}.`);
    },
  });
  const connectionTest = useMutation({
    mutationFn: testRepositoryConnection,
    onSuccess: (result) => {
      setAssignmentNotice(
        `Connection verified through @${result.account_login} on ${result.remote_name}.`,
      );
    },
  });
  const unlink = useMutation({
    mutationFn: (repositoryId: string) => unlinkRepository(repositoryId, false),
    onSuccess: async () => {
      setRepoToUnlink(null);
      await queryClient.invalidateQueries({ queryKey: ["repositories"] });
      setAssignmentNotice("Repository routing was removed and original Git settings restored.");
    },
    onError: () => setRepoToUnlink(null),
  });

  function confirmUnlink(repo: RepositorySummary) {
    setRepoToUnlink(repo);
  }

  async function chooseRepository() {
    addRepo.reset();
    setAssignmentNotice(null);
    const selected = await open({
      directory: true,
      multiple: false,
      title: "Choose a Git repository",
    });
    if (selected) addRepo.mutate(selected);
  }

  const assignedCount = repos.data?.filter((repo) => repo.assigned_login).length ?? 0;
  const filteredRepos = useMemo(() => {
    const needle = search.trim().toLowerCase();
    if (!needle) return repos.data ?? [];
    return (repos.data ?? []).filter((repo) =>
      [
        repo.display_name,
        repo.canonical_path,
        repo.owner,
        repo.repo_name,
        repo.current_branch,
        repo.assigned_login,
      ].some((value) => value?.toLowerCase().includes(needle)),
    );
  }, [repos.data, search]);

  return (
    <div className="mx-auto w-full max-w-6xl space-y-5">
      <section className="instrument-panel overflow-hidden rounded-[0.75rem]">
        <div className="flex flex-col gap-5 p-5 sm:p-6 lg:flex-row lg:items-end lg:justify-between">
          <div>
            <p className="eyebrow">Repository registry / local machine</p>
            <h2 className="mt-2 font-display text-2xl font-semibold tracking-tight">
              Identity routes begin here.
            </h2>
            <p className="mt-2 max-w-2xl text-sm leading-6 text-muted-foreground">
              Inspection is read-only. Git configuration changes only after you review and confirm
              an account assignment.
            </p>
          </div>
          <div className="flex flex-wrap gap-2">
            <Button variant="outline" onClick={() => repos.refetch()} disabled={repos.isFetching}>
              <RefreshCw className={repos.isFetching ? "animate-spin" : undefined} aria-hidden />
              Rescan
            </Button>
            <Button onClick={chooseRepository} disabled={addRepo.isPending}>
              {addRepo.isPending ? (
                <Loader2 className="animate-spin" aria-hidden />
              ) : (
                <FolderOpen aria-hidden />
              )}
              {addRepo.isPending ? "Inspecting…" : "Connect repository"}
            </Button>
          </div>
        </div>
        <div className="grid border-t border-border bg-background/20 sm:grid-cols-3 sm:divide-x sm:divide-border">
          <RegistryMetric label="REGISTERED" value={repos.data?.length ?? 0} />
          <RegistryMetric label="ASSIGNED" value={assignedCount} tone="success" />
          <RegistryMetric
            label="AWAITING ROUTE"
            value={(repos.data?.length ?? 0) - assignedCount}
            tone="warning"
          />
        </div>
      </section>

      {(repos.data?.length ?? 0) > 0 && (
        <div className="liquid-panel rounded-[0.8rem] p-3">
          <SearchField
            value={search}
            onChange={setSearch}
            label="Search repositories"
            placeholder="Search repository, path, branch, or identity…"
            resultCount={filteredRepos.length}
          />
        </div>
      )}

      {assignmentNotice && (
        <div className="flex items-center justify-between gap-4 border border-success/25 bg-success/[0.07] px-4 py-3 text-sm">
          <span className="flex items-center gap-2 text-success">
            <CheckCircle2 className="h-4 w-4" aria-hidden />
            {assignmentNotice}
          </span>
          <button
            type="button"
            className="text-muted-foreground hover:text-foreground"
            onClick={() => setAssignmentNotice(null)}
            aria-label="Dismiss message"
          >
            <X className="h-4 w-4" aria-hidden />
          </button>
        </div>
      )}

      {(repos.isError ||
        addRepo.isError ||
        link.isError ||
        connectionTest.isError ||
        unlink.isError) && (
        <div className="flex gap-3 border border-destructive/35 bg-destructive/[0.06] p-4">
          <AlertCircle className="mt-0.5 h-5 w-5 shrink-0 text-destructive" aria-hidden />
          <div>
            <p className="text-sm font-semibold text-destructive">Repository inspection failed</p>
            <p className="mt-1 text-sm text-muted-foreground">
              {errorMessage(
                addRepo.error ?? repos.error ?? link.error ?? connectionTest.error ?? unlink.error,
              )}
            </p>
          </div>
        </div>
      )}

      {repos.isLoading && (
        <div className="flex items-center gap-2 py-8 text-sm text-muted-foreground">
          <Loader2 className="h-4 w-4 animate-spin" aria-hidden /> Reading repository registry…
        </div>
      )}

      {repos.data?.length === 0 && (
        <section className="relative flex min-h-72 flex-col items-center justify-center overflow-hidden border border-dashed border-border bg-card/45 p-8 text-center">
          <span className="absolute left-0 top-0 h-5 w-5 border-l border-t border-primary/50" />
          <span className="absolute right-0 top-0 h-5 w-5 border-r border-t border-primary/50" />
          <span className="absolute bottom-0 left-0 h-5 w-5 border-b border-l border-primary/50" />
          <span className="absolute bottom-0 right-0 h-5 w-5 border-b border-r border-primary/50" />
          <div className="flex h-14 w-14 items-center justify-center border border-border bg-background/50">
            <FolderGit2 className="h-6 w-6 text-primary" aria-hidden />
          </div>
          <p className="eyebrow mt-5">Registry empty</p>
          <h3 className="mt-2 font-display text-xl font-semibold">Connect your first repository</h3>
          <p className="mt-2 max-w-md text-sm leading-6 text-muted-foreground">
            Select a project folder. Shehata Git reads its Git metadata and stores no source files.
          </p>
          <Button className="mt-6" onClick={chooseRepository} disabled={addRepo.isPending}>
            <FolderOpen aria-hidden /> Connect repository
          </Button>
        </section>
      )}

      {!repos.isLoading && (repos.data?.length ?? 0) > 0 && filteredRepos.length === 0 && (
        <section className="instrument-panel flex min-h-40 flex-col items-center justify-center rounded-[0.8rem] p-6 text-center">
          <FolderGit2 className="h-7 w-7 text-muted-foreground/45" aria-hidden />
          <p className="mt-3 font-display font-semibold">No matching repositories</p>
          <p className="mt-1 text-sm text-muted-foreground">
            Try a different name, path, branch, or identity.
          </p>
        </section>
      )}

      <div className="space-y-3">
        {filteredRepos.map((repo, index) => (
          <RepositoryRow
            key={repo.id}
            repo={repo}
            index={index}
            onAssign={() => {
              assign.reset();
              setGuidedRepositoryId(null);
              setSelectedRepo(repo);
            }}
            onLink={() => link.mutate(repo.id)}
            onTest={() => connectionTest.mutate(repo.id)}
            onUnlink={() => confirmUnlink(repo)}
            onActions={() => setChangesRepo(repo)}
            onOpen={() => onOpenRepository?.(repo.id)}
            pending={
              (link.isPending && link.variables === repo.id) ||
              (connectionTest.isPending && connectionTest.variables === repo.id) ||
              (unlink.isPending && unlink.variables === repo.id)
            }
          />
        ))}
      </div>

      {selectedRepo && (
        <AssignmentDialog
          repo={selectedRepo}
          accounts={accounts.data ?? []}
          guided={guidedRepositoryId === selectedRepo.id}
          pending={assign.isPending}
          error={assign.isError ? errorMessage(assign.error) : null}
          onClose={() => setSelectedRepo(null)}
          onSubmit={(account, commitName, commitEmail) =>
            assign.mutate({
              repository_id: selectedRepo.id,
              host: account.host,
              login: account.login,
              commit_name: commitName.trim() || null,
              commit_email: commitEmail.trim() || null,
            })
          }
        />
      )}

      {changesRepo && <GitActionsDialog repo={changesRepo} onClose={() => setChangesRepo(null)} />}

      {repoToUnlink && (
        <ConfirmDialog
          eyebrow="Restore repository routing"
          title={`Unlink ${repoToUnlink.display_name}?`}
          description="Remove Shehata Git credential routing from this repository and restore the Git settings that existed before connection."
          detail="Repository files and commits are untouched. The local commit author is kept, and the GitHub account stays signed in on this PC."
          confirmLabel="Unlink and restore"
          cancelLabel="Keep route"
          pendingLabel="Restoring…"
          pending={unlink.isPending}
          onCancel={() => setRepoToUnlink(null)}
          onConfirm={() => unlink.mutate(repoToUnlink.id)}
        />
      )}
    </div>
  );
}

function RegistryMetric({
  label,
  value,
  tone,
}: {
  label: string;
  value: number;
  tone?: "success" | "warning";
}) {
  return (
    <div className="flex items-center justify-between border-b border-border px-5 py-3 last:border-b-0 sm:border-b-0">
      <span className="data-label">{label}</span>
      <span
        className={cn(
          "font-mono text-lg tabular-nums",
          tone === "success" && "text-success",
          tone === "warning" && "text-warning",
        )}
      >
        {String(value).padStart(2, "0")}
      </span>
    </div>
  );
}

function RepositoryRow({
  repo,
  index,
  onAssign,
  onLink,
  onTest,
  onUnlink,
  onActions,
  onOpen,
  pending,
}: {
  repo: RepositorySummary;
  index: number;
  onAssign: () => void;
  onLink: () => void;
  onTest: () => void;
  onUnlink: () => void;
  onActions: () => void;
  onOpen: () => void;
  pending: boolean;
}) {
  const [expanded, setExpanded] = useState(false);

  return (
    <article className="instrument-panel group relative overflow-hidden rounded-[0.7rem] transition-colors hover:border-muted-foreground/35">
      <span
        className={cn(
          "absolute inset-y-0 left-0 w-0.5",
          repo.assigned_login ? "bg-success" : "bg-warning",
        )}
      />
      <button
        type="button"
        className="flex min-h-[5rem] w-full items-center gap-4 p-4 text-left sm:p-5"
        onClick={() => setExpanded((value) => !value)}
        aria-expanded={expanded}
        aria-controls={`repository-${repo.id}`}
      >
        <div className="flex min-w-0 flex-1 items-center gap-3 sm:gap-4">
          <span className="font-mono text-[0.65rem] text-muted-foreground/50">
            {String(index + 1).padStart(2, "0")}
          </span>
          <div className="flex h-10 w-10 shrink-0 items-center justify-center rounded-[0.55rem] border border-border bg-background/35">
            <FolderGit2 className="h-4 w-4 text-primary" aria-hidden />
          </div>
          <div className="min-w-0 flex-1">
            <div className="flex min-w-0 flex-wrap items-center gap-2">
              <h3 className="min-w-0 font-display font-semibold leading-snug tracking-tight">
                {repo.display_name}
              </h3>
              {repo.remote_protocol && (
                <Badge variant={repo.remote_protocol === "https" ? "secondary" : "warning"}>
                  {repo.remote_protocol.toUpperCase()}
                </Badge>
              )}
            </div>
            <p className="mt-1 truncate font-mono text-[0.7rem] text-muted-foreground">
              {displayRepositoryPath(repo.canonical_path)}
            </p>
          </div>
        </div>
        <div className="hidden min-w-0 items-center justify-end gap-3 md:flex">
          <span className="max-w-44 truncate font-mono text-[0.7rem] text-muted-foreground">
            {repo.current_branch ?? "No commits"}
          </span>
          <Badge variant={repo.routing_configured ? "success" : "warning"}>
            {repo.assigned_login ? `@${repo.assigned_login}` : "unassigned"}
          </Badge>
        </div>
        <span className="ml-1 flex h-9 w-9 shrink-0 items-center justify-center rounded-full border border-border bg-background/25 text-muted-foreground transition-colors group-hover:text-foreground">
          <ChevronDown
            className={cn("h-4 w-4 transition-transform duration-200", expanded && "rotate-180")}
            aria-hidden
          />
        </span>
      </button>

      {expanded && (
        <div
          id={`repository-${repo.id}`}
          className="animate-fade-in border-t border-border/80 bg-background/15"
        >
          <div className="grid gap-3 p-4 sm:grid-cols-2 sm:p-5 lg:grid-cols-3">
            <div className="rounded-[0.6rem] border border-border/70 bg-background/25 p-3">
              <p className="data-label">CURRENT BRANCH</p>
              <p className="mt-2 flex min-w-0 items-center gap-2 text-sm text-muted-foreground">
                <GitBranch className="h-3.5 w-3.5 shrink-0" aria-hidden />
                <span className="truncate">{repo.current_branch ?? "No commits"}</span>
              </p>
            </div>
            <div className="rounded-[0.6rem] border border-border/70 bg-background/25 p-3">
              <p className="data-label">REMOTE</p>
              <p className="mt-2 flex min-w-0 items-center gap-2 text-sm text-muted-foreground">
                <Globe className="h-3.5 w-3.5 shrink-0" aria-hidden />
                {repo.host === "github.com" && repo.owner && repo.repo_name ? (
                  <button
                    type="button"
                    onClick={(event) => {
                      // The whole card opens the repository in the app; this
                      // one control means the remote instead.
                      event.stopPropagation();
                      void openUrl(`https://${repo.host}/${repo.owner}/${repo.repo_name}`);
                    }}
                    title={`Open ${repo.owner}/${repo.repo_name} on ${repo.host}`}
                    className="group flex min-w-0 items-center gap-1.5 rounded-[0.3rem] text-left transition hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary/40"
                  >
                    <span className="truncate">{`${repo.owner}/${repo.repo_name}`}</span>
                    <ExternalLink
                      className="h-3 w-3 shrink-0 opacity-50 transition group-hover:opacity-100"
                      aria-hidden
                    />
                  </button>
                ) : (
                  <span className="truncate">Remote unavailable</span>
                )}
              </p>
            </div>
            <div className="rounded-[0.6rem] border border-border/70 bg-background/25 p-3 sm:col-span-2 lg:col-span-1">
              <p className="data-label">
                {repo.routing_configured ? "ROUTE ACTIVE" : "ROUTING STATE"}
              </p>
              <p
                className={cn(
                  "mt-2 text-sm font-semibold",
                  repo.routing_configured ? "text-success" : "text-warning",
                )}
              >
                {repo.assigned_login ? `@${repo.assigned_login}` : "Unassigned"}
              </p>
            </div>
          </div>
          <div className="grid grid-cols-2 gap-2 border-t border-border/70 px-4 py-3.5 sm:flex sm:flex-wrap sm:justify-end sm:px-5">
            <Button
              size="sm"
              variant="ghost"
              className="min-h-11 sm:min-h-9"
              onClick={onOpen}
              disabled={pending}
            >
              Open workspace <ArrowRight aria-hidden />
            </Button>
            {repo.assigned_login && (
              <Button
                size="sm"
                variant="outline"
                className="min-h-11 sm:min-h-9"
                onClick={onActions}
                disabled={pending}
              >
                <FileDiff aria-hidden /> Changes
              </Button>
            )}
            {repo.assigned_login &&
              repo.remote_protocol === "https" &&
              (!repo.routing_configured ? (
                <Button
                  size="sm"
                  className="min-h-11 sm:min-h-9"
                  onClick={onLink}
                  disabled={pending}
                >
                  {pending ? (
                    <Loader2 className="animate-spin" aria-hidden />
                  ) : (
                    <PlugZap aria-hidden />
                  )}
                  Enable route
                </Button>
              ) : (
                <>
                  <Button
                    size="sm"
                    variant="outline"
                    className="min-h-11 sm:min-h-9"
                    onClick={onTest}
                    disabled={pending}
                  >
                    {pending ? (
                      <Loader2 className="animate-spin" aria-hidden />
                    ) : (
                      <ShieldCheck aria-hidden />
                    )}
                    Verify
                  </Button>
                  <Button
                    size="sm"
                    variant="ghost"
                    className="min-h-11 sm:min-h-9"
                    onClick={onUnlink}
                    disabled={pending}
                  >
                    <Unplug aria-hidden /> Unlink
                  </Button>
                </>
              ))}
            <Button
              size="sm"
              variant={repo.assigned_login ? "outline" : "default"}
              className="min-h-11 sm:min-h-9"
              onClick={onAssign}
              disabled={pending}
            >
              <KeyRound aria-hidden />
              {repo.assigned_login ? "Edit" : "Assign identity"}
              {!repo.assigned_login && <ArrowRight aria-hidden />}
            </Button>
          </div>
        </div>
      )}
    </article>
  );
}

function GitActionsDialog({ repo, onClose }: { repo: RepositorySummary; onClose: () => void }) {
  const queryClient = useQueryClient();
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [message, setMessage] = useState("");
  const [networkNotice, setNetworkNotice] = useState<string | null>(null);
  const [confirmingPush, setConfirmingPush] = useState(false);
  const [pushPolicy, setPushPolicy] = useState<PushPolicy>(normalizePushPolicy(repo.push_policy));
  const status = useQuery({
    queryKey: ["repository-status", repo.id],
    queryFn: () => getRepositoryStatus(repo.id),
  });
  const refresh = async () => {
    setSelected(new Set());
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: ["repository-status", repo.id] }),
      queryClient.invalidateQueries({ queryKey: ["repositories"] }),
      queryClient.invalidateQueries({ queryKey: ["audit"] }),
    ]);
  };
  const stage = useMutation({
    mutationFn: (paths: string[]) => stageRepositoryPaths(repo.id, paths),
    onSuccess: refresh,
  });
  const unstage = useMutation({
    mutationFn: (paths: string[]) => unstageRepositoryPaths(repo.id, paths),
    onSuccess: refresh,
  });
  const commit = useMutation({
    mutationFn: () => commitRepository(repo.id, message),
    onSuccess: async () => {
      setMessage("");
      await refresh();
    },
  });
  const pull = useMutation({
    mutationFn: () => pullRepository(repo.id),
    onSuccess: async (result) => {
      setNetworkNotice(
        `Fast-forward pull completed on ${result.branch} through @${result.account_login}.`,
      );
      await refresh();
    },
  });
  const push = useMutation({
    mutationFn: () => pushRepository(repo.id),
    onSuccess: async (result) => {
      setConfirmingPush(false);
      setNetworkNotice(
        `Normal push completed to ${result.remote_name}/${result.branch} through @${result.account_login}.`,
      );
      await refresh();
    },
    onError: () => setConfirmingPush(false),
  });
  const policy = useMutation({
    mutationFn: (value: PushPolicy) => setRepositoryPushPolicy(repo.id, value),
    onSuccess: async (result) => {
      setPushPolicy(result.push_policy);
      setNetworkNotice("Push policy updated for this repository.");
      await queryClient.invalidateQueries({ queryKey: ["repositories"] });
    },
  });
  const pending =
    stage.isPending ||
    unstage.isPending ||
    commit.isPending ||
    pull.isPending ||
    push.isPending ||
    policy.isPending;
  const error =
    stage.error ??
    unstage.error ??
    commit.error ??
    pull.error ??
    push.error ??
    policy.error ??
    status.error;
  const selectedPaths = [...selected];
  const stagedCount = status.data?.changes.filter(isStaged).length ?? 0;

  function toggle(path: string) {
    setSelected((current) => {
      const next = new Set(current);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  }

  function confirmPush() {
    setNetworkNotice(null);
    setConfirmingPush(true);
  }

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center overflow-y-auto bg-background/85 p-4 backdrop-blur-sm">
      <section
        role="dialog"
        aria-modal="true"
        aria-labelledby="changes-title"
        className="instrument-panel flex max-h-[calc(100vh-2rem)] w-full max-w-3xl flex-col overflow-hidden rounded-[0.8rem]"
      >
        <header className="flex items-start justify-between gap-5 border-b border-border p-5 sm:p-6">
          <div className="flex gap-4">
            <div className="flex h-11 w-11 shrink-0 items-center justify-center border border-primary/30 bg-primary/[0.08]">
              <FileDiff className="h-5 w-5 text-primary" aria-hidden />
            </div>
            <div>
              <p className="eyebrow">Guarded local Git actions</p>
              <h2 id="changes-title" className="mt-1 font-display text-xl font-semibold">
                Changes in {repo.display_name}
              </h2>
              <p className="mt-1 font-mono text-xs text-muted-foreground">
                {status.data?.detached_head
                  ? "DETACHED HEAD"
                  : (status.data?.branch ?? "No branch")}
              </p>
            </div>
          </div>
          <button
            type="button"
            className="flex h-10 w-10 items-center justify-center border border-transparent text-muted-foreground hover:border-border hover:text-foreground"
            onClick={onClose}
            disabled={pending}
            aria-label="Close changes"
          >
            <X className="h-4 w-4" aria-hidden />
          </button>
        </header>

        <div className="min-h-0 flex-1 overflow-y-auto p-5 sm:p-6">
          {status.isLoading && (
            <p className="flex items-center gap-2 text-sm text-muted-foreground">
              <Loader2 className="h-4 w-4 animate-spin" aria-hidden /> Reading changes…
            </p>
          )}
          {status.data?.changes.length === 0 && (
            <div className="border border-success/25 bg-success/[0.05] p-5 text-sm text-success">
              Working tree is clean.
            </div>
          )}
          <div className="space-y-2">
            {status.data?.changes.map((change) => (
              <label
                key={`${change.index_status}${change.worktree_status}:${change.path}`}
                className="flex cursor-pointer items-center gap-3 border border-border bg-background/25 p-3 hover:border-muted-foreground/40"
              >
                <input
                  type="checkbox"
                  checked={selected.has(change.path)}
                  onChange={() => toggle(change.path)}
                  className="h-4 w-4 accent-primary"
                />
                <span
                  className={cn(
                    "w-16 shrink-0 font-mono text-[0.65rem] font-semibold",
                    isStaged(change) ? "text-success" : "text-warning",
                  )}
                >
                  {isStaged(change) ? "STAGED" : "CHANGED"}
                </span>
                <span className="min-w-0 flex-1 truncate font-mono text-xs">{change.path}</span>
                <span className="font-mono text-[0.65rem] text-muted-foreground">
                  {change.index_status}
                  {change.worktree_status}
                </span>
              </label>
            ))}
          </div>

          {error && (
            <div className="mt-4 flex gap-3 border border-destructive/30 bg-destructive/[0.06] p-3">
              <AlertCircle className="mt-0.5 h-4 w-4 shrink-0 text-destructive" aria-hidden />
              <p className="text-sm text-destructive">{errorMessage(error)}</p>
            </div>
          )}

          {networkNotice && (
            <div className="mt-4 flex gap-3 border border-success/25 bg-success/[0.06] p-3">
              <CheckCircle2 className="mt-0.5 h-4 w-4 shrink-0 text-success" aria-hidden />
              <p className="text-sm text-success">{networkNotice}</p>
            </div>
          )}

          <div className="mt-6 border-t border-border pt-5">
            <label className="space-y-2 text-sm font-medium">
              <span>Commit message</span>
              <textarea
                value={message}
                onChange={(event) => setMessage(event.target.value)}
                maxLength={1000}
                rows={3}
                placeholder="feat: describe the change"
                className="w-full resize-none rounded-[0.5rem] border border-input bg-background/45 p-3 text-sm outline-none placeholder:text-muted-foreground/50 focus:border-primary focus:ring-2 focus:ring-primary/15"
              />
            </label>
          </div>

          <div className="mt-6 flex flex-col gap-4 border-t border-border pt-5 sm:flex-row sm:items-center sm:justify-between">
            <div>
              <p className="data-label">REMOTE SYNC / SAFE MODE</p>
              <p className="mt-1 text-xs leading-5 text-muted-foreground">
                Pull is fast-forward only. Push runs full preflight and never uses force.
              </p>
            </div>
            <div className="flex gap-2">
              <Button
                size="sm"
                variant="outline"
                disabled={
                  pending || !repo.routing_configured || !status.data || status.data.detached_head
                }
                onClick={() => {
                  setNetworkNotice(null);
                  pull.mutate();
                }}
              >
                {pull.isPending ? (
                  <Loader2 className="animate-spin" aria-hidden />
                ) : (
                  <RefreshCw aria-hidden />
                )}
                Pull FF-only
              </Button>
              <Button
                size="sm"
                disabled={
                  pending || !repo.routing_configured || !status.data || status.data.detached_head
                }
                onClick={confirmPush}
              >
                {push.isPending ? (
                  <Loader2 className="animate-spin" aria-hidden />
                ) : (
                  <ShieldCheck aria-hidden />
                )}
                Normal push
              </Button>
            </div>
          </div>

          <label className="mt-4 flex flex-col gap-2 text-xs sm:flex-row sm:items-center sm:justify-between">
            <span className="text-muted-foreground">Push policy for this repository</span>
            <select
              value={pushPolicy}
              disabled={pending}
              onChange={(event) => policy.mutate(event.target.value as PushPolicy)}
              className="h-9 rounded-[0.45rem] border border-input bg-background/60 px-3 text-xs font-medium outline-none focus:border-primary"
            >
              <option value="allow_normal_push">Allow normal push</option>
              <option value="block_ai_push">Block AI push</option>
            </select>
          </label>

          {!repo.routing_configured && (
            <p className="mt-3 text-xs text-warning">
              Enable credential routing before using remote sync.
            </p>
          )}
        </div>

        <footer className="flex flex-col gap-3 border-t border-border bg-background/20 p-5 sm:flex-row sm:items-center sm:justify-between sm:px-6">
          <div className="flex flex-wrap gap-2">
            <Button
              size="sm"
              variant="outline"
              disabled={pending || selectedPaths.length === 0}
              onClick={() => stage.mutate(selectedPaths)}
            >
              Stage selected
            </Button>
            <Button
              size="sm"
              variant="ghost"
              disabled={pending || selectedPaths.length === 0}
              onClick={() => unstage.mutate(selectedPaths)}
            >
              Unstage selected
            </Button>
          </div>
          <Button
            disabled={pending || stagedCount === 0 || message.trim().length === 0}
            onClick={() => commit.mutate()}
          >
            {commit.isPending ? (
              <Loader2 className="animate-spin" aria-hidden />
            ) : (
              <GitCommit aria-hidden />
            )}
            {commit.isPending
              ? "Committing…"
              : `Commit ${stagedCount} change${stagedCount === 1 ? "" : "s"}`}
          </Button>
        </footer>
      </section>

      {confirmingPush && (
        <ConfirmDialog
          eyebrow="Guarded normal push"
          title={`Push ${repo.display_name}?`}
          description={
            <>
              Push committed changes normally through{" "}
              <strong className="text-foreground">@{repo.assigned_login}</strong>.
            </>
          }
          detail="Uncommitted files remain local. Force push is unavailable, and the repository identity route does not change."
          confirmLabel="Push safely"
          cancelLabel="Review first"
          pendingLabel="Pushing…"
          pending={push.isPending}
          tone="primary"
          onCancel={() => setConfirmingPush(false)}
          onConfirm={() => push.mutate()}
        />
      )}
    </div>
  );
}

function isStaged(change: RepositoryActionStatus["changes"][number]): boolean {
  return change.index_status !== " " && change.index_status !== "?";
}

function normalizePushPolicy(value: string): PushPolicy {
  // `ask_before_push` was retired: it described asking but always refused an
  // agent, which is what blocking does. Existing repositories keep that
  // behaviour under the name that is true.
  if (value === "block_ai_push" || value === "ask_before_push") return "block_ai_push";
  return "allow_normal_push";
}

function AssignmentDialog({
  repo,
  accounts,
  guided,
  pending,
  error,
  onClose,
  onSubmit,
}: {
  repo: RepositorySummary;
  accounts: GhAccount[];
  guided: boolean;
  pending: boolean;
  error: string | null;
  onClose: () => void;
  onSubmit: (account: GhAccount, commitName: string, commitEmail: string) => void;
}) {
  const available = accounts.filter(
    (account) => account.token_available && (!repo.host || account.host === repo.host),
  );
  const recommended = available.find(
    (account) => account.login.toLowerCase() === repo.owner?.toLowerCase(),
  );
  const initial =
    available.find((account) => account.login === repo.assigned_login) ??
    recommended ??
    available[0];
  const [selectedKey, setSelectedKey] = useState(initial ? `${initial.host}:${initial.login}` : "");
  const [commitName, setCommitName] = useState(repo.commit_name ?? "");
  const [commitEmail, setCommitEmail] = useState(repo.commit_email ?? "");
  const selected = available.find((account) => `${account.host}:${account.login}` === selectedKey);

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-background/85 p-4 backdrop-blur-sm">
      <section
        role="dialog"
        aria-modal="true"
        aria-labelledby="assignment-title"
        className="instrument-panel my-auto w-full max-w-2xl overflow-visible rounded-[0.8rem]"
      >
        <header className="flex items-start justify-between gap-5 border-b border-border p-5 sm:p-6">
          <div className="flex gap-4">
            <div className="flex h-11 w-11 shrink-0 items-center justify-center border border-primary/30 bg-primary/[0.08]">
              <ShieldCheck className="h-5 w-5 text-primary" aria-hidden />
            </div>
            <div>
              <p className="eyebrow">{guided ? "Guided connection" : "Repository assignment"}</p>
              <h2 id="assignment-title" className="mt-1 font-display text-xl font-semibold">
                {guided ? "Connect" : "Lock identity for"} {repo.display_name}
              </h2>
              <p className="mt-1 text-sm text-muted-foreground">
                Remote: {repo.host ?? "unknown"}/{repo.owner ?? "—"}/{repo.repo_name ?? "—"}
              </p>
            </div>
          </div>
          <button
            type="button"
            className="flex h-10 w-10 items-center justify-center border border-transparent text-muted-foreground hover:border-border hover:text-foreground"
            onClick={onClose}
            disabled={pending}
            aria-label="Close assignment"
          >
            <X className="h-4 w-4" aria-hidden />
          </button>
        </header>

        <form
          onSubmit={(event) => {
            event.preventDefault();
            if (selected) onSubmit(selected, commitName, commitEmail);
          }}
        >
          <div className="space-y-6 p-5 sm:p-6">
            <fieldset>
              <legend className="data-label">01 / GitHub identity</legend>
              {available.length === 0 ? (
                <div className="mt-3 flex gap-3 border border-warning/30 bg-warning/[0.06] p-4">
                  <AlertCircle className="mt-0.5 h-4 w-4 shrink-0 text-warning" aria-hidden />
                  <div>
                    <p className="text-sm font-semibold">
                      No usable account for {repo.host ?? "this remote"}
                    </p>
                    <p className="mt-1 text-xs leading-5 text-muted-foreground">
                      Add or refresh a GitHub account first. Token values never enter this dialog.
                    </p>
                  </div>
                </div>
              ) : (
                <AccountPicker
                  accounts={available}
                  selectedKey={selectedKey}
                  recommendedKey={
                    recommended ? `${recommended.host}:${recommended.login}` : undefined
                  }
                  onSelect={setSelectedKey}
                />
              )}
            </fieldset>

            <fieldset className="border-t border-border pt-6">
              <legend className="data-label">02 / Local commit author</legend>
              <p className="mt-2 text-xs leading-5 text-muted-foreground">
                Optional. These values are written to this repository only. Existing values are
                backed up before they change.
              </p>
              {(repo.inherited_commit_name || repo.inherited_commit_email) && (
                <div className="mt-3 flex gap-3 rounded-[0.5rem] border border-warning/25 bg-warning/[0.05] p-3 text-xs leading-5 text-muted-foreground">
                  <AlertCircle className="mt-0.5 h-4 w-4 shrink-0 text-warning" aria-hidden />
                  <span>
                    This repository sets no author of its own, so commits would be made as{" "}
                    <strong className="text-foreground">
                      {repo.inherited_commit_name ?? "an unnamed author"}
                    </strong>
                    {repo.inherited_commit_email ? (
                      <>
                        {" "}
                        &lt;
                        <strong className="text-foreground">{repo.inherited_commit_email}</strong>
                        &gt;
                      </>
                    ) : null}
                    , inherited from your global Git configuration. Enter the author this repository
                    should use.
                  </span>
                </div>
              )}
              <div className="mt-4 grid gap-4 sm:grid-cols-2">
                <label className="space-y-2 text-sm font-medium">
                  <span>Author name</span>
                  <input
                    value={commitName}
                    onChange={(event) => setCommitName(event.target.value)}
                    maxLength={128}
                    placeholder="e.g. Ada Lovelace"
                    className="h-11 w-full rounded-[0.5rem] border border-input bg-background/45 px-3 text-sm outline-none transition-colors placeholder:text-muted-foreground/50 focus:border-primary focus:ring-2 focus:ring-primary/15"
                  />
                </label>
                <label className="space-y-2 text-sm font-medium">
                  <span>Author email</span>
                  <input
                    type="email"
                    value={commitEmail}
                    onChange={(event) => setCommitEmail(event.target.value)}
                    maxLength={254}
                    placeholder="name@example.com"
                    className="h-11 w-full rounded-[0.5rem] border border-input bg-background/45 px-3 text-sm outline-none transition-colors placeholder:text-muted-foreground/50 focus:border-primary focus:ring-2 focus:ring-primary/15"
                  />
                </label>
              </div>
            </fieldset>

            {repo.remote_protocol === "ssh" && (
              <div className="flex gap-3 border border-warning/25 bg-warning/[0.05] p-3 text-xs leading-5 text-muted-foreground">
                <AlertCircle className="mt-0.5 h-4 w-4 shrink-0 text-warning" aria-hidden />
                Account assignment will be saved, but automatic credential routing needs an HTTPS
                remote during connection setup.
              </div>
            )}

            {guided && repo.remote_protocol === "https" && (
              <div className="flex gap-3 border border-primary/25 bg-primary/[0.06] p-3 text-xs leading-5 text-muted-foreground">
                <PlugZap className="mt-0.5 h-4 w-4 shrink-0 text-primary" aria-hidden />
                One confirmation assigns the identity, enables credential routing, and verifies the
                remote connection. Your token never enters the app.
              </div>
            )}

            {error && (
              <div className="flex gap-3 border border-destructive/30 bg-destructive/[0.06] p-3">
                <AlertCircle className="mt-0.5 h-4 w-4 shrink-0 text-destructive" aria-hidden />
                <p className="text-sm text-destructive">{error}</p>
              </div>
            )}
          </div>

          <footer className="flex flex-col-reverse gap-3 border-t border-border bg-background/20 p-5 sm:flex-row sm:items-center sm:justify-between sm:px-6">
            <p className="flex items-center gap-2 text-xs text-muted-foreground">
              <KeyRound className="h-3.5 w-3.5" aria-hidden />
              Original Git identity remains restorable
            </p>
            <div className="flex gap-2">
              <Button type="button" variant="ghost" onClick={onClose} disabled={pending}>
                Cancel
              </Button>
              <Button type="submit" disabled={!selected || pending}>
                {pending ? (
                  <Loader2 className="animate-spin" aria-hidden />
                ) : (
                  <ShieldCheck aria-hidden />
                )}
                {pending ? "Connecting…" : guided ? "Connect and verify" : "Confirm assignment"}
              </Button>
            </div>
          </footer>
        </form>
      </section>
    </div>
  );
}

function AccountPicker({
  accounts,
  selectedKey,
  recommendedKey,
  onSelect,
}: {
  accounts: GhAccount[];
  selectedKey: string;
  recommendedKey: string | undefined;
  onSelect: (key: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const [search, setSearch] = useState("");
  const selected = accounts.find((account) => `${account.host}:${account.login}` === selectedKey);
  const filtered = useMemo(() => {
    const needle = search.trim().toLowerCase();
    if (!needle) return accounts;
    return accounts.filter(
      (account) =>
        account.login.toLowerCase().includes(needle) || account.host.toLowerCase().includes(needle),
    );
  }, [accounts, search]);

  return (
    <div className="relative mt-3">
      <button
        type="button"
        className={cn(
          "flex min-h-[4.5rem] w-full items-center gap-3 rounded-[0.75rem] border bg-background/30 p-3 text-left transition-all",
          open
            ? "border-primary/45 shadow-[0_0_0_3px_hsl(var(--primary)/0.08)]"
            : "border-border hover:border-muted-foreground/45",
        )}
        onClick={() => setOpen((value) => !value)}
        aria-expanded={open}
        aria-haspopup="listbox"
      >
        <span className="flex h-10 w-10 shrink-0 items-center justify-center rounded-[0.6rem] border border-primary/20 bg-primary/[0.07]">
          <UserRound className="h-4 w-4 text-primary" aria-hidden />
        </span>
        <span className="min-w-0 flex-1">
          <span className="data-label block">SELECTED IDENTITY</span>
          <span className="mt-1 block truncate text-sm font-semibold">
            {selected ? `@${selected.login}` : "Choose a GitHub account"}
          </span>
          {selected && (
            <span className="mt-0.5 block truncate font-mono text-[0.65rem] text-muted-foreground">
              {selected.host}
              {`${selected.host}:${selected.login}` === recommendedKey ? " · recommended" : ""}
            </span>
          )}
        </span>
        <ChevronDown
          className={cn(
            "h-4 w-4 shrink-0 text-muted-foreground transition-transform",
            open && "rotate-180",
          )}
          aria-hidden
        />
      </button>

      {open && (
        <div className="absolute left-0 right-0 top-[calc(100%+0.5rem)] z-40 overflow-hidden rounded-[0.8rem] border border-white/10 bg-surface-elevated/95 shadow-[0_24px_70px_rgba(0,0,0,0.55)] backdrop-blur-2xl">
          <div className="border-b border-white/10 p-2.5">
            <label className="glass-input flex min-h-11 items-center gap-2.5 px-3 focus-within:border-primary/45">
              <Search className="h-4 w-4 shrink-0 text-muted-foreground" aria-hidden />
              <span className="sr-only">Search GitHub accounts</span>
              <input
                type="search"
                value={search}
                onChange={(event) => setSearch(event.target.value)}
                placeholder="Search GitHub accounts…"
                className="min-w-0 flex-1 bg-transparent text-sm outline-none placeholder:text-muted-foreground/55 focus-visible:ring-0 focus-visible:ring-offset-0 [&::-webkit-search-cancel-button]:hidden"
              />
              <span className="font-mono text-[0.65rem] text-muted-foreground/60">
                {filtered.length}/{accounts.length}
              </span>
            </label>
          </div>
          <div
            className="scrollbar-thin max-h-60 overflow-y-auto p-2"
            role="listbox"
            aria-label="GitHub identities"
          >
            {filtered.length ? (
              filtered.map((account) => {
                const key = `${account.host}:${account.login}`;
                const active = key === selectedKey;
                return (
                  <button
                    key={key}
                    type="button"
                    role="option"
                    aria-selected={active}
                    onClick={() => {
                      onSelect(key);
                      setOpen(false);
                      setSearch("");
                    }}
                    className={cn(
                      "flex min-h-12 w-full items-center gap-3 rounded-[0.6rem] px-3 py-2 text-left transition-colors",
                      active
                        ? "bg-primary/[0.11] text-foreground"
                        : "text-muted-foreground hover:bg-white/[0.04] hover:text-foreground",
                    )}
                  >
                    <span className="flex h-8 w-8 shrink-0 items-center justify-center rounded-full border border-white/10 bg-background/30">
                      <UserRound className="h-3.5 w-3.5" aria-hidden />
                    </span>
                    <span className="min-w-0 flex-1">
                      <span className="block truncate text-sm font-semibold">@{account.login}</span>
                      <span className="block truncate font-mono text-[0.65rem] text-muted-foreground">
                        {account.host}
                        {key === recommendedKey ? " · recommended" : ""}
                      </span>
                    </span>
                    {active ? (
                      <span className="flex h-6 w-6 items-center justify-center rounded-full bg-primary text-primary-foreground">
                        <Check className="h-3.5 w-3.5" aria-hidden />
                      </span>
                    ) : (
                      <span className="h-2.5 w-2.5 rounded-full border border-muted-foreground/55" />
                    )}
                  </button>
                );
              })
            ) : (
              <div className="px-3 py-7 text-center text-sm text-muted-foreground">
                No account matches “{search}”.
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  );
}

function errorMessage(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (typeof error === "string") return error;
  return "Choose a valid Git repository folder and try again.";
}
