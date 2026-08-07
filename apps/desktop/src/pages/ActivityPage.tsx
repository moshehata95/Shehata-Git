import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  ArrowDownUp,
  CheckCircle2,
  Clock3,
  RefreshCw,
  ScrollText,
  Search,
  ShieldCheck,
  Trash2,
  XCircle,
} from "lucide-react";
import { useMemo, useState } from "react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/ui/ConfirmDialog";
import { Card, CardContent } from "@/components/ui/card";
import { clearAuditEvents, deleteAuditEvent, listAuditEvents } from "@/lib/tauri";
import type { AuditEvent } from "@/lib/types";

type ResultFilter = "all" | "success" | "failed";
type SortOrder = "newest" | "oldest";
type KindFilter = "operations" | "credentials" | "all";

/**
 * Handing a credential to git is recorded every time git asks for one, which
 * is far more often than anything is pushed or committed. Left mixed in, those
 * rows bury the ones that say what actually happened to a repository.
 *
 * They are separated rather than hidden, and each tab carries its own count,
 * so nothing disappears without saying so.
 */
function isCredentialEvent(eventType: string): boolean {
  return eventType.startsWith("credential_");
}

export function ActivityPage() {
  const queryClient = useQueryClient();
  // The audit trail is the one surface users expect to be live: actions can
  // arrive from a terminal or a coding agent while this page is open. It reads
  // the local database only — no git or gh process is launched — so a short
  // poll while the page is visible is cheap.
  const events = useQuery({
    queryKey: ["audit"],
    queryFn: listAuditEvents,
    refetchInterval: 5_000,
    refetchIntervalInBackground: false,
    staleTime: 0,
  });
  const [search, setSearch] = useState("");
  const [result, setResult] = useState<ResultFilter>("all");
  const [sort, setSort] = useState<SortOrder>("newest");
  const [kind, setKind] = useState<KindFilter>("operations");
  const [deleteTarget, setDeleteTarget] = useState<AuditEvent | "all" | null>(null);
  const removeOne = useMutation({
    mutationFn: deleteAuditEvent,
    onSuccess: (_removed, id) => {
      queryClient.setQueryData<AuditEvent[]>(["audit"], (current) =>
        current?.filter((event) => event.id !== id),
      );
      setDeleteTarget(null);
    },
  });
  const clearAll = useMutation({
    mutationFn: clearAuditEvents,
    onSuccess: () => {
      queryClient.setQueryData<AuditEvent[]>(["audit"], []);
      setDeleteTarget(null);
    },
  });
  const filtered = useMemo(() => {
    const needle = search.trim().toLowerCase();
    const rows = (events.data ?? []).filter((event) => {
      const matchesResult =
        result === "all" ||
        (result === "success" ? event.result === "success" : event.result !== "success");
      const matchesSearch =
        !needle ||
        event.summary.toLowerCase().includes(needle) ||
        event.detail?.toLowerCase().includes(needle) ||
        event.event_type.toLowerCase().includes(needle) ||
        event.account_login?.toLowerCase().includes(needle);
      const matchesKind =
        kind === "all" || isCredentialEvent(event.event_type) === (kind === "credentials");
      return matchesResult && matchesSearch && matchesKind;
    });
    // The backend already returns newest first, so oldest is a plain reverse.
    return sort === "newest" ? rows : [...rows].reverse();
  }, [events.data, result, search, sort, kind]);

  const credentialCount = events.data?.filter((event) =>
    isCredentialEvent(event.event_type),
  ).length;
  const operationCount = (events.data?.length ?? 0) - (credentialCount ?? 0);
  const kindCounts: Record<KindFilter, number | undefined> = {
    operations: operationCount,
    credentials: credentialCount,
    all: events.data?.length,
  };
  const successCount = events.data?.filter((event) => event.result === "success").length ?? 0;
  const failedCount = (events.data?.length ?? 0) - successCount;

  return (
    <div className="mx-auto w-full max-w-5xl space-y-5">
      <section className="liquid-hero overflow-hidden rounded-[1rem]">
        <div className="grid gap-6 p-6 sm:grid-cols-[minmax(0,1fr)_auto] sm:items-end sm:p-8">
          <div>
            <p className="eyebrow">Redacted local audit</p>
            <h2 className="mt-3 font-display text-3xl font-semibold tracking-[-0.04em]">
              Every guarded action leaves a safe trace.
            </h2>
            <p className="mt-3 max-w-2xl text-sm leading-6 text-muted-foreground">
              Tokens, environment values, source contents, and credential material are never written
              to this history.
            </p>
          </div>
          <div className="flex gap-2">
            <AuditMetric icon={CheckCircle2} label="SUCCESS" value={successCount} />
            <AuditMetric icon={XCircle} label="FAILED" value={failedCount} />
          </div>
        </div>
      </section>

      <div className="liquid-panel flex flex-col gap-3 rounded-[0.8rem] p-3">
        <label className="flex min-h-11 min-w-0 flex-1 items-center gap-2 rounded-[0.65rem] border border-white/10 bg-background/25 px-2 transition-all focus-within:border-primary/40 focus-within:bg-background/40 focus-within:shadow-[0_0_0_3px_hsl(var(--primary)/0.08)]">
          <span className="flex h-8 w-8 shrink-0 items-center justify-center rounded-[0.5rem] bg-white/[0.035] text-muted-foreground">
            <Search className="h-4 w-4" aria-hidden />
          </span>
          <span className="sr-only">Search activity</span>
          <input
            type="search"
            value={search}
            onChange={(event) => setSearch(event.target.value)}
            placeholder="Search action, account, or event…"
            className="h-9 min-w-0 flex-1 bg-transparent text-sm outline-none placeholder:text-muted-foreground/50 focus-visible:ring-0 focus-visible:ring-offset-0 [&::-webkit-search-cancel-button]:hidden"
          />
        </label>
        <div className="flex flex-wrap items-center gap-2">
          <div className="flex gap-1 rounded-[0.55rem] border border-white/10 bg-background/20 p-1">
            {(["operations", "credentials", "all"] as const).map((option) => (
              <button
                key={option}
                type="button"
                onClick={() => setKind(option)}
                title={
                  option === "credentials"
                    ? "Every time git was handed a credential"
                    : option === "operations"
                      ? "Pushes, pulls, and commits"
                      : "Everything recorded"
                }
                className={`min-h-8 rounded-[0.4rem] px-3 text-xs font-semibold capitalize transition ${kind === option ? "bg-primary text-primary-foreground" : "text-muted-foreground hover:text-foreground"}`}
              >
                {option}
                {kindCounts[option] !== undefined && (
                  <span className="ml-1.5 font-mono text-[0.65rem] opacity-70">
                    {kindCounts[option]}
                  </span>
                )}
              </button>
            ))}
          </div>
          <div className="flex gap-1 rounded-[0.55rem] border border-white/10 bg-background/20 p-1">
            {(["all", "success", "failed"] as const).map((option) => (
              <button
                key={option}
                type="button"
                onClick={() => setResult(option)}
                className={`min-h-8 rounded-[0.4rem] px-3 text-xs font-semibold capitalize transition ${result === option ? "bg-primary text-primary-foreground" : "text-muted-foreground hover:text-foreground"}`}
              >
                {option}
              </button>
            ))}
          </div>
          <Button
            variant="ghost"
            size="sm"
            onClick={() => setSort((value) => (value === "newest" ? "oldest" : "newest"))}
            title={sort === "newest" ? "Showing newest first" : "Showing oldest first"}
          >
            <ArrowDownUp aria-hidden /> {sort === "newest" ? "Newest" : "Oldest"}
          </Button>
          <Button
            variant="ghost"
            size="sm"
            onClick={() => events.refetch()}
            disabled={events.isFetching}
          >
            <RefreshCw className={events.isFetching ? "animate-spin" : undefined} aria-hidden />{" "}
            Refresh
          </Button>
          <Button
            variant="ghost"
            size="sm"
            className="text-muted-foreground hover:text-destructive"
            onClick={() => setDeleteTarget("all")}
            disabled={!events.data?.length || clearAll.isPending || removeOne.isPending}
          >
            <Trash2 aria-hidden /> Clear history
          </Button>
        </div>
      </div>

      {events.isError && (
        <Card className="border-destructive/40">
          <CardContent className="py-4 text-sm text-destructive">
            {events.error instanceof Error ? events.error.message : "Could not read activity."}
          </CardContent>
        </Card>
      )}

      {!events.isLoading && filtered.length === 0 && (
        <Card>
          <CardContent className="flex min-h-52 flex-col items-center justify-center gap-3 text-center">
            <ScrollText className="h-8 w-8 text-muted-foreground/45" aria-hidden />
            <div>
              <p className="font-medium">
                {events.data?.length ? "No matching activity" : "Nothing yet"}
              </p>
              <p className="mt-1 max-w-sm text-sm text-muted-foreground">
                {events.data?.length
                  ? "Try a different search, result, or category filter."
                  : "Repository routing and Git actions will appear here."}
              </p>
            </div>
          </CardContent>
        </Card>
      )}

      <div className="relative space-y-3 before:absolute before:bottom-6 before:left-[1.45rem] before:top-6 before:w-px before:bg-white/10 sm:before:left-[1.95rem]">
        {filtered.map((event) => {
          const succeeded = event.result === "success";
          return (
            <article
              key={event.id}
              className="liquid-panel relative rounded-[0.8rem] p-4 pl-14 sm:p-5 sm:pl-20"
            >
              <span
                className={`absolute left-[0.95rem] top-5 z-10 flex h-8 w-8 items-center justify-center rounded-full border sm:left-[1.45rem] ${succeeded ? "border-success/30 bg-success/10 text-success" : "border-destructive/30 bg-destructive/10 text-destructive"}`}
              >
                {succeeded ? (
                  <ShieldCheck className="h-3.5 w-3.5" aria-hidden />
                ) : (
                  <XCircle className="h-3.5 w-3.5" aria-hidden />
                )}
              </span>
              <div className="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
                <div className="min-w-0">
                  <div className="flex flex-wrap items-center gap-2">
                    <p className="text-sm font-semibold leading-6">{event.summary}</p>
                    <Badge variant={succeeded ? "success" : "destructive"}>{event.result}</Badge>
                  </div>
                  {event.detail && (
                    <p className="mt-1.5 truncate text-xs leading-5 text-muted-foreground/90">
                      {event.detail}
                    </p>
                  )}
                  <p className="mt-2 font-mono text-[0.68rem] uppercase tracking-[0.08em] text-muted-foreground">
                    {event.event_type.replaceAll("_", " ")}
                    {event.account_login ? ` · @${event.account_login}` : ""}
                  </p>
                </div>
                <div className="flex shrink-0 items-center gap-2">
                  <time className="flex items-center gap-1.5 font-mono text-[0.68rem] text-muted-foreground">
                    <Clock3 className="h-3 w-3" aria-hidden />
                    {new Date(event.timestamp).toLocaleString()}
                  </time>
                  <button
                    type="button"
                    onClick={() => setDeleteTarget(event)}
                    disabled={clearAll.isPending || removeOne.isPending}
                    className="flex h-9 w-9 items-center justify-center rounded-[0.5rem] text-muted-foreground transition-colors hover:bg-destructive/10 hover:text-destructive disabled:opacity-40"
                    aria-label={`Delete ${event.summary}`}
                    title="Delete this event"
                  >
                    <Trash2 className="h-4 w-4" aria-hidden />
                  </button>
                </div>
              </div>
            </article>
          );
        })}
      </div>

      {deleteTarget && (
        <ConfirmDialog
          eyebrow={deleteTarget === "all" ? "Clear local history" : "Delete local event"}
          title={deleteTarget === "all" ? "Clear all activity?" : "Delete this activity event?"}
          description={
            deleteTarget === "all"
              ? `This permanently removes all ${events.data?.length ?? 0} redacted events from this PC.`
              : "This permanently removes the selected redacted event from this PC."
          }
          detail="This does not change repositories, accounts, commits, or anything on GitHub."
          confirmLabel={deleteTarget === "all" ? "Clear all history" : "Delete event"}
          pendingLabel="Deleting…"
          pending={clearAll.isPending || removeOne.isPending}
          onCancel={() => setDeleteTarget(null)}
          onConfirm={() => {
            if (deleteTarget === "all") clearAll.mutate();
            else removeOne.mutate(deleteTarget.id);
          }}
        />
      )}
    </div>
  );
}

function AuditMetric({
  icon: Icon,
  label,
  value,
}: {
  icon: typeof CheckCircle2;
  label: string;
  value: number;
}) {
  return (
    <div className="min-w-24 rounded-[0.7rem] border border-white/10 bg-background/20 p-3">
      <div className="flex items-center gap-1.5 text-muted-foreground">
        <Icon className="h-3.5 w-3.5" aria-hidden />
        <span className="data-label">{label}</span>
      </div>
      <p className="mt-2 font-mono text-xl">{String(value).padStart(2, "0")}</p>
    </div>
  );
}
