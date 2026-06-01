<script lang="ts">
  // Workspace shell (#57 PR1). Single-symbol model: pick a ticker on the
  // right, see everything about it in the right detail panel; workflows
  // (events, discovery, decisions, calibration) live in the bottom drawer.
  // Chart in the main area is a placeholder — PR2 wires a real chart.
  import { onMount } from "svelte";
  import {
    ackAlert,
    addToWatchlist,
    confirmCandidate,
    createWatchlist,
    fetchAlerts,
    fetchBrainOverview,
    fetchBrainStatus,
    fetchCalibration,
    fetchAttention,
    dismissAttention,
    fetchDecisions,
    fetchDiscoveryPool,
    fetchEvidenceRequirements,
    fetchPendingCandidates,
    fetchPositions,
    fetchRegime,
    fetchResearchEvidence,
    fetchThesisDeclines,
    fetchTheses,
    fetchTickerContext,
    fetchTickers,
    fetchWatchlistMembers,
    fetchWatchlists,
    postDecision,
    rejectCandidate,
    removeFromWatchlist,
    subscribe,
    transitionAttention,
    type Alert,
    type AttentionItem,
    type BrainOverview,
    type BrainSourceStatus,
    type BrainStatus,
    type BrainThesis,
    type Calibration,
    type DecisionRow,
    type EvidenceRequirement,
    type MarketState,
    type PendingCandidate,
    type PoolMember,
    type PositionRow,
    type ResearchEvidence,
    type StreamEvent,
    type ThesisDetail,
    type ThesisDecline,
    type Ticker,
    type TickerContext,
    type Watchlist,
    type WatchlistMember,
  } from "./lib/api";
  import ContextPanel from "./lib/ContextPanel.svelte";
  import ThesisDetails from "./lib/ThesisDetails.svelte";
  import ChartPanel from "./lib/ChartPanel.svelte";
  import { PaneGroup, Pane, PaneResizer } from "paneforge";

  // ---------- workspace state ----------
  type RightTab = "overview" | "context" | "evidence" | "theses" | "alerts" | "decisions";
  type BottomMode = "brain" | "attention" | "events" | "discovery" | "decisions" | "calibration" | "diagnostics";

  let selectedSymbol = $state<string | null>(null);
  let rightTab = $state<RightTab>("overview");
  let bottomMode = $state<BottomMode>("attention");
  let bottomOpen = $state(true);

  // ---------- global data ----------
  let regime = $state<MarketState | null>(null);
  let brainOverview = $state<BrainOverview | null>(null);
  let calibration = $state<Calibration | null>(null);
  // System status (#92) — populated on demand when diagnostics tab is open.
  let sysStatus = $state<Record<string, unknown> | null>(null);
  let sysStatusError = $state<string | null>(null);
  let sysStatusTimer: ReturnType<typeof setInterval> | null = null;
  let tickers = $state<Ticker[]>([]);
  let alerts = $state<Alert[]>([]);
  let live = $state<StreamEvent[]>([]);
  let connected = $state(false);
  let error = $state<string | null>(null);
  let pending = $state<PendingCandidate[]>([]);
  let watchlists = $state<Watchlist[]>([]);
  let watchlistMembers = $state<Record<string, WatchlistMember[]>>({});
  let pool = $state<PoolMember[]>([]);
  let attention = $state<AttentionItem[]>([]);
  let attentionFilter = $state<string>("all");

  async function refreshAttention() {
    try {
      attention = await fetchAttention("open");
    } catch {}
  }
  async function dismissOne(id: number, reason?: string) {
    try {
      await dismissAttention(id, reason);
      await refreshAttention();
    } catch (e) {
      error = String(e);
    }
  }
  async function deferOne(id: number) {
    try {
      await transitionAttention(id, {
        to_state: "operator_deferred",
        owner: "operator",
        reason: "defer",
        source_ref: { source: "operator" },
      });
      await refreshAttention();
    } catch (e) {
      error = String(e);
    }
  }
  async function rejectGroup(candidateIds: number[], _reason: string) {
    try {
      // Iterate; backend resolves the matching attention item per candidate.
      for (const id of candidateIds) await rejectCandidate(id);
      await Promise.all([refreshAttention(), refreshPending()]);
    } catch (e) {
      error = String(e);
    }
  }
  async function confirmGroup(candidateIds: number[]) {
    const lists = new Set<string>();
    for (const cid of candidateIds) {
      const inner = chosenLists[cid] ?? {};
      for (const [wlId, on] of Object.entries(inner)) if (on) lists.add(wlId);
    }
    const ids = [...lists];
    try {
      // Confirm always promotes the ticker. Optional checked lists add
      // watchlist memberships; empty list selection means Universe only.
      for (const cid of candidateIds) await confirmCandidate(cid, ids);
      await Promise.all([refreshAttention(), refreshPending(), refreshWatchlists(), fetchTickers().then((t) => (tickers = t))]);
    } catch (e) {
      error = String(e);
    }
  }

  // Plain-English reason from a candidate's signal_name + signal_value.
  function reasonFor(signal: string, value: number | null): string {
    if (signal === "research_nomination") return "research nomination";
    if (signal === "volume_anomaly" && value !== null) return `${value.toFixed(1)}× volume vs 20-day avg`;
    if (signal === "base_breakout" && value !== null) return `base breakout +${value.toFixed(2)}% above prior high`;
    if (signal === "estimate_revision_velocity" && value !== null) {
      const dir = value > 0 ? "↑" : "↓";
      return `${Math.abs(value)|0} net estimate revisions ${dir}`;
    }
    if (signal === "news_sentiment_shift" && value !== null) {
      const sign = value > 0 ? "+" : "";
      return `news sentiment shift ${sign}${value.toFixed(2)}`;
    }
    return signal.replace(/_/g, " ");
  }

  function rawSignals(it: AttentionItem): string[] {
    const raw = it.source_ref?.raw_signals;
    return Array.isArray(raw) ? raw.filter((s): s is string => typeof s === "string") : [];
  }

  function displayReason(text: string): string {
    return text.replace(/vs SMA\b/g, "vs 200-day SMA");
  }

  // Pretty short relative time ("2m", "3h", "1d", or absolute "3:17 PM").
  function relativeTime(iso: string): string {
    const t = new Date(iso).getTime();
    const dt = Date.now() - t;
    if (dt < -86_400_000) return new Date(t).toLocaleDateString();
    if (dt < -3_600_000) return `in ${Math.ceil(Math.abs(dt) / 3_600_000)}h`;
    if (dt < -60_000) return `in ${Math.ceil(Math.abs(dt) / 60_000)}m`;
    if (dt < 60_000) return "just now";
    if (dt < 3_600_000) return `${Math.floor(dt / 60_000)}m ago`;
    if (dt < 86_400_000) return new Date(t).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
    if (dt < 7 * 86_400_000) return `${Math.floor(dt / 86_400_000)}d ago`;
    return new Date(t).toLocaleDateString();
  }

  function healthLabel(status: string, failureKind?: string | null): string {
    if (failureKind) return failureKind;
    if (status === "no_new_rows") return "checked, no new rows";
    if (status === "ok") return "new data";
    if (status === "rate_limited") return "rate limited";
    return status;
  }

  function brainStatusLabel(status: string): string {
    return status.replace(/_/g, " ");
  }

  function brainActionLabel(action: string): string {
    return action.replace(/_/g, " ");
  }

  function sourceLabel(source: string): string {
    return source.replace(/_/g, " ");
  }

  function sourceTime(source: BrainSourceStatus): string {
    if (source.last_checked_at) return `checked ${relativeTime(source.last_checked_at)}`;
    if (source.last_changed_at) return `changed ${relativeTime(source.last_changed_at)}`;
    return "not seen";
  }

  function brainDirectionLabel(direction: string): string {
    if (direction === "risk_on") return "risk on";
    if (direction === "risk_off") return "risk off";
    return direction.replace(/_/g, " ");
  }

  function brainThingText(value: unknown): string {
    if (typeof value === "string") return value;
    if (!value || typeof value !== "object" || Array.isArray(value)) return String(value ?? "");
    const obj = value as Record<string, unknown>;
    for (const key of ["name", "assertion", "claim", "source", "reason"]) {
      if (typeof obj[key] === "string") return obj[key] as string;
    }
    return JSON.stringify(obj);
  }

  function brainMaintainer(sourceRef: Record<string, unknown>): Record<string, unknown> | null {
    const maintainer = sourceRef?.maintainer;
    return maintainer && typeof maintainer === "object" && !Array.isArray(maintainer)
      ? maintainer as Record<string, unknown>
      : null;
  }

  function brainCoverageText(sourceRef: Record<string, unknown>): string {
    const maintainer = brainMaintainer(sourceRef);
    const coverage = maintainer?.coverage;
    if (!coverage || typeof coverage !== "object" || Array.isArray(coverage)) return "";
    const c = coverage as Record<string, unknown>;
    const linked = Number(c.linked ?? 0);
    if (!linked) return "";
    return [
      `${Number(c.contexts ?? 0)}/${linked} context`,
      `${Number(c.open_theses ?? 0)}/${linked} theses`,
      `${Number(c.news ?? 0)}/${linked} news`,
      `${Number(c.estimates ?? 0)}/${linked} estimates`,
      `${Number(c.analyst_opinion ?? 0)}/${linked} opinion`,
    ].join(" · ");
  }

  function brainSourceText(sourceRef: Record<string, unknown>): string {
    const maintainer = brainMaintainer(sourceRef);
    const sources = maintainer?.sources;
    if (!sources || typeof sources !== "object" || Array.isArray(sources)) return "";
    return Object.values(sources as Record<string, unknown>)
      .filter((item): item is Record<string, unknown> => !!item && typeof item === "object" && !Array.isArray(item))
      .map((item) => `${sourceLabel(String(item.source ?? ""))} ${String(item.freshness ?? item.status ?? "")}`)
      .filter((item) => item.trim() !== "")
      .join(" · ");
  }

  function evidenceActions(req: EvidenceRequirement): string[] {
    const actions = req.source_ref?.fetch_actions;
    return Array.isArray(actions)
      ? actions.filter((a): a is string => typeof a === "string")
      : [];
  }

  function evidencePriorityLabel(priority: EvidenceRequirement["priority"]): string {
    if (priority === "blocking") return "blocks if missing";
    return `${priority} priority`;
  }

  function evidenceRequirementCount(req: EvidenceRequirement): string {
    const counts = req.source_ref?.counts;
    if (!counts || typeof counts !== "object" || Array.isArray(counts)) return "";
    const keyByRequirement: Record<string, string> = {
      price_history: "price_bars",
      company_facts: "company_facts",
      recent_news: "recent_news",
      analyst_estimates: "estimate_snapshots",
      product_research: "research_evidence",
    };
    const countKey = keyByRequirement[req.requirement_key];
    const value = countKey ? (counts as Record<string, unknown>)[countKey] : undefined;
    if (typeof value !== "number") return "";
    const label = countKey.replace(/_/g, " ");
    return `available: ${value.toLocaleString()} ${label}`;
  }

  function evidenceCounts(req: EvidenceRequirement): string {
    const counts = req.source_ref?.counts;
    if (!counts || typeof counts !== "object" || Array.isArray(counts)) return "";
    return Object.entries(counts)
      .filter(([, v]) => typeof v === "number")
      .map(([k, v]) => `${k.replace(/_/g, " ")} ${v}`)
      .join(" · ");
  }

  function evidenceHealth(req: EvidenceRequirement): string {
    const state = req.source_ref?.acquisition_state;
    const health = req.source_ref?.source_health;
    const parts: string[] = [];
    if (typeof state === "string") parts.push(state.replace(/_/g, " "));
    if (Array.isArray(health)) {
      for (const h of health) {
        if (!h || typeof h !== "object" || Array.isArray(h)) continue;
        const row = h as Record<string, unknown>;
        const source = typeof row.source === "string" ? row.source.replace(/_/g, " ") : "";
        const status = typeof row.last_status === "string" ? row.last_status.replace(/_/g, " ") : "";
        if (source || status) parts.push(`${source} ${status}`.trim());
      }
    }
    return [...new Set(parts)].join(" · ");
  }

  function evidenceSourceTasks(req: EvidenceRequirement): string {
    const tasks = req.source_tasks ?? [];
    if (!tasks.length) return "";
    return tasks
      .slice(0, 4)
      .map((task) => {
        const action = task.action.replace(/_/g, " ");
        const state = task.state.replace(/_/g, " ");
        const due = task.due_at ? ` ${relativeTime(task.due_at)}` : "";
        return `${action}: ${state}${due}`;
      })
      .join(" · ");
  }

  // Group attention items by (kind, symbol). For candidate_review this
  // collapses N candidates on the same ticker into one card; for other
  // kinds it's typically 1 item per group.
  type AttGroup = {
    key: string;
    kind: string;
    symbol: string | null;
    severity: string;
    fsmState: string;
    owner: string;
    stateReason: string | null;
    nextRetryAt: string | null;
    resurfaceAt: string | null;
    items: AttentionItem[];
    candidateIds: number[];     // for candidate_review groups
    latestAt: string;
  };
  type AttSection = {
    key: string;
    fsmState: string;
    owner: string;
    groups: AttGroup[];
  };
  let groupedAttention = $derived.by<AttGroup[]>(() => {
    const map = new Map<string, AttGroup>();
    for (const a of attention) {
      const fsmState = a.fsm_state ?? "ready_for_review";
      const owner = a.owner ?? "operator";
      const key = `${fsmState}::${owner}::${a.kind}::${a.symbol ?? ""}::${a.thesis_id ?? ""}`;
      const g = map.get(key) ?? {
        key,
        kind: a.kind,
        symbol: a.symbol ?? null,
        severity: a.severity,
        fsmState,
        owner,
        stateReason: a.state_reason ?? null,
        nextRetryAt: a.next_retry_at ?? null,
        resurfaceAt: a.resurface_at ?? null,
        items: [], candidateIds: [], latestAt: a.created_at,
      };
      g.items.push(a);
      if (a.candidate_id) g.candidateIds.push(a.candidate_id);
      if (a.created_at > g.latestAt) g.latestAt = a.created_at;
      const rank = (s: string) =>
        s === "blocked" ? 0 : s === "decision" ? 1 : s === "review" ? 2 : 3;
      if (rank(a.severity) < rank(g.severity)) g.severity = a.severity;
      map.set(key, g);
    }
    return [...map.values()].sort((a, b) => {
      const rank = (s: string) =>
        s === "blocked" ? 0 : s === "decision" ? 1 : s === "review" ? 2 : 3;
      const r = rank(a.severity) - rank(b.severity);
      return r !== 0 ? r : (b.latestAt > a.latestAt ? 1 : -1);
    });
  });

  function attentionStateRank(state: string): number {
    return {
      actionable: 0,
      ready_for_review: 1,
      blocked: 2,
      waiting_on_data: 3,
      evaluating: 4,
      queued: 5,
      operator_deferred: 6,
      resolved: 7,
      dismissed: 8,
    }[state] ?? 9;
  }

  function attentionStateLabel(state: string): string {
    return state.replace(/_/g, " ");
  }

  function attentionOwnerLabel(owner: string): string {
    const labels: Record<string, string> = {
      operator: "operator owns next step",
      source: "waiting on data source",
      cognition: "cognition owns next step",
      risk: "risk owns next step",
      system: "system owns next step",
    };
    return labels[owner] ?? `${owner} owns next step`;
  }

  function attentionSections(groups: AttGroup[]): AttSection[] {
    const map = new Map<string, AttSection>();
    for (const group of groups) {
      const key = `${group.fsmState}::${group.owner}`;
      const section = map.get(key) ?? {
        key,
        fsmState: group.fsmState,
        owner: group.owner,
        groups: [],
      };
      section.groups.push(group);
      map.set(key, section);
    }
    return [...map.values()].sort((a, b) => {
      const r = attentionStateRank(a.fsmState) - attentionStateRank(b.fsmState);
      if (r !== 0) return r;
      return a.owner.localeCompare(b.owner);
    });
  }

  // Reject reasons (#95 disagreement_reason vocabulary).
  const REJECT_REASONS = [
    "wrong_cluster",
    "not_my_edge",
    "signal_too_weak",
    "valuation_priced",
    "data_stale",
    "llm_overreached",
    "risk_too_high",
  ];
  let rejectOpenFor = $state<string | null>(null);

  // ---------- selected-symbol-scoped data ----------
  let symbolContext = $state<TickerContext | null | undefined>(undefined);
  let symbolEvidence = $state<EvidenceRequirement[] | undefined>(undefined);
  let symbolResearch = $state<ResearchEvidence[] | undefined>(undefined);
  let symbolBrain = $state<BrainStatus | null | undefined>(undefined);
  let symbolTheses = $state<ThesisDetail[] | null | undefined>(undefined);
  let symbolDeclines = $state<ThesisDecline[] | null | undefined>(undefined);
  let symbolDecisions = $state<DecisionRow[] | null | undefined>(undefined);
  let symbolPositions = $state<PositionRow[] | null | undefined>(undefined);
  let openSymbolTheses = $derived.by<ThesisDetail[]>(() =>
    [...(symbolTheses ?? [])]
      .filter((t) => !["closed", "disqualified"].includes(t.state))
      .sort((a, b) => Date.parse(b.updated_at) - Date.parse(a.updated_at)),
  );
  let currentSymbolThesis = $derived<ThesisDetail | null>(openSymbolTheses[0] ?? null);
  let openSymbolPositions = $derived<PositionRow[]>(
    (symbolPositions ?? []).filter((p) => !p.closed_at),
  );
  let retiredSymbolTheses = $derived.by<ThesisDetail[]>(() =>
    [...(symbolTheses ?? [])]
      .filter((t) => ["closed", "disqualified"].includes(t.state))
      .sort((a, b) => Date.parse(b.updated_at) - Date.parse(a.updated_at)),
  );
  let activeThesisDirections = $derived.by<string[]>(() => {
    const dirs = new Set<string>();
    for (const t of openSymbolTheses) {
      const dir = forecastDirectionFrom(t.forecast);
      if (dir) dirs.add(dir);
    }
    return [...dirs].sort();
  });
  // We don't have a per-symbol alerts endpoint yet; we filter globally.
  let showAcked = $state(false);

  // ---------- discovery review state (still uses the same model) ----------
  let chosenLists = $state<Record<number, Record<string, boolean>>>({});

  // ---------- watchlist controls ----------
  let newListName = $state("");
  let addSymbolFor = $state<Record<string, string>>({});
  let expandedListIds = $state<Record<string, boolean>>({});

  // ---------- decision form (in bottom drawer) ----------
  let decThesisId = $state("");
  let decAction = $state("skip");
  let decSide = $state("none");
  let decInstrument = $state("equity");
  let decChoice = $state("deferred");
  let decStatus = $state<string | null>(null);
  let decRecordFill = $state(false);
  let decPositionId = $state("");
  let decQty = $state("");
  let decPrice = $state("");
  let decFees = $state("");
  let decDeltaNotional = $state("");
  let decPremiumAtRisk = $state("");
  let decFillNotes = $state("");
  let decThesis = $derived(symbolTheses?.find((t) => t.thesis_id === decThesisId) ?? null);
  let decThesisDirection = $derived(forecastDirectionFrom(decThesis?.forecast));

  // Synthetic "Universe" pseudo-list — all active tickers. Computed on the
  // fly from /api/tickers so we don't need a DB-side system list.
  const UNIVERSE_ID = "__universe__";
  let universeList = $derived<Watchlist>({
    id: UNIVERSE_ID,
    name: "Universe",
    description: "All active tickers",
    color: "#9aa3b8",
    is_system: true,
    created_at: "",
    member_count: tickers.length,
  });
  let universeMembers = $derived<WatchlistMember[]>(
    tickers.map((t) => ({
      watchlist_id: UNIVERSE_ID,
      symbol: t.symbol,
      added_at: t.added_at,
      added_by: "system",
      latest_thesis_id: t.latest_thesis_id,
      thesis_state: t.thesis_state,
      thesis_direction: t.thesis_direction,
      open_theses: t.open_theses,
    })),
  );

  // Discovery pool pseudo-list (#88) — broad investible names from FMP screener.
  // Bigger than the active universe; clicking a pool member loads its chart
  // and (sparse) context so the operator can decide whether to promote.
  const POOL_ID = "__pool__";
  let poolList = $derived<Watchlist>({
    id: POOL_ID,
    name: "Discovery pool",
    description: "Broad scan pool (FMP screener)",
    color: "#cba6f7",
    is_system: true,
    created_at: "",
    member_count: pool.length,
  });
  let poolMembers = $derived<WatchlistMember[]>(
    pool.map((p) => ({
      watchlist_id: POOL_ID,
      symbol: p.symbol,
      added_at: p.first_seen_at,
      added_by: "pool",
      latest_thesis_id: p.latest_thesis_id,
      thesis_state: p.thesis_state,
      thesis_direction: p.thesis_direction,
      open_theses: p.open_theses ?? 0,
    })),
  );
  let allWatchlists = $derived<Watchlist[]>([...watchlists, universeList, poolList]);

  // ---------- helpers ----------
  function regimeColor(r: string | undefined): string {
    switch (r) {
      case "risk_on": return "rgb(166, 227, 161)";
      case "risk_off": return "rgb(243, 139, 168)";
      case "neutral": return "rgb(249, 226, 175)";
      default: return "rgb(124, 124, 124)";
    }
  }
  function kindColor(k: string, payload: Record<string, unknown> | undefined): string {
    if (k === "risk") {
      if ((payload as any)?.veto) return "rgb(243, 139, 168)";
      if ((payload as any)?.kind === "goalpost_moved") return "rgb(245, 194, 231)";
      return "rgb(249, 226, 175)";
    }
    if (k === "state_transition") return "rgb(137, 180, 250)";
    return "rgb(180, 190, 254)";
  }
  function shortTs(s: string): string {
    if (!s) return "";
    const d = new Date(s);
    return d.toLocaleTimeString();
  }

  function money(v: number | null | undefined): string {
    if (v === null || v === undefined || Number.isNaN(v)) return "—";
    return v.toLocaleString(undefined, { style: "currency", currency: "USD", maximumFractionDigits: 0 });
  }

  function price(v: number | null | undefined): string {
    if (v === null || v === undefined || Number.isNaN(v)) return "—";
    return v.toLocaleString(undefined, { style: "currency", currency: "USD", maximumFractionDigits: 2 });
  }

  function parseOptionalNumber(value: unknown): number | undefined {
    const trimmed = String(value ?? "").trim();
    if (!trimmed) return undefined;
    const n = Number(trimmed);
    return Number.isFinite(n) ? n : undefined;
  }

  function resetFillForm() {
    decPositionId = "";
    decQty = "";
    decPrice = "";
    decFees = "";
    decDeltaNotional = "";
    decPremiumAtRisk = "";
    decFillNotes = "";
  }

  function usePositionForExit(p: PositionRow) {
    decThesisId = p.thesis_id ?? "";
    decAction = "exit";
    decChoice = "confirmed";
    decRecordFill = true;
    decPositionId = p.position_id;
    decSide = p.side;
    decInstrument = p.instrument;
    decQty = String(p.qty);
    decPrice = p.latest_price ? String(p.latest_price) : "";
    bottomMode = "decisions";
    if (!bottomOpen) bottomPane?.expand();
  }

  function openBrainDrawer() {
    bottomMode = "brain";
    if (!bottomOpen) bottomPane?.expand();
  }

  function forecastDirectionFrom(forecast: Record<string, unknown> | null | undefined): string | null {
    const dir = forecast?.direction;
    return typeof dir === "string" && dir.length > 0 ? dir : null;
  }

  function thesisStatusLabel(state: string | null | undefined): string {
    return state ? state.replace(/_/g, " ") : "no thesis";
  }

  function thesisDirectionLabel(direction: string | null | undefined): string {
    if (direction === "up") return "bull";
    if (direction === "down") return "bear";
    if (direction === "neutral") return "neutral";
    return "none";
  }

  function thesisDirectionClass(direction: string | null | undefined): string {
    if (direction === "up" || direction === "down" || direction === "neutral") return `thesis-${direction}`;
    return "thesis-none";
  }

  function decisionIntentLabel(d: DecisionRow): string {
    if (d.action === "enter") {
      const side = (d.side ?? "").trim();
      if (side && side !== "none") return `enter ${side}`;
      if (d.thesis_direction === "down") return "enter bearish thesis";
      if (d.thesis_direction === "up") return "enter bullish thesis";
      return "enter thesis";
    }
    return d.action;
  }

  function visibleSizing(d: DecisionRow): Record<string, unknown> | null {
    const entries = Object.entries(d.sizing ?? {}).filter(
      ([k]) => !["side", "instrument", "thesis_direction"].includes(k),
    );
    return entries.length > 0 ? Object.fromEntries(entries) : null;
  }

  function tickerFor(symbol: string | null): Ticker | undefined {
    if (!symbol) return undefined;
    return tickers.find((t) => t.symbol === symbol);
  }

  function membersFor(listId: string): WatchlistMember[] {
    if (listId === UNIVERSE_ID) return universeMembers;
    if (listId === POOL_ID) return poolMembers;
    return watchlistMembers[listId] ?? [];
  }

  function normalizeSymbol(value: string | null | undefined): string | null {
    const symbol = (value ?? "").trim().toUpperCase();
    if (!/^[A-Z][A-Z0-9.-]{0,9}$/.test(symbol)) return null;
    return symbol;
  }

  function symbolFromRoute(): string | null {
    const match = window.location.pathname.match(/^\/symbol\/([^/]+)\/?$/);
    return match ? normalizeSymbol(decodeURIComponent(match[1])) : null;
  }

  function syncSymbolRoute(symbol: string, replace = false) {
    const path = `/symbol/${encodeURIComponent(symbol)}`;
    if (window.location.pathname === path) return;
    const method = replace ? "replaceState" : "pushState";
    window.history[method](null, "", path);
  }

  // ---------- selection logic ----------
  async function selectSymbol(
    value: string,
    opts: { updateRoute?: boolean; replaceRoute?: boolean } = {},
  ) {
    const symbol = normalizeSymbol(value);
    if (!symbol) return;
    if (opts.updateRoute ?? true) syncSymbolRoute(symbol, opts.replaceRoute ?? false);
    if (selectedSymbol === symbol) return;
    selectedSymbol = symbol;
    symbolContext = undefined;
    symbolEvidence = undefined;
    symbolResearch = undefined;
    symbolBrain = undefined;
    symbolTheses = undefined;
    symbolDeclines = undefined;
    symbolDecisions = undefined;
    symbolPositions = undefined;
    // Fetch detail in parallel.
    const [ctx, evidence, research, brain, theses, declines, decisions, positions] = await Promise.all([
      fetchTickerContext(symbol).catch(() => null),
      fetchEvidenceRequirements(symbol).catch(() => []),
      fetchResearchEvidence(symbol).catch(() => []),
      fetchBrainStatus(symbol).catch(() => null),
      fetchTheses(symbol).catch(() => []),
      fetchThesisDeclines(symbol).catch(() => []),
      fetchDecisions(symbol).catch(() => []),
      fetchPositions(symbol).catch(() => []),
    ]);
    symbolContext = ctx;
    symbolEvidence = evidence;
    symbolResearch = research;
    symbolBrain = brain;
    symbolTheses = theses;
    symbolDeclines = declines;
    symbolDecisions = decisions;
    symbolPositions = positions;
  }

  function pickFirstSymbol() {
    if (selectedSymbol) return;
    // Try first non-empty user watchlist, then Universe.
    for (const w of allWatchlists) {
      const m = membersFor(w.id);
      if (m.length > 0) {
        expandedListIds = { ...expandedListIds, [w.id]: true };
        selectSymbol(m[0].symbol);
        return;
      }
    }
  }

  async function toggleListExpanded(id: string) {
    const open = !expandedListIds[id];
    expandedListIds = { ...expandedListIds, [id]: open };
    if (open && id !== UNIVERSE_ID && id !== POOL_ID && !watchlistMembers[id]) {
      try {
        const m = await fetchWatchlistMembers(id);
        watchlistMembers = { ...watchlistMembers, [id]: m };
      } catch (e) {
        error = String(e);
      }
    }
  }

  // ---------- discovery review ----------
  async function refreshPending() {
    try {
      pending = await fetchPendingCandidates();
      const fresh: Record<number, Record<string, boolean>> = {};
      for (const c of pending) {
        fresh[c.id] = chosenLists[c.id] ?? {};
        for (const p of c.proposed_lists) {
          if (p.watchlist_id && fresh[c.id][p.watchlist_id] === undefined) {
            fresh[c.id][p.watchlist_id] = p.confidence !== "low";
          }
        }
      }
      chosenLists = fresh;
    } catch (e) {
      error = String(e);
    }
  }
  function toggleChoice(candId: number, wlId: string) {
    const inner = { ...(chosenLists[candId] ?? {}) };
    inner[wlId] = !inner[wlId];
    chosenLists = { ...chosenLists, [candId]: inner };
  }
  async function confirmOne(candId: number) {
    const inner = chosenLists[candId] ?? {};
    const ids = Object.entries(inner).filter(([, v]) => v).map(([k]) => k);
    try {
      await confirmCandidate(candId, ids);
      await Promise.all([refreshPending(), refreshWatchlists(), fetchTickers().then((t) => (tickers = t))]);
    } catch (e) {
      error = String(e);
    }
  }
  async function rejectOne(candId: number) {
    try {
      await rejectCandidate(candId);
      await refreshPending();
    } catch (e) {
      error = String(e);
    }
  }

  // ---------- watchlists CRUD ----------
  async function refreshWatchlists() {
    try {
      watchlists = await fetchWatchlists();
    } catch (e) {
      error = String(e);
    }
  }
  async function submitNewList(e: Event) {
    e.preventDefault();
    if (!newListName.trim()) return;
    try {
      await createWatchlist({ name: newListName.trim() });
      newListName = "";
      await refreshWatchlists();
    } catch (err) {
      error = String(err);
    }
  }
  async function addMember(id: string) {
    const sym = normalizeSymbol(addSymbolFor[id]) ?? "";
    if (!sym) return;
    try {
      await addToWatchlist(id, sym);
      addSymbolFor = { ...addSymbolFor, [id]: "" };
      const m = await fetchWatchlistMembers(id);
      watchlistMembers = { ...watchlistMembers, [id]: m };
      await refreshWatchlists();
    } catch (err) {
      error = String(err);
    }
  }
  async function removeMember(id: string, symbol: string) {
    try {
      await removeFromWatchlist(id, symbol);
      const m = await fetchWatchlistMembers(id);
      watchlistMembers = { ...watchlistMembers, [id]: m };
      await refreshWatchlists();
    } catch (err) {
      error = String(err);
    }
  }

  // ---------- alerts ----------
  async function ack(id: number) {
    try {
      await ackAlert(id);
      alerts = alerts.filter((a) => a.id !== id || showAcked);
      if (showAcked) {
        alerts = alerts.map((a) => (a.id === id ? { ...a, acknowledged: true } : a));
      }
    } catch (e) {
      error = String(e);
    }
  }

  // ---------- decision form ----------
  async function submitDecision(e: Event) {
    e.preventDefault();
    if (decAction === "enter" && decSide === "none") {
      decStatus = "pick a trade side before entering";
      return;
    }
    const qty = parseOptionalNumber(decQty);
    const fillPrice = parseOptionalNumber(decPrice);
    const fees = parseOptionalNumber(decFees) ?? 0;
    if (decRecordFill && (qty === undefined || qty <= 0 || fillPrice === undefined || fillPrice <= 0)) {
      decStatus = "manual fill needs positive qty and price";
      return;
    }
    decStatus = "sending…";
    try {
      const sizing: Record<string, unknown> = {};
      if (decAction === "enter" || decAction === "resize" || decAction === "exit") {
        sizing.side = decSide;
        sizing.instrument = decInstrument;
      }
      const delta = parseOptionalNumber(decDeltaNotional);
      const premium = parseOptionalNumber(decPremiumAtRisk);
      if (delta !== undefined) sizing.delta_notional = delta;
      if (premium !== undefined) sizing.premium_at_risk = premium;
      if (decThesisDirection) sizing.thesis_direction = decThesisDirection;
      const manual_fill = decRecordFill && qty !== undefined && fillPrice !== undefined
        ? {
            position_id: decPositionId || undefined,
            side: decSide,
            instrument: decInstrument,
            qty,
            price: fillPrice,
            fees,
            delta_notional: delta,
            premium_at_risk: premium,
            notes: decFillNotes.trim() || undefined,
          }
        : undefined;
      await postDecision({
        thesis_id: decThesisId || undefined,
        action: decAction,
        user_choice: decChoice,
        sizing: Object.keys(sizing).length > 0 ? sizing : undefined,
        manual_fill,
      });
      decStatus = "recorded ✓";
      setTimeout(() => (decStatus = null), 2500);
      resetFillForm();
      if (selectedSymbol) {
        const [theses, decisions, positions] = await Promise.all([
          fetchTheses(selectedSymbol).catch(() => symbolTheses ?? []),
          fetchDecisions(selectedSymbol).catch(() => symbolDecisions ?? []),
          fetchPositions(selectedSymbol).catch(() => symbolPositions ?? []),
        ]);
        symbolTheses = theses;
        symbolDecisions = decisions;
        symbolPositions = positions;
        await Promise.all([
          fetchTickers().then((t) => (tickers = t)).catch(() => {}),
          refreshAttention(),
          fetchBrainOverview().then((b) => (brainOverview = b)).catch(() => {}),
        ]);
      }
    } catch (err) {
      decStatus = `error: ${err}`;
    }
  }

  // ---------- bootstrap ----------
  function refreshAll() {
    fetchAlerts({ unacked: !showAcked }).then((a) => (alerts = a)).catch((e) => (error = String(e)));
    fetchRegime().then((r) => (regime = r)).catch((e) => (error = String(e)));
    fetchTickers().then((t) => (tickers = t)).catch((e) => (error = String(e)));
    fetchBrainOverview().then((b) => (brainOverview = b)).catch(() => {});
    fetchCalibration().then((c) => (calibration = c)).catch(() => {});
    refreshWatchlists();
    refreshPending();
    refreshAttention();
    fetchDiscoveryPool().then((p) => (pool = p)).catch(() => {});
  }

  $effect(() => {
    fetchAlerts({ unacked: !showAcked }).then((a) => (alerts = a)).catch(() => {});
  });

  async function refreshSysStatus() {
    try {
      const r = await fetch("/api/system-status");
      if (!r.ok) {
        sysStatusError = `HTTP ${r.status}`;
        return;
      }
      sysStatus = await r.json();
      sysStatusError = null;
    } catch (e) {
      sysStatusError = e instanceof Error ? e.message : String(e);
    }
  }
  // Poll while the diagnostics tab is open AND the drawer is expanded.
  $effect(() => {
    const shouldPoll = bottomMode === "diagnostics" && bottomOpen;
    if (shouldPoll) {
      void refreshSysStatus();
      sysStatusTimer = setInterval(refreshSysStatus, 30000);
      return () => {
        if (sysStatusTimer) { clearInterval(sysStatusTimer); sysStatusTimer = null; }
      };
    }
  });

  $effect(() => {
    // Once tickers and watchlists arrive, auto-pick the first symbol.
    if (!selectedSymbol && (tickers.length > 0 || watchlists.length > 0)) {
      pickFirstSymbol();
    }
  });
  // Auto-default the decision form's thesis ID to the selected symbol's
  // most recent open thesis — saves the operator from typing UUIDs.
  $effect(() => {
    if (symbolTheses && symbolTheses.length > 0) {
      const open = symbolTheses.find(
        (t) => !["closed", "disqualified"].includes(t.state),
      );
      if (open) decThesisId = open.thesis_id;
    }
  });

  $effect(() => {
    if (decAction === "enter" && decSide === "none") {
      if (decThesisDirection === "up") decSide = "long";
      if (decThesisDirection === "down") decSide = "short";
    }
  });

  onMount(() => {
    const routedSymbol = symbolFromRoute();
    if (routedSymbol) void selectSymbol(routedSymbol, { replaceRoute: true });
    refreshAll();
    const onPopState = () => {
      const routed = symbolFromRoute();
      if (routed) void selectSymbol(routed, { updateRoute: false });
    };
    window.addEventListener("popstate", onPopState);
    const stop = subscribe(
      (e) => {
        live = [e, ...live].slice(0, 200);
        if (e.subject?.startsWith("regime.")) {
          fetchRegime().then((r) => (regime = r)).catch(() => {});
        }
        if (e.kind === "state_transition" || e.kind === "risk") {
          fetchAlerts({ unacked: !showAcked }).then((a) => (alerts = a)).catch(() => {});
          fetchBrainOverview().then((b) => (brainOverview = b)).catch(() => {});
          refreshAttention();
        }
        if (e.subject?.startsWith("decision.") && selectedSymbol) {
          fetchDecisions(selectedSymbol).then((d) => (symbolDecisions = d)).catch(() => {});
          fetchPositions(selectedSymbol).then((p) => (symbolPositions = p)).catch(() => {});
        }
        // Discovery hits also produce attention items; refresh on any
        // discovery.* subject too.
        if (e.subject?.startsWith("discovery.")) {
          refreshAttention();
          refreshPending();
        }
      },
      (open) => (connected = open),
    );
    return () => {
      window.removeEventListener("popstate", onPopState);
      stop();
    };
  });

  let selectedTicker = $derived(tickerFor(selectedSymbol));
  let selectedParentTheses = $derived<BrainThesis[]>(
    brainOverview?.sectors.filter((thesis) =>
      selectedSymbol ? thesis.tickers.some((t) => t.symbol === selectedSymbol) : false,
    ) ?? [],
  );

  // ---------- panel sizing + resize ----------
  // paneforge bottom-pane API ref so the "hide" button can collapse it
  // imperatively. Sizes persist via PaneGroup autoSaveId — no manual
  // localStorage juggling.
  let bottomPane: { collapse: () => void; expand: () => void; isCollapsed: () => boolean } | null = null;
  function toggleBottom() {
    if (!bottomPane) return;
    if (bottomPane.isCollapsed()) bottomPane.expand();
    else bottomPane.collapse();
    bottomOpen = !bottomPane.isCollapsed();
  }
</script>

<div class="workspace">
  <!-- Top bar: symbol + regime + connection -->
  <header class="top">
    <div class="brand">stocks <span class="muted">intel</span></div>

    <div class="symbol-box">
      <input
        type="text"
        placeholder="Symbol…"
        value={selectedSymbol ?? ""}
        oninput={(e) => {
          const v = (e.target as HTMLInputElement).value.toUpperCase();
          if (v && tickers.some((t) => t.symbol === v)) selectSymbol(v);
        }}
        onkeydown={(e) => {
          if (e.key !== "Enter") return;
          const v = (e.target as HTMLInputElement).value;
          if (normalizeSymbol(v)) selectSymbol(v);
        }}
      />
      {#if selectedTicker}
        <span class="muted">T{selectedTicker.tier} · {selectedTicker.cluster_name ?? selectedTicker.cluster_id}</span>
      {/if}
    </div>

    <div class="regime" title={regime ? `as of ${regime.as_of ?? "?"}` : ""}>
      <span class="dot" style="background:{regimeColor(regime?.regime)}"></span>
      <strong>{regime?.regime ?? "loading…"}</strong>
      {#if regime?.capitulation}
        <span class="capitulation">CAPITULATION</span>
      {/if}
    </div>

    {#if calibration}
      <div class="calibration" title="Forward-only validation (SPEC §9). Brier=0 is perfect calibration; lead-time positive means alert preceded consensus.">
        <span class="muted">cal</span>
        <strong>{calibration.outcomes_scored}</strong>/<span class="muted">{calibration.predictions_total}</span>
        {#if calibration.mean_brier !== null}
          <span class="muted">brier</span>
          <strong>{calibration.mean_brier.toFixed(3)}</strong>
        {/if}
      </div>
    {/if}

    <span class="status" class:on={connected}>{connected ? "● live" : "○ offline"}</span>
  </header>

  {#if error}
    <div class="error error-bar">{error} <button class="x" onclick={() => (error = null)} aria-label="dismiss">✕</button></div>
  {/if}

  <!-- Body: left column (chart + bottom drawer stacked) + vertical splitter + right panel (full height) -->
  <PaneGroup direction="horizontal" autoSaveId="ws.v3.outer" class="body">
    <Pane defaultSize={72} minSize={40}>
      <PaneGroup direction="vertical" autoSaveId="ws.v3.left" class="main-col">
        <Pane defaultSize={70} minSize={30}>
          <ChartPanel symbol={selectedSymbol} />
        </Pane>

        <PaneResizer class="split-h" />

        <Pane
          bind:this={bottomPane}
          defaultSize={30}
          minSize={6}
          collapsible
          collapsedSize={5}
          onCollapse={() => (bottomOpen = false)}
          onExpand={() => (bottomOpen = true)}
        >
          <footer class="bottom">
    <nav class="bottom-tabs">
      {#each ["brain", "attention", "events", "discovery", "decisions", "calibration", "diagnostics"] as BottomMode[] as m}
        <button
          class:active={bottomMode === m}
          onclick={() => { bottomMode = m; if (!bottomOpen) bottomPane?.expand(); }}
        >
          {m}
          {#if m === "discovery" && pending.length > 0}<span class="badge tiny">{pending.length}</span>{/if}
          {#if m === "events"}<span class="badge tiny">{live.length}</span>{/if}
        </button>
      {/each}
      <button
        class="bottom-toggle"
        onclick={toggleBottom}
        title={bottomOpen ? "collapse drawer" : "expand drawer"}
      >
        {bottomOpen ? "▾ hide" : "▴ show"}
      </button>
    </nav>

    {#if bottomOpen}
      <div class="bottom-body">
        {#if bottomMode === "brain"}
          {#if brainOverview}
            <div class="brain-board">
              <section class="brain-topline">
                <div>
                  <strong>Brain</strong>
                  <span class="muted">
                    {brainOverview.summary.active_theses} active ·
                    {brainOverview.summary.stale_or_missing} stale/missing ·
                    {brainOverview.summary.open_nominations} nominations
                  </span>
                </div>
                {#if brainOverview.market_state}
                  <span class="badge tiny">market {brainOverview.market_state.regime}</span>
                {/if}
              </section>

              {#if brainOverview.macro}
                {@const macro = brainOverview.macro}
                {@const macroSources = brainSourceText(macro.source_ref)}
                <section class="brain-theme macro-theme freshness-{macro.freshness}">
                  <div class="brain-theme-hdr">
                    <div>
                      <strong>{macro.name}</strong>
                      <span class="muted">v{macro.version}</span>
                    </div>
                    <div class="brain-badges">
                      <span class="badge tiny brain-dir-{macro.direction}">{brainDirectionLabel(macro.direction)}</span>
                      <span class="badge tiny brain-fresh-{macro.freshness}">{macro.freshness}</span>
                      {#if macro.last_evaluated_at}<span class="muted">evaluated {relativeTime(macro.last_evaluated_at)}</span>{/if}
                    </div>
                  </div>
                  <p>{macro.summary}</p>
                  <p class="muted">{macro.core_claim}</p>
                  {#if macroSources}
                    <div class="brain-line">
                      <span class="muted">sources</span>
                      <span class="brain-token">{macroSources}</span>
                    </div>
                  {/if}
                  {#if macro.missing_evidence.length}
                    <div class="brain-line">
                      <span class="muted">missing</span>
                      {#each macro.missing_evidence.slice(0, 5) as item}
                        <span class="brain-token">{brainThingText(item)}</span>
                      {/each}
                    </div>
                  {/if}
                </section>
              {:else}
                <p class="muted">No macro thesis recorded.</p>
              {/if}

              {#if brainOverview.contradictions.length}
                <section class="brain-contradictions">
                  <strong>Contradictions</strong>
                  {#each brainOverview.contradictions as c}
                    <span class="badge tiny danger">{c.summary}</span>
                  {/each}
                </section>
              {/if}

              <div class="brain-theme-grid">
                {#each brainOverview.sectors as thesis (thesis.id)}
                  {@const coverage = brainCoverageText(thesis.source_ref)}
                  <section class="brain-theme freshness-{thesis.freshness}">
                    <div class="brain-theme-hdr">
                      <div>
                        <strong>{thesis.name}</strong>
                        <span class="muted">{thesis.scope} · {thesis.state}</span>
                      </div>
                      <div class="brain-badges">
                        <span class="badge tiny brain-dir-{thesis.direction}">{brainDirectionLabel(thesis.direction)}</span>
                        <span class="badge tiny brain-fresh-{thesis.freshness}">{thesis.freshness}</span>
                      </div>
                    </div>
                    <p>{thesis.summary}</p>
                    <p class="muted">{thesis.core_claim}</p>
                    {#if coverage}
                      <div class="brain-line">
                        <span class="muted">coverage</span>
                        <span class="brain-token">{coverage}</span>
                      </div>
                    {/if}

                    {#if thesis.watchlists.length}
                      <div class="brain-line">
                        <span class="muted">lists</span>
                        {#each thesis.watchlists.slice(0, 4) as w (w.id)}
                          <span class="brain-token" style={w.color ? `border-color: ${w.color}` : ""}>{w.name}</span>
                        {/each}
                      </div>
                    {/if}

                    {#if thesis.tickers.length}
                      <div class="brain-tickers">
                        {#each thesis.tickers.slice(0, 12) as t (`${thesis.id}-${t.symbol}`)}
                          <button type="button" class="brain-ticker" onclick={() => selectSymbol(t.symbol)}>
                            <strong>{t.symbol}</strong>
                            <span>{t.role}</span>
                            {#if t.thesis_state}
                              <span class="wl-thesis-state">{thesisStatusLabel(t.thesis_state)}</span>
                            {/if}
                            <span class={`badge tiny ${thesisDirectionClass(t.thesis_direction)}`}>
                              {thesisDirectionLabel(t.thesis_direction)}
                            </span>
                          </button>
                        {/each}
                      </div>
                    {/if}

                    {#if thesis.nominations.length}
                      <div class="brain-line">
                        <span class="muted">queued</span>
                        {#each thesis.nominations.slice(0, 4) as n (n.candidate_id)}
                          <button type="button" class="brain-token action" onclick={() => selectSymbol(n.symbol)}>
                            {n.symbol} · {n.signal_name.replace(/_/g, " ")}
                          </button>
                        {/each}
                      </div>
                    {/if}

                    {#if thesis.missing_evidence.length || thesis.open_questions.length}
                      <ul class="brain-gaps">
                        {#each [...thesis.missing_evidence, ...thesis.open_questions].slice(0, 4) as item}
                          <li>{brainThingText(item)}</li>
                        {/each}
                      </ul>
                    {/if}
                  </section>
                {/each}
              </div>
            </div>
          {:else}
            <p class="muted">Loading brain…</p>
          {/if}
        {:else if bottomMode === "attention"}
          <div class="att-toolbar">
            <span class="muted">{groupedAttention.length} pending</span>
            <span class="att-filters">
              {#each ["all", "candidate_review", "thesis_actionable", "risk_review"] as f}
                <button class:active={attentionFilter === f} onclick={() => (attentionFilter = f)}>
                  {f === "all" ? "all" : f.replace(/_/g, " ")}
                </button>
              {/each}
            </span>
            <button class="reset" onclick={refreshAttention} title="reload">⟲</button>
          </div>
          {#if groupedAttention.length === 0}
            <p class="muted">No open attention. The system is quiet.</p>
          {:else}
            {@const groups = groupedAttention.filter((g) => attentionFilter === "all" || g.kind === attentionFilter)}
            {#if groups.length === 0}
              <p class="muted">No attention matches this filter.</p>
            {:else}
              {#each attentionSections(groups) as section (section.key)}
                <section class="att-section">
                  <div class="att-section-head">
                    <strong>{attentionStateLabel(section.fsmState)}</strong>
                    <span class="muted">{attentionOwnerLabel(section.owner)}</span>
                    <span class="badge tiny">{section.groups.length}</span>
                  </div>
                  <ul class="att-list">
              {#each section.groups as g (g.key)}
                {@const ticker = g.symbol ? tickers.find((t) => t.symbol === g.symbol) : undefined}
                {@const poolMeta = g.symbol ? pool.find((p) => p.symbol === g.symbol) : undefined}
                {@const tierLabel = ticker ? `T${ticker.tier}` : (poolMeta ? "pool" : "")}
                {@const reasonMap = (() => {
                  // Dedupe bullets by composed interpretation. Raw detector
                  // names are kept in source_ref.raw_signals for audit.
                  const seen = new Map<string, string>();
                  for (const it of g.items) {
                    let key: string, text: string;
                    if (g.kind === "candidate_review") {
                      const pc = pending.find((p) => p.id === it.candidate_id);
                      const sig = pc?.signal_name
                        ?? (it.title.match(/via (\w+)$/)?.[1])
                        ?? "signal";
                      key = `${it.source_ref?.interpretation_kind ?? sig}`;
                      text = displayReason(it.reason ?? pc?.reasoning ?? reasonFor(sig, pc?.signal_value ?? null));
                    } else {
                      text = displayReason(it.reason ?? it.title);
                      key = text;
                    }
                    if (!seen.has(key)) seen.set(key, text);
                  }
                  return seen;
                })()}
                {@const reasons = [...reasonMap.values()]}
                {@const interpretations = reasonMap.size}
                {@const rawInputCount = new Set(g.items.flatMap(rawSignals)).size}
                {@const deferred = g.items.find((it) => it.fsm_state === "operator_deferred")}
                <li class="att-card sev-{g.severity}">
                  <div class="att-row1">
                    {#if g.symbol}
                      <strong
                        class="att-symbol link-symbol"
                        onclick={() => g.symbol && selectSymbol(g.symbol)}
                      >{g.symbol}</strong>
                      <span class="att-tier muted">{tierLabel}</span>
                    {/if}
                    <span class="badge tiny state-{g.fsmState}">{attentionStateLabel(g.fsmState)}</span>
                    <span class="badge tiny owner-{g.owner}">{g.owner}</span>
                    <span class="att-time muted">{relativeTime(g.latestAt)}</span>
                  </div>
                  <div class="att-status muted">
                    {#if g.kind === "candidate_review"}
                      discovery review · {interpretations} interpretation{interpretations === 1 ? "" : "s"}
                      {#if rawInputCount > 0}
                        · {rawInputCount} raw input{rawInputCount === 1 ? "" : "s"}
                      {/if}
                    {:else if g.kind === "thesis_actionable"}
                      thesis ready
                    {:else if g.kind === "risk_review"}
                      ⛔ risk · {g.severity}
                    {:else if g.kind === "thesis_incomplete"}
                      system declined to draft thesis
                    {:else}
                      {g.kind.replace(/_/g, " ")}
                    {/if}
                    {#if deferred?.resurface_at}
                      · resurfaced {relativeTime(deferred.resurface_at)}
                    {/if}
                    {#if g.nextRetryAt}
                      · retry {relativeTime(g.nextRetryAt)}
                    {/if}
                    {#if g.resurfaceAt}
                      · resurface {relativeTime(g.resurfaceAt)}
                    {/if}
                    {#if g.stateReason}
                      · {g.stateReason.replace(/_/g, " ")}
                    {/if}
                  </div>

                  <ul class="att-reasons">
                    {#each reasons as text}
                      <li>• {text}</li>
                    {/each}
                  </ul>

                  {#if g.kind === "candidate_review"}
                    {@const allLists = [...new Map(
                      g.candidateIds
                        .flatMap((cid) => (pending.find((p) => p.id === cid)?.proposed_lists ?? [])
                          .filter((p) => p.watchlist_id)
                          .map((p) => [p.watchlist_id, p]))
                    ).values()]}
                    {#if allLists.length > 0}
                      <div class="att-fits">
                        <span class="muted">Fits →</span>
                        {#each allLists as p}
                          {#if p.watchlist_id}
                            <label class="att-pick">
                              <input
                                type="checkbox"
                                checked={g.candidateIds.some((cid) => chosenLists[cid]?.[p.watchlist_id!])}
                                onchange={() => {
                                  if (!p.watchlist_id) return;
                                  const target = !g.candidateIds.every((cid) => chosenLists[cid]?.[p.watchlist_id!]);
                                  for (const cid of g.candidateIds) {
                                    const inner = { ...(chosenLists[cid] ?? {}) };
                                    inner[p.watchlist_id!] = target;
                                    chosenLists = { ...chosenLists, [cid]: inner };
                                  }
                                }}
                              />
                              {p.watchlist_name}
                              <span class="badge tiny conf-{p.confidence}">{p.confidence}</span>
                            </label>
                          {/if}
                        {/each}
                      </div>
                    {/if}
                  {/if}

                  <div class="att-actions">
                    {#if g.kind === "candidate_review"}
                      <button class="confirm" onclick={() => confirmGroup(g.candidateIds)}>Confirm</button>
                      <button class="reject" onclick={() => (rejectOpenFor = rejectOpenFor === g.key ? null : g.key)}>
                        Reject ▾
                      </button>
                    {:else if g.kind === "thesis_actionable"}
                      <button class="confirm" onclick={() => {
                        const tid = g.items[0]?.thesis_id;
                        if (tid) {
                          decThesisId = tid;
                          decAction = "enter";
                          decChoice = "confirmed";
                          decRecordFill = true;
                          bottomMode = "decisions";
                        }
                      }}>Enter ▾</button>
                      <button class="reject" onclick={() => g.items.forEach((it) => deferOne(it.id))}>Defer 7d</button>
                      <button class="reject" onclick={() => g.items.forEach((it) => dismissOne(it.id, "skip"))}>Skip</button>
                    {:else if g.kind === "risk_review"}
                      <button class="confirm" onclick={() => g.items.forEach((it) => dismissOne(it.id, "ack"))}>Acknowledge</button>
                    {:else}
                      <button class="reject" onclick={() => g.items.forEach((it) => dismissOne(it.id))}>Dismiss</button>
                    {/if}
                  </div>

                  {#if rejectOpenFor === g.key}
                    <div class="att-reject-menu">
                      <span class="muted">why?</span>
                      {#each REJECT_REASONS as r}
                        <button class="reject-reason" onclick={() => {
                          rejectGroup(g.candidateIds, r);
                          rejectOpenFor = null;
                        }}>{r.replace(/_/g, " ")}</button>
                      {/each}
                    </div>
                  {/if}
                </li>
              {/each}
                  </ul>
                </section>
              {/each}
            {/if}
          {/if}
        {:else if bottomMode === "events"}
          {#if live.length === 0}
            <p class="muted">Waiting for events…</p>
          {:else}
            <ul class="event-feed">
              {#each live.slice(0, 80) as e, i (i)}
                {@const p = (e.payload ?? {}) as Record<string, unknown>}
                <li
                  onclick={() => p.symbol && selectSymbol(p.symbol as string)}
                  class:linkable={!!p.symbol}
                >
                  <span class="kind" style="color:{kindColor(e.kind, p)}">{e.kind}</span>
                  <code>{e.subject}</code>
                  {#if p.symbol}<strong>{p.symbol as string}</strong>{/if}
                  {#if e.kind === "risk" && p.veto}<span class="badge danger tiny">VETO {(p.reasons as string[])?.join(", ")}</span>{/if}
                </li>
              {/each}
            </ul>
          {/if}
        {:else if bottomMode === "discovery"}
          {#if pending.length === 0}
            <p class="muted">Nothing pending. Run <code>make run-discovery</code> + <code>make classify-candidates</code>.</p>
          {:else}
            <ul class="disc-list">
              {#each pending as c (c.id)}
                <li class="disc-card">
                  <div class="disc-hdr">
                    <strong class="link-symbol" onclick={() => selectSymbol(c.symbol)}>{c.symbol}</strong>
                    {#if c.rank_bucket}
                      <span class="badge tiny rank-{c.rank_bucket}">
                        {c.rank_bucket} {Math.round(c.rank_score ?? 0)}
                      </span>
                    {/if}
                    <span class="badge tiny">{c.signal_name}</span>
                    {#if c.signal_value !== null}<span class="muted">value {c.signal_value.toFixed(3)}</span>{/if}
                    <span class="muted">{shortTs(c.proposed_at)}</span>
                  </div>
                  {#if c.reasoning}<p class="muted disc-reasoning">{displayReason(c.reasoning)}</p>{/if}
                  {#if c.rank_reasons?.length}
                    <p class="muted disc-rank">{c.rank_reasons.join(" · ")}</p>
                  {/if}
                  {#if c.proposed_lists.length > 0}
                    <div class="disc-lists">
                      {#each c.proposed_lists as p}
                        {#if p.watchlist_id}
                          <label class="disc-pick">
                            <input
                              type="checkbox"
                              checked={chosenLists[c.id]?.[p.watchlist_id] ?? false}
                              onchange={() => p.watchlist_id && toggleChoice(c.id, p.watchlist_id)}
                            />
                            <span>{p.watchlist_name}</span>
                            <span class="badge tiny conf-{p.confidence}">{p.confidence}</span>
                            <span class="muted disc-rat">{p.rationale}</span>
                          </label>
                        {/if}
                      {/each}
                    </div>
                  {/if}
                  {#if c.suggested_new_list}
                    <div class="disc-newlist">
                      <span class="badge tiny">propose new</span>
                      <strong>{c.suggested_new_list.name}</strong>
                      <span class="muted">— {c.suggested_new_list.description}</span>
                    </div>
                  {/if}
                  <div class="disc-actions">
                    <button onclick={() => confirmOne(c.id)}>Confirm</button>
                    <button class="reject" onclick={() => rejectOne(c.id)}>Reject</button>
                  </div>
                </li>
              {/each}
            </ul>
          {/if}
        {:else if bottomMode === "decisions"}
          <form onsubmit={submitDecision} class="decform">
            <label>
              Thesis ID
              <input bind:value={decThesisId} placeholder="(leave blank for ad-hoc)" />
            </label>
            {#if decThesisDirection}
              <span class="decision-context thesis-{decThesisDirection}">
                thesis {decThesisDirection}
              </span>
            {/if}
            <label>
              Action
              <select bind:value={decAction}>
                <option value="enter">enter thesis</option>
                <option value="exit">exit position</option>
                <option value="skip">skip</option>
                <option value="resize">resize</option>
              </select>
            </label>
            <label>
              Side
              <select bind:value={decSide}>
                <option value="none">choose side…</option>
                <option value="long">long common</option>
                <option value="short">short common</option>
                <option value="call">calls / call spread</option>
                <option value="put">puts / put spread</option>
                <option value="hedge">hedge</option>
              </select>
            </label>
            <label>
              Instrument
              <select bind:value={decInstrument}>
                <option value="equity">equity</option>
                <option value="leaps">LEAPS</option>
                <option value="options">options</option>
              </select>
            </label>
            <label>
              User choice
              <select bind:value={decChoice}>
                <option>confirmed</option><option>rejected</option><option>deferred</option>
              </select>
            </label>
            <label class="checkline">
              <input type="checkbox" bind:checked={decRecordFill} />
              <span>record manual fill</span>
            </label>
            {#if decRecordFill}
              {#if decAction === "exit" && openSymbolPositions.length > 0}
                <label>
                  Position
                  <select bind:value={decPositionId} onchange={() => {
                    const p = openSymbolPositions.find((x) => x.position_id === decPositionId);
                    if (p) {
                      decSide = p.side;
                      decInstrument = p.instrument;
                      decQty = String(p.qty);
                      decPrice = p.latest_price ? String(p.latest_price) : decPrice;
                    }
                  }}>
                    <option value="">latest open position</option>
                    {#each openSymbolPositions as p (p.position_id)}
                      <option value={p.position_id}>{p.side} {p.instrument} · {p.qty} @ {price(p.avg_price)}</option>
                    {/each}
                  </select>
                </label>
              {/if}
              <label>
                Qty
                <input type="number" min="0" step="any" bind:value={decQty} />
              </label>
              <label>
                Fill price
                <input type="number" min="0" step="any" bind:value={decPrice} />
              </label>
              <label>
                Fees
                <input type="number" min="0" step="any" bind:value={decFees} />
              </label>
              <label>
                Delta notional
                <input type="number" min="0" step="any" bind:value={decDeltaNotional} placeholder="auto for equity" />
              </label>
              <label>
                Premium at risk
                <input type="number" min="0" step="any" bind:value={decPremiumAtRisk} placeholder="auto for options" />
              </label>
              <label class="wide">
                Notes
                <input bind:value={decFillNotes} placeholder="fill source, broker note, reason" />
              </label>
            {/if}
            <button type="submit">Submit</button>
            {#if decStatus}<span class="muted">{decStatus}</span>{/if}
          </form>
        {:else if bottomMode === "calibration"}
          {#if calibration}
            <dl class="meta-list inline">
              <dt>Predictions</dt><dd>{calibration.predictions_total}</dd>
              <dt>Scored outcomes</dt><dd>{calibration.outcomes_scored}</dd>
              {#if calibration.mean_brier !== null}
                <dt>Mean Brier</dt><dd>{calibration.mean_brier.toFixed(4)}</dd>
              {/if}
              {#if calibration.median_lead_time_days !== null}
                <dt>Median lead</dt><dd>{calibration.median_lead_time_days.toFixed(1)}d</dd>
              {/if}
            </dl>
          {:else}
            <p class="muted">No calibration data yet.</p>
          {/if}
        {:else if bottomMode === "diagnostics"}
          {#if sysStatus}
            {@const ing = (sysStatus.ingest ?? {}) as Record<string, { last_at: string|null; count_24h: number; symbols_24h?: number }>}
            {@const disc = sysStatus.discovery as { last_pass_at: string|null; open_candidates: number; by_signal: { signal: string; count: number }[]; pool_size: number }}
            {@const cog = sysStatus.cognition as { contexts_24h: number; contexts_total_symbols: number; thesis_by_state: { state: string; count: number }[] }}
            {@const ev = sysStatus.evidence as { open_requirements: number; by_state: { state: string; count: number }[]; by_reason?: { reason: string; count: number }[] }}
            {@const att = sysStatus.attention as { open_items: number; deferred_items?: number; by_kind: { kind: string; count: number }[]; by_state?: { state: string; count: number }[]; by_owner?: { owner: string; count: number }[] }}
            {@const llm = sysStatus.llm as { calls_24h: number; avg_latency_ms: number|null; by_prompt: { prompt: string; count: number; avg_ms: number|null; last_at: string|null }[] }}
            {@const health = (sysStatus.source_health ?? []) as { source: string; last_status: string; last_started_at: string|null; last_success_at: string|null; last_failure_at: string|null; last_failure_kind?: string|null; last_error?: string|null; retry_after_at?: string|null; rows_seen: number; rows_inserted: number; symbols_attempted: number; symbols_failed: number }[]}
            {@const priceFresh = sysStatus.price_freshness as { expected_latest_session?: string|null; actual_latest_session?: string|null; symbols_total?: number; symbols_fresh?: number; status?: string }}
            <div class="diag-grid">
              <section class="diag">
                <h5>Stored events <span class="muted">— new rows / 24h</span></h5>
                <table class="diag-tbl">
                  <thead><tr><th>table/feed</th><th>last new row</th><th>new rows</th><th>symbols</th></tr></thead>
                  <tbody>
                    {#each Object.entries(ing) as [src, v] (src)}
                      <tr>
                        <td><strong>{src}</strong></td>
                        <td class="muted">{v.last_at ? relativeTime(v.last_at) : "—"}</td>
                        <td>{v.count_24h}</td>
                        <td>{v.symbols_24h ?? "—"}</td>
                      </tr>
                    {/each}
                  </tbody>
                </table>
              </section>

              <section class="diag wide">
                <h5>Source health</h5>
                <table class="diag-tbl">
                  <thead><tr><th>source</th><th>status</th><th>last result</th><th>checked rows</th><th>new rows</th><th>symbols</th><th>retry</th></tr></thead>
                  <tbody>
                    {#each health as h (h.source)}
                      <tr title={h.last_error ?? ""}>
                        <td><strong>{h.source}</strong></td>
                        <td><span class={`badge tiny health-${h.last_status}`}>{healthLabel(h.last_status, h.last_failure_kind)}</span></td>
                        <td class="muted">{h.last_success_at ? relativeTime(h.last_success_at) : "—"}</td>
                        <td>{h.last_status === "running" && !h.last_success_at ? "checking" : h.rows_seen}</td>
                        <td>{h.last_status === "running" && !h.last_success_at ? "—" : h.rows_inserted}</td>
                        <td>{h.symbols_attempted - h.symbols_failed}/{h.symbols_attempted}</td>
                        <td class="muted">{h.retry_after_at ? relativeTime(h.retry_after_at) : "—"}</td>
                      </tr>
                    {/each}
                  </tbody>
                </table>
              </section>

              <section class="diag">
                <h5>Price freshness</h5>
                <dl class="meta-list inline">
                  <dt>expected</dt><dd>{priceFresh?.expected_latest_session ?? "—"}</dd>
                  <dt>latest</dt><dd>{priceFresh?.actual_latest_session ?? "—"}</dd>
                  <dt>symbols fresh</dt><dd>{priceFresh?.symbols_fresh ?? 0}/{priceFresh?.symbols_total ?? 0}</dd>
                  <dt>status</dt><dd>{priceFresh?.status ?? "—"}</dd>
                </dl>
              </section>

              <section class="diag">
                <h5>Discovery</h5>
                <dl class="meta-list inline">
                  <dt>last pass</dt><dd>{disc.last_pass_at ? relativeTime(disc.last_pass_at) : "—"}</dd>
                  <dt>open candidates</dt><dd>{disc.open_candidates}</dd>
                  <dt>pool size</dt><dd>{disc.pool_size}</dd>
                </dl>
                {#if disc.by_signal?.length}
                  <ul class="chips">
                    {#each disc.by_signal as s (s.signal)}
                      <li class="chip">{s.signal}: <strong>{s.count}</strong></li>
                    {/each}
                  </ul>
                {/if}
              </section>

              <section class="diag">
                <h5>Cognition</h5>
                <dl class="meta-list inline">
                  <dt>contexts (24h)</dt><dd>{cog.contexts_24h}</dd>
                  <dt>symbols with context</dt><dd>{cog.contexts_total_symbols}</dd>
                </dl>
                {#if cog.thesis_by_state?.length}
                  <ul class="chips">
                    {#each cog.thesis_by_state as s (s.state)}
                      <li class="chip">{s.state}: <strong>{s.count}</strong></li>
                    {/each}
                  </ul>
                {/if}
              </section>

              <section class="diag">
                <h5>Evidence</h5>
                <dl class="meta-list inline">
                  <dt>open requirements</dt><dd>{ev.open_requirements}</dd>
                </dl>
                {#if ev.by_state?.length}
                  <ul class="chips">
                    {#each ev.by_state as s (s.state)}
                      <li class="chip">{s.state}: <strong>{s.count}</strong></li>
                    {/each}
                  </ul>
                {/if}
                {#if ev.by_reason?.length}
                  <ul class="chips">
                    {#each ev.by_reason as s (s.reason)}
                      <li class="chip">{s.reason.replace(/_/g, " ")}: <strong>{s.count}</strong></li>
                    {/each}
                  </ul>
                {/if}
              </section>

              <section class="diag">
                <h5>Attention</h5>
                <dl class="meta-list inline">
                  <dt>visible</dt><dd>{att.open_items}</dd>
                  <dt>deferred</dt><dd>{att.deferred_items ?? 0}</dd>
                </dl>
                {#if att.by_kind?.length}
                  <ul class="chips">
                    {#each att.by_kind as k (k.kind)}
                      <li class="chip">{k.kind}: <strong>{k.count}</strong></li>
                    {/each}
                  </ul>
                {/if}
                {#if att.by_state?.length}
                  <ul class="chips">
                    {#each att.by_state as s (s.state)}
                      <li class="chip">{s.state}: <strong>{s.count}</strong></li>
                    {/each}
                  </ul>
                {/if}
                {#if att.by_owner?.length}
                  <ul class="chips">
                    {#each att.by_owner as o (o.owner)}
                      <li class="chip">{o.owner}: <strong>{o.count}</strong></li>
                    {/each}
                  </ul>
                {/if}
              </section>

              <section class="diag wide">
                <h5>LLM <span class="muted">— {llm.calls_24h} calls / 24h · avg {llm.avg_latency_ms ?? "—"}ms</span></h5>
                {#if llm.by_prompt?.length}
                  <table class="diag-tbl">
                    <thead><tr><th>prompt</th><th>calls</th><th>avg ms</th><th>last</th></tr></thead>
                    <tbody>
                      {#each llm.by_prompt as p (p.prompt)}
                        <tr>
                          <td><code>{p.prompt}</code></td>
                          <td>{p.count}</td>
                          <td>{p.avg_ms ?? "—"}</td>
                          <td class="muted">{p.last_at ? relativeTime(p.last_at) : "—"}</td>
                        </tr>
                      {/each}
                    </tbody>
                  </table>
                {/if}
              </section>
            </div>
            <p class="muted hint">Auto-refreshes every 30s while this tab is open.</p>
          {:else if sysStatusError}
            <p class="err">Failed to load: {sysStatusError}</p>
          {:else}
            <p class="muted">Loading…</p>
          {/if}
        {/if}
      </div>
    {/if}
          </footer>
        </Pane>
      </PaneGroup>
    </Pane>

    <PaneResizer class="split-v" />

    <Pane defaultSize={28} minSize={18} maxSize={50}>
      <aside class="right">
      <!-- Watchlists nav -->
      <section class="wl-section">
        <div class="wl-hdr">
          <h3>Watchlists</h3>
        </div>
        <form onsubmit={submitNewList} class="wl-new">
          <input bind:value={newListName} placeholder="+ new list" />
          <button type="submit" disabled={!newListName.trim()}>add</button>
        </form>
        <ul class="wl-list">
          {#each allWatchlists as w (w.id)}
            {@const open = expandedListIds[w.id] ?? false}
            {@const members = membersFor(w.id)}
            <li class="wl-item">
              <button type="button" class="wl-row" onclick={() => toggleListExpanded(w.id)}>
                <span class="caret">{open ? "▾" : "▸"}</span>
                <span class="wl-name" style={w.color ? `border-left: 3px solid ${w.color}; padding-left: .35rem` : ""}>{w.name}</span>
                <span class="muted">{w.member_count}</span>
                {#if w.is_system}<span class="badge tiny">sys</span>{/if}
              </button>
              {#if open}
                {#if w.id !== UNIVERSE_ID && w.id !== POOL_ID}
                  <form
                    onsubmit={(e) => { e.preventDefault(); addMember(w.id); }}
                    class="wl-add-sym"
                  >
                    <input
                      placeholder="+ AAPL"
                      value={addSymbolFor[w.id] ?? ""}
                      oninput={(e) => addSymbolFor = { ...addSymbolFor, [w.id]: (e.target as HTMLInputElement).value }}
                    />
                  </form>
                {/if}
                <ul class="wl-members">
                  {#each members as m (m.symbol)}
                    <li
                      class="wl-mem"
                      class:active={selectedSymbol === m.symbol}
                    >
                      <button type="button" class="wl-mem-select" onclick={() => selectSymbol(m.symbol)}>
                        <strong>{m.symbol}</strong>
                        <span class="wl-thesis-state" class:empty={!m.thesis_state}>
                          {thesisStatusLabel(m.thesis_state)}
                        </span>
                        <span class={`badge tiny ${thesisDirectionClass(m.thesis_direction)}`}>
                          {thesisDirectionLabel(m.thesis_direction)}
                        </span>
                      </button>
                      {#if w.id !== UNIVERSE_ID && w.id !== POOL_ID}
                        <button
                          class="rm"
                          onclick={(e) => { e.stopPropagation(); removeMember(w.id, m.symbol); }}
                          title="remove from {w.name}"
                          aria-label="remove"
                        >×</button>
                      {/if}
                    </li>
                  {/each}
                  {#if members.length === 0}
                    <li class="muted wl-empty">empty</li>
                  {/if}
                </ul>
              {/if}
            </li>
          {/each}
        </ul>
      </section>

      <!-- Selected-symbol detail tabs -->
      <section class="detail-section">
        {#if selectedSymbol}
          <nav class="tabs">
            {#each ["overview", "context", "evidence", "theses", "alerts", "decisions"] as RightTab[] as t}
              <button class:active={rightTab === t} onclick={() => (rightTab = t)}>{t}</button>
            {/each}
          </nav>
          <div class="tab-body">
            {#if rightTab === "overview"}
              {#if selectedTicker}
                <dl class="meta-list">
                  <dt>Symbol</dt><dd><strong>{selectedTicker.symbol}</strong></dd>
                  <dt>Cluster</dt><dd>{selectedTicker.cluster_name ?? selectedTicker.cluster_id}</dd>
                  <dt>Tier</dt><dd>T{selectedTicker.tier}</dd>
                  <dt>Domain fit</dt><dd>{selectedTicker.domain_fit !== null && selectedTicker.domain_fit !== undefined ? Math.round(selectedTicker.domain_fit) : "—"}</dd>
                  <dt>Options</dt><dd>{selectedTicker.options_eligible ? "yes" : "no"}</dd>
                  <dt>Open theses</dt><dd>{selectedTicker.open_theses}</dd>
                </dl>
              {:else}
                <p class="muted">Ticker metadata not loaded yet.</p>
              {/if}
              {#if symbolBrain === undefined}
                <p class="muted">Loading brain status…</p>
              {:else if symbolBrain}
                <section class="brain-card brain-{symbolBrain.status}">
                  <div class="brain-hdr">
                    <span class="brain-title">Brain</span>
                    <span class="badge tiny brain-{symbolBrain.status}">
                      {brainStatusLabel(symbolBrain.status)}
                    </span>
                    <strong>{brainActionLabel(symbolBrain.next_action)}</strong>
                  </div>
                  <p>{symbolBrain.reason}</p>
                  <dl class="meta-list inline">
                    <dt>evidence</dt><dd>{symbolBrain.evidence.rows} rows, {symbolBrain.evidence.open} open</dd>
                    <dt>attention</dt><dd>{symbolBrain.attention.open} open</dd>
                    <dt>target</dt><dd>{symbolBrain.freshness_target_minutes}m</dd>
                  </dl>
                  <ul class="brain-sources">
                    {#each symbolBrain.sources as s (s.source)}
                      <li title={s.last_error ?? ""}>
                        <strong>{sourceLabel(s.source)}</strong>
                        <span class="badge tiny brain-source-{s.status}">
                          {healthLabel(s.status, s.failure_kind)}
                        </span>
                        <span class="muted">{sourceTime(s)}</span>
                      </li>
                    {/each}
                  </ul>
                </section>
              {:else}
                <p class="muted">Brain status unavailable.</p>
              {/if}
              {#if selectedParentTheses.length}
                <section class="brain-card parent-brain-card">
                  <div class="brain-hdr">
                    <span class="brain-title">Parent Brain</span>
                    <span class="badge tiny">{selectedParentTheses.length}</span>
                    <button type="button" class="text-action" onclick={openBrainDrawer}>open brain</button>
                  </div>
                  <ul class="parent-brain-list">
                    {#each selectedParentTheses as parent (parent.id)}
                      {@const linked = parent.tickers.find((t) => t.symbol === selectedSymbol)}
                      <li>
                        <div class="parent-brain-hdr">
                          <strong>{parent.name}</strong>
                          <span class="badge tiny brain-dir-{parent.direction}">{brainDirectionLabel(parent.direction)}</span>
                          <span class="badge tiny brain-fresh-{parent.freshness}">{parent.freshness}</span>
                          {#if linked?.role}<span class="muted">{linked.role}</span>{/if}
                        </div>
                        <p>{parent.summary}</p>
                        {#if linked?.rationale}
                          <p class="muted">{linked.rationale}</p>
                        {/if}
                        {#if parent.open_questions.length}
                          <div class="parent-brain-questions">
                            {#each parent.open_questions.slice(0, 2) as question}
                              <span>{brainThingText(question)}</span>
                            {/each}
                          </div>
                        {/if}
                      </li>
                    {/each}
                  </ul>
                </section>
              {/if}
            {:else if rightTab === "context"}
              {#if symbolContext === undefined}
                <p class="muted">Loading…</p>
              {:else}
                <ContextPanel ctx={symbolContext ?? null} symbol={selectedSymbol} />
              {/if}
            {:else if rightTab === "evidence"}
              {#if symbolEvidence === undefined}
                <p class="muted">Loading…</p>
              {:else if symbolEvidence.length === 0}
                <p class="muted">Evidence checklist pending initialization for <strong>{selectedSymbol}</strong>.</p>
              {:else}
                <ul class="evidence-list">
                  {#each symbolEvidence as req (req.id)}
                    <li class="evidence-card state-{req.blocking_state}">
                      <div class="evidence-row">
                        <strong>{req.source_type.replace(/_/g, " ")}</strong>
                        <span class="badge tiny priority-{req.priority}">{evidencePriorityLabel(req.priority)}</span>
                        <span class="badge tiny">{req.blocking_state}</span>
                        {#if req.next_retry_at}<span class="muted">retry {relativeTime(req.next_retry_at)}</span>{/if}
                      </div>
                      <p>{req.reason}</p>
                      {#if req.blocking_state === "satisfied"}
                        {#if evidenceRequirementCount(req)}
                          <p class="muted">{evidenceRequirementCount(req)}</p>
                        {/if}
                      {:else if evidenceActions(req).length}
                        <p class="muted">next fetch: {evidenceActions(req).map((a) => a.replace(/_/g, " ")).join(", ")}</p>
                      {/if}
                      {#if evidenceSourceTasks(req)}
                        <p class="muted">source tasks: {evidenceSourceTasks(req)}</p>
                      {/if}
                      {#if req.blocking_state !== "satisfied" && evidenceCounts(req)}
                        <p class="muted">{evidenceCounts(req)}</p>
                      {/if}
                      {#if evidenceHealth(req)}
                        <p class="muted">{evidenceHealth(req)}</p>
                      {/if}
                      {#if req.last_error}<p class="error-text">{req.last_error}</p>{/if}
                    </li>
                  {/each}
                </ul>
              {/if}
              {#if symbolResearch === undefined}
                <p class="muted">Loading research sources…</p>
              {:else if symbolResearch.length > 0}
                <section class="research-sources">
                  <h4>Research sources</h4>
                  <ul class="evidence-list">
                    {#each symbolResearch as src (src.id)}
                      <li class="evidence-card">
                        <div class="evidence-row">
                          <a href={src.url} target="_blank" rel="noreferrer">{src.title}</a>
                          <span class="badge tiny">{src.credibility}</span>
                        </div>
                        <p class="muted">
                          {(src.publisher ?? src.provider)} ·
                          {src.published_at ? relativeTime(src.published_at) : `retrieved ${relativeTime(src.retrieved_at)}`}
                        </p>
                        <p class="muted">{src.query}</p>
                      </li>
                    {/each}
                  </ul>
                </section>
              {/if}
            {:else if rightTab === "theses"}
              {#if symbolTheses === undefined || symbolDeclines === undefined}
                <p class="muted">Loading…</p>
              {:else}
                {#if symbolTheses && symbolTheses.length > 0}
                  {#if activeThesisDirections.length > 1}
                    <p class="decision-warning">
                      Conflicting open thesis directions: {activeThesisDirections.join(" / ")}.
                      Do not treat this symbol as a single clean signal until one thesis is selected or retired.
                    </p>
                  {/if}
                  {#if currentSymbolThesis}
                    <ThesisDetails thesis={currentSymbolThesis} />
                  {:else}
                    <p class="muted">No open thesis for <strong>{selectedSymbol}</strong>.</p>
                  {/if}
                  {#if retiredSymbolTheses.length > 0}
                    <section class="declines">
                      <h4>Retired thesis history</h4>
                      <ul>
                        {#each retiredSymbolTheses as t (t.thesis_id)}
                          {@const dir = forecastDirectionFrom(t.forecast)}
                          <li class="decline-card status-{t.state}">
                            <div class="decline-hdr">
                              <span class="badge tiny">{t.state.replace(/_/g, " ")}</span>
                              {#if dir}<span class={`badge tiny ${thesisDirectionClass(dir)}`}>{thesisDirectionLabel(dir)}</span>{/if}
                              <span class="muted">v{t.version}</span>
                              <span class="muted">updated {shortTs(t.updated_at)}</span>
                            </div>
                            <p>{t.edge_rationale}</p>
                          </li>
                        {/each}
                      </ul>
                    </section>
                  {/if}
                {/if}
                {#if symbolDeclines && symbolDeclines.length > 0}
                  <section class="declines">
                    <h4>Declined thesis attempts</h4>
                    <ul>
                      {#each symbolDeclines as d (d.id)}
                        <li class="decline-card status-{d.status}">
                          <div class="decline-hdr">
                            <span class="badge tiny">{d.status}</span>
                            {#if d.resolution_kind}<span class="muted">{d.resolution_kind}</span>{/if}
                            <span class="muted">{shortTs(d.created_at)}</span>
                          </div>
                          <p>{d.reason ?? "The thesis engine declined without a recorded reason."}</p>
                        </li>
                      {/each}
                    </ul>
                  </section>
                {/if}
                {#if (!symbolTheses || symbolTheses.length === 0) && (!symbolDeclines || symbolDeclines.length === 0)}
                  <p class="muted">
                    No thesis attempts for <strong>{selectedSymbol}</strong> yet.
                    The system should either draft a monitoring thesis or show a
                    declined attempt with a reason.
                  </p>
                {/if}
              {/if}
            {:else if rightTab === "alerts"}
              <div class="alert-toolbar">
                <label class="toggle"><input type="checkbox" bind:checked={showAcked} /> show acked</label>
              </div>
              {@const syms = alerts.filter((a) => a.symbol === selectedSymbol)}
              {#if syms.length === 0}
                <p class="muted">No alerts for this symbol.</p>
              {:else}
                <ul class="alerts">
                  {#each syms as a (a.id)}
                    {@const p = (a.payload ?? {}) as Record<string, unknown>}
                    <li class:acked={a.acknowledged}>
                      <span class="kind" style="color:{kindColor(a.kind, p)}">{a.kind}</span>
                      {#if p.veto}<span class="badge danger tiny">VETO</span>{/if}
                      {#if p.kind === "goalpost_moved"}<span class="badge warning tiny">GOALPOST</span>{/if}
                      {#if p.reasons}<span class="muted">{(p.reasons as string[]).join(" · ")}</span>{/if}
                      <span class="muted">{shortTs(a.created_at)}</span>
                      {#if !a.acknowledged}
                        <button class="x" onclick={() => ack(a.id)} title="ack">ack</button>
                      {/if}
                    </li>
                  {/each}
                </ul>
              {/if}
            {:else if rightTab === "decisions"}
              {#if symbolDecisions === undefined || symbolPositions === undefined}
                <p class="muted">Loading…</p>
              {:else}
                {#if activeThesisDirections.length > 1}
                  <p class="decision-warning">
                    Conflicting open thesis directions: {activeThesisDirections.join(" / ")}.
                    Choose the thesis before recording a decision.
                  </p>
                {/if}
                {#if symbolPositions && symbolPositions.length > 0}
                  <h4>Positions</h4>
                  <ul class="positions">
                    {#each symbolPositions as p (p.position_id)}
                      <li class:closed={!!p.closed_at}>
                        <div class="pos-line">
                          <span class="badge tiny thesis-{p.thesis_direction ?? 'none'}">{p.side}</span>
                          <strong>{p.qty}</strong>
                          <span>{p.instrument}</span>
                          <span class="muted">@ {price(p.avg_price)}</span>
                          {#if p.closed_at}
                            <span class="muted">closed {shortTs(p.closed_at)}</span>
                            <span class:pnl-win={(p.realized_pnl ?? 0) > 0} class:pnl-loss={(p.realized_pnl ?? 0) < 0}>
                              {money(p.realized_pnl)}
                            </span>
                          {:else}
                            <span class="muted">mark {price(p.latest_price)}</span>
                            <span class:pnl-win={(p.unrealized_pnl ?? 0) > 0} class:pnl-loss={(p.unrealized_pnl ?? 0) < 0}>
                              {money(p.unrealized_pnl)}
                            </span>
                            <button class="link-mini" onclick={() => usePositionForExit(p)}>exit ↓</button>
                          {/if}
                        </div>
                        <div class="pos-risk muted">
                          delta {money(p.delta_notional)} · premium {money(p.premium_at_risk)} · fills {p.fill_count}
                        </div>
                      </li>
                    {/each}
                  </ul>
                {:else}
                  <p class="muted">No positions recorded yet for <strong>{selectedSymbol}</strong>.</p>
                {/if}

                {#if symbolDecisions && symbolDecisions.length > 0}
                  <h4>Decisions</h4>
                  <ul class="decisions">
                    {#each symbolDecisions as d (d.decision_id)}
                      {@const extraSizing = visibleSizing(d)}
                      <li>
                        <span class="badge tiny dec-{d.action} thesis-{d.thesis_direction ?? 'unknown'}">{decisionIntentLabel(d)}</span>
                        {#if d.thesis_direction}<span class="muted">thesis {d.thesis_direction}</span>{/if}
                        {#if d.instrument}<span class="muted">{d.instrument}</span>{/if}
                        {#if d.user_choice}<span class="muted">{d.user_choice}</span>{/if}
                        <span class="muted">{shortTs(d.at)}</span>
                        {#if d.thesis_id}
                          <button
                            class="link-mini"
                            onclick={() => { decThesisId = d.thesis_id ?? ""; bottomMode = "decisions"; if (!bottomOpen) bottomPane?.expand(); }}
                            title="prefill the decision form with this thesis"
                          >use ↓</button>
                        {/if}
                        {#if extraSizing}
                          <pre class="dec-sizing">{JSON.stringify(extraSizing)}</pre>
                        {/if}
                      </li>
                    {/each}
                  </ul>
                  <p class="muted hint">Submit new decisions via the bottom drawer's <strong>decisions</strong> tab.</p>
                {:else}
                  <p class="muted">No decisions recorded yet for <strong>{selectedSymbol}</strong>.</p>
                {/if}
              {/if}
            {/if}
          </div>
        {:else}
          <p class="muted center-msg">Pick a symbol on the left.</p>
        {/if}
      </section>
      </aside>
    </Pane>
  </PaneGroup>

</div>

<style>
  .workspace {
    /* Locked to viewport edges — no dependency on any parent chain. */
    position: fixed;
    inset: 0;
    display: grid;
    /* Top bar (44) / body (fills). Error bar overlays via position:absolute. */
    grid-template-rows: 44px minmax(0, 1fr);
    grid-template-columns: 1fr;
    background: #0b0e14;
    overflow: hidden;
  }
  .error-bar {
    position: absolute; top: 44px; left: 0; right: 0; z-index: 5;
    margin: .35rem .75rem;
  }

  /* Body splits horizontally: main-col | splitter | right panel.
     Right panel is full body height; bottom drawer is nested in main-col. */
  /* paneforge nests its own divs inside .body / .main-col — they need to
     fill the workspace row and stack their flex children. */
  :global(.body) {
    height: 100%;
    overflow: hidden;
  }
  :global(.main-col) {
    height: 100%;
    overflow: hidden;
  }

  /* Top bar */
  .top {
    display: flex; align-items: center; gap: 1rem; flex-wrap: wrap;
    padding: 0 1rem;
    background: #11161f; border-bottom: 1px solid #1f2733;
    height: 44px;
  }
  .brand { font-weight: 600; font-size: 1rem; }
  .symbol-box { display: flex; gap: .5rem; align-items: baseline; }
  .symbol-box input {
    background: #0a0d14; color: #cdd6f4; border: 1px solid #2a3548; border-radius: 4px;
    padding: .25rem .5rem; font: inherit; width: 110px; text-transform: uppercase;
  }
  .regime { display: flex; align-items: center; gap: .4rem; font-size: .85rem; }
  .regime .dot { width: .55rem; height: .55rem; border-radius: 50%; }
  .regime .capitulation {
    background: rgba(243, 139, 168, .2); color: rgb(243, 139, 168);
    padding: .05rem .35rem; border-radius: 3px; font-size: .65rem; letter-spacing: .05em;
  }
  .calibration {
    display: flex; align-items: baseline; gap: .25rem; font-size: .8rem;
    padding: .2rem .5rem; background: rgba(180, 190, 254, .05);
    border: 1px solid #1f2733; border-radius: 4px;
  }
  .status { margin-left: auto; font-size: .75rem; color: #f38ba8; }
  .status.on { color: #a6e3a1; }

  .error-bar { display: flex; align-items: center; gap: .5rem; }
  .error-bar .x {
    margin-left: auto;
    background: transparent; border: 1px solid currentColor; border-radius: 3px;
    color: inherit; cursor: pointer; padding: 0 .35rem;
  }

  .chart-panel {
    overflow: auto;
    padding: 1rem;
    min-width: 0;
    min-height: 0;
    height: 100%;
  }
  /* paneforge wraps each Pane in a div; the inner content fills it. */
  :global([data-pane]) { display: flex; flex-direction: column; min-height: 0; }
  :global([data-pane] > *) { flex: 1 1 auto; min-height: 0; }
  :global(.split-v) {
    background: #1f2733;
    cursor: col-resize;
    width: 6px;
    flex-shrink: 0;
    transition: background .15s;
    position: relative;
  }
  :global(.split-v::before) {
    content: ""; position: absolute; top: 0; bottom: 0;
    left: 50%; width: 2px; transform: translateX(-50%);
    background: #2a3548;
  }
  :global(.split-v:hover), :global(.split-v[data-active]) { background: #45567a; }
  :global(.split-v:hover::before), :global(.split-v[data-active]::before) { background: #89b4fa; }

  :global(.split-h) {
    background: #1f2733;
    cursor: row-resize;
    height: 8px;
    flex-shrink: 0;
    transition: background .15s;
    position: relative;
  }
  :global(.split-h::before) {
    content: ""; position: absolute; left: 50%; top: 50%;
    transform: translate(-50%, -50%);
    width: 40px; height: 3px; border-radius: 2px;
    background: #45567a;
  }
  :global(.split-h:hover), :global(.split-h[data-active]) { background: #45567a; }
  :global(.split-h:hover::before), :global(.split-h[data-active]::before) { background: #89b4fa; }
  .chart-stub {
    height: 100%;
    display: flex; flex-direction: column;
    border: 1px dashed #2a3548; border-radius: 8px;
    padding: 1.5rem; align-items: center; justify-content: center;
    background: rgba(180, 190, 254, .02);
    text-align: center;
  }

  /* Right panel */
  .right {
    display: grid;
    grid-template-rows: minmax(120px, 35%) minmax(0, 1fr);
    background: #0a0d14;
    overflow: hidden;
    height: 100%;
    min-height: 0;
  }
  .wl-section, .detail-section { overflow: auto; padding: .5rem .75rem; }
  .wl-section { border-bottom: 1px solid #1f2733; }
  .wl-hdr { display: flex; justify-content: space-between; margin-bottom: .25rem; }
  .wl-new { display: flex; gap: .35rem; margin-bottom: .35rem; }
  .wl-new input {
    flex: 1; background: #0a0d14; color: #cdd6f4; border: 1px solid #2a3548;
    border-radius: 4px; padding: .2rem .35rem; font: inherit; font-size: .8rem;
  }
  .wl-list { list-style: none; padding: 0; margin: 0; display: flex; flex-direction: column; gap: .15rem; }
  .wl-row {
    width: 100%; display: flex; gap: .35rem; align-items: baseline; cursor: pointer;
    padding: .2rem .25rem; border-radius: 3px; border: none; background: transparent;
    color: inherit; font: inherit; text-align: left;
  }
  .wl-row:hover { background: rgba(137, 180, 250, .06); }
  .caret { color: #6c7693; font-size: .7rem; width: .9rem; }
  .wl-name { font-size: .85rem; font-weight: 500; flex: 1; }
  .wl-members { list-style: none; padding: 0 0 0 1.5rem; margin: .1rem 0 .25rem; display: flex; flex-direction: column; gap: .1rem; }
  .wl-mem {
    display: flex; gap: .35rem; align-items: center; padding: .15rem .3rem;
    border-radius: 3px; font-size: .8rem;
  }
  .wl-mem:hover { background: rgba(137, 180, 250, .08); }
  .wl-mem.active { background: rgba(137, 180, 250, .18); }
  .wl-mem-select {
    min-width: 0; flex: 1; display: flex; gap: .35rem; align-items: baseline;
    border: none; background: transparent; color: inherit; font: inherit;
    text-align: left; padding: 0; cursor: pointer;
  }
  .wl-mem-select strong { flex: 1; }
  .wl-thesis-state {
    color: #9aa3b8; font-size: .68rem; text-transform: lowercase;
    white-space: nowrap; max-width: 8.5rem; overflow: hidden; text-overflow: ellipsis;
  }
  .wl-thesis-state.empty { color: #5f6780; }
  .wl-mem .rm {
    background: transparent; border: none; color: #6c7693; font-size: .9rem;
    cursor: pointer; padding: 0 .3rem; line-height: 1;
  }
  .wl-mem .rm:hover { color: #f38ba8; }
  .wl-empty { padding: .15rem .3rem; font-size: .75rem; }
  .wl-add-sym { padding: 0 0 0 1.5rem; margin: .1rem 0; }
  .wl-add-sym input {
    width: 100%; background: #0a0d14; color: #cdd6f4; border: 1px solid #2a3548;
    border-radius: 3px; padding: .15rem .35rem; font: inherit; font-size: .75rem;
    text-transform: uppercase;
  }

  /* Detail tabs */
  .tabs {
    display: flex; gap: .25rem; border-bottom: 1px solid #1f2733;
    margin-bottom: .5rem;
  }
  .tabs button {
    background: transparent; color: #6c7693; border: none; border-bottom: 2px solid transparent;
    padding: .35rem .55rem; cursor: pointer; font: inherit; font-size: .8rem;
    text-transform: capitalize;
  }
  .tabs button.active { color: #cdd6f4; border-bottom-color: #89b4fa; }
  .tab-body { font-size: .85rem; }
  .meta-list {
    display: grid; grid-template-columns: auto 1fr; gap: .25rem .75rem;
    margin: 0;
  }
  .meta-list.inline { grid-template-columns: repeat(4, auto 1fr); }
  .meta-list dt { color: #6c7693; }
  .meta-list dd { margin: 0; }
  .center-msg { text-align: center; padding: 2rem; }

  /* Bottom drawer — height is driven by the workspace --bottom-h CSS var */
  .bottom {
    background: #11161f;
    display: flex; flex-direction: column;
    overflow: hidden;
    min-height: 36px;
  }
  .bottom-tabs {
    display: flex; gap: .25rem; padding: .35rem .5rem;
    border-bottom: 1px solid #1f2733;
    height: 36px;
    align-items: center;
    flex-shrink: 0;
  }
  .bottom-tabs button {
    background: #1b2230; color: #bac2de; border: 1px solid #2a3548;
    border-radius: 4px; padding: .15rem .55rem; cursor: pointer; font: inherit;
    font-size: .8rem; text-transform: capitalize;
    display: flex; gap: .35rem; align-items: center;
  }
  .bottom-tabs button.active { background: #2a3548; border-color: #45567a; color: #cdd6f4; }
  .bottom-toggle {
    margin-left: auto;
    background: #2a3548; color: #cdd6f4; border-color: #45567a;
    font-weight: 600;
  }
  .bottom-toggle:hover { background: #3a4866; }
  .bottom-body {
    flex: 1; overflow: auto; padding: .5rem .75rem;
  }

  .brain-board {
    display: flex;
    flex-direction: column;
    gap: .55rem;
  }
  .brain-topline,
  .brain-contradictions {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: .65rem;
    flex-wrap: wrap;
    border: 1px solid #1f2733;
    background: #0a0d14;
    border-radius: 4px;
    padding: .5rem .65rem;
  }
  .brain-theme-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(320px, 1fr));
    gap: .55rem;
  }
  .brain-theme {
    border: 1px solid #1f2733;
    border-left: 3px solid #6c7693;
    background: #0c1019;
    border-radius: 4px;
    padding: .55rem .65rem;
    display: flex;
    flex-direction: column;
    gap: .4rem;
    min-width: 0;
  }
  .brain-theme.macro-theme {
    background: #0a0d14;
  }
  .brain-theme.freshness-fresh { border-left-color: rgb(166,227,161); }
  .brain-theme.freshness-stale,
  .brain-theme.freshness-missing { border-left-color: rgb(249,226,175); }
  .brain-theme-hdr,
  .brain-badges,
  .brain-line {
    display: flex;
    align-items: center;
    gap: .4rem;
    flex-wrap: wrap;
  }
  .brain-theme-hdr {
    justify-content: space-between;
  }
  .brain-theme p {
    margin: 0;
  }
  .brain-token {
    border: 1px solid #2a3548;
    background: #111827;
    color: #bac2de;
    border-radius: 4px;
    padding: .12rem .35rem;
    font-size: .74rem;
  }
  .brain-token.action {
    cursor: pointer;
    font: inherit;
  }
  .brain-tickers {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(150px, 1fr));
    gap: .35rem;
  }
  .brain-ticker {
    display: grid;
    grid-template-columns: auto 1fr auto auto;
    align-items: center;
    gap: .35rem;
    min-width: 0;
    border: 1px solid #1f2733;
    background: #111827;
    color: #cdd6f4;
    border-radius: 4px;
    padding: .3rem .4rem;
    cursor: pointer;
    font: inherit;
    font-size: .76rem;
    text-align: left;
  }
  .brain-ticker span {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .brain-gaps {
    margin: 0;
    padding-left: 1rem;
    color: #8b95ad;
    font-size: .76rem;
  }

  /* Event feed in drawer */
  .event-feed { list-style: none; padding: 0; margin: 0; display: flex; flex-direction: column; gap: .15rem; }
  .event-feed li {
    background: #0a0d14; border: 1px solid #1f2733; border-radius: 4px;
    padding: .25rem .5rem; display: flex; gap: .4rem; align-items: baseline;
    font-size: .8rem;
  }
  .event-feed li.linkable { cursor: pointer; }
  .event-feed li.linkable:hover { background: rgba(137, 180, 250, .08); }

  /* Alerts */
  .alerts { list-style: none; padding: 0; margin: 0; display: flex; flex-direction: column; gap: .2rem; }
  .alerts li {
    background: #11161f; border: 1px solid #1f2733; border-radius: 4px;
    padding: .25rem .5rem; display: flex; gap: .4rem; align-items: baseline;
    font-size: .8rem;
  }
  .alerts li.acked { opacity: .5; }
  .alerts .x {
    margin-left: auto;
    background: #1b2230; color: #cdd6f4; border: 1px solid #2a3548;
    border-radius: 3px; padding: .05rem .35rem; font-size: .7rem; cursor: pointer;
  }
  .alert-toolbar { margin-bottom: .4rem; }
  .toggle { display: flex; gap: .35rem; align-items: center; font-size: .75rem; color: #6c7693; cursor: pointer; }

  /* Discovery cards in drawer (same as before, compacted) */
  .disc-list { list-style: none; padding: 0; margin: 0; display: flex; flex-direction: column; gap: .5rem; }
  .disc-card {
    background: #0a0d14; border: 1px solid #1f2733; border-radius: 4px;
    padding: .5rem .6rem;
  }
  .disc-hdr { display: flex; gap: .4rem; align-items: baseline; flex-wrap: wrap; }
  .link-symbol { cursor: pointer; }
  .link-symbol:hover { color: #89b4fa; }
  .disc-reasoning { margin: .3rem 0 .4rem; font-size: .8rem; }
  .disc-rank { margin: -.2rem 0 .4rem; font-size: .75rem; }
  .disc-lists { display: flex; flex-direction: column; gap: .2rem; margin-bottom: .35rem; }
  .disc-pick {
    display: flex; align-items: baseline; gap: .35rem; flex-wrap: wrap;
    padding: .2rem .35rem; border: 1px solid #1f2733; border-radius: 3px;
    cursor: pointer; font-size: .8rem;
  }
  .disc-rat { flex: 1; font-size: .75rem; }
  .disc-newlist {
    background: rgba(180, 190, 254, .05); border: 1px dashed #2a3548;
    border-radius: 3px; padding: .25rem .4rem; margin-bottom: .35rem;
    display: flex; gap: .35rem; flex-wrap: wrap; align-items: baseline;
    font-size: .8rem;
  }
  .disc-actions { display: flex; gap: .35rem; margin-top: .3rem; }
  .disc-actions button {
    background: #1b2230; color: #cdd6f4; border: 1px solid #2a3548;
    border-radius: 3px; padding: .2rem .55rem; font: inherit; font-size: .75rem; cursor: pointer;
  }
  .disc-actions .reject {
    background: rgba(243, 139, 168, .1); border-color: rgba(243, 139, 168, .3);
    color: rgb(243, 139, 168);
  }

  /* Decision form */
  .decform {
    display: grid; grid-template-columns: 1fr 1fr; gap: .5rem; max-width: 760px;
    font-size: .85rem;
  }
  .decform label { display: flex; flex-direction: column; gap: .15rem; }
  .decform input, .decform select {
    background: #0a0d14; color: #cdd6f4; border: 1px solid #2a3548; border-radius: 4px;
    padding: .25rem .4rem; font: inherit;
  }
  .decform .checkline {
    flex-direction: row; align-items: center; gap: .4rem; grid-column: 1 / -1;
  }
  .decform .checkline input { width: auto; }
  .decform .wide { grid-column: 1 / -1; }
  .decform button {
    grid-column: 1;
    background: #1b2230; color: #cdd6f4; border: 1px solid #45567a;
    border-radius: 4px; padding: .35rem .8rem; font: inherit; cursor: pointer;
  }
  .decform .muted { grid-column: 2; align-self: end; }
  .decision-context {
    align-self: end;
    border: 1px solid #2a3548;
    border-radius: 3px;
    padding: .25rem .45rem;
    font-size: .75rem;
  }
  .decision-warning {
    margin: 0 0 .4rem 0;
    padding: .35rem .5rem;
    border: 1px solid rgba(249,226,175,.35);
    border-radius: 4px;
    color: rgb(249,226,175);
    background: rgba(249,226,175,.08);
    font-size: .78rem;
  }
  .declines {
    display: flex; flex-direction: column; gap: .45rem;
  }
  .declines h4 { margin: .25rem 0 0; font-size: .82rem; }
  .declines ul {
    list-style: none; padding: 0; margin: 0;
    display: flex; flex-direction: column; gap: .45rem;
  }
  .decline-card {
    border: 1px solid #1f2733;
    border-radius: 4px;
    background: #0c1019;
    padding: .55rem .65rem;
    font-size: .8rem;
  }
  .decline-card.status-open { border-color: rgba(249, 226, 175, .35); }
  .decline-card.status-resolved { opacity: .72; }
  .decline-card.status-dismissed { opacity: .6; }
  .decline-hdr {
    display: flex; align-items: baseline; gap: .4rem; margin-bottom: .25rem;
  }
  .decline-card p { margin: 0; color: #bac2de; line-height: 1.35; }
  .brain-card {
    margin-top: .65rem;
    border: 1px solid #1f2733;
    border-left: 3px solid #45567a;
    border-radius: 4px;
    background: #0c1019;
    padding: .6rem .7rem;
    font-size: .8rem;
  }
  .brain-card.brain-fresh { border-left-color: rgb(166,227,161); }
  .brain-card.brain-due,
  .brain-card.brain-stale,
  .brain-card.brain-waiting_on_evidence { border-left-color: rgb(249,226,175); }
  .brain-card.brain-blocked { border-left-color: rgb(243,139,168); }
  .brain-card p {
    margin: .35rem 0;
    color: #bac2de;
    line-height: 1.35;
  }
  .brain-hdr {
    display: flex;
    align-items: baseline;
    gap: .4rem;
    flex-wrap: wrap;
  }
  .brain-title {
    font-size: .7rem;
    text-transform: uppercase;
    letter-spacing: .05em;
    color: #9aa3b8;
  }
  .brain-sources {
    list-style: none;
    padding: 0;
    margin: .45rem 0 0;
    display: flex;
    flex-direction: column;
    gap: .28rem;
  }
  .brain-sources li {
    display: flex;
    align-items: baseline;
    gap: .35rem;
    flex-wrap: wrap;
  }
  .text-action {
    background: transparent;
    border: none;
    color: #89b4fa;
    padding: 0;
    font: inherit;
    cursor: pointer;
  }
  .parent-brain-list {
    list-style: none;
    padding: 0;
    margin: .45rem 0 0;
    display: flex;
    flex-direction: column;
    gap: .55rem;
  }
  .parent-brain-list li {
    border-top: 1px solid #1f2733;
    padding-top: .45rem;
  }
  .parent-brain-list li:first-child {
    border-top: none;
    padding-top: 0;
  }
  .parent-brain-hdr,
  .parent-brain-questions {
    display: flex;
    align-items: baseline;
    gap: .35rem;
    flex-wrap: wrap;
  }
  .parent-brain-questions span {
    color: #9aa3b8;
    font-size: .72rem;
  }
  .evidence-list {
    list-style: none; padding: 0; margin: 0;
    display: flex; flex-direction: column; gap: .45rem;
  }
  .evidence-card {
    border: 1px solid #1f2733;
    border-radius: 4px;
    background: #0c1019;
    padding: .55rem .65rem;
    font-size: .8rem;
  }
  .evidence-card.state-missing,
  .evidence-card.state-partial,
  .evidence-card.state-blocked {
    border-color: rgba(249, 226, 175, .35);
  }
  .evidence-card.state-satisfied { opacity: .72; }
  .evidence-row {
    display: flex; gap: .4rem; align-items: baseline; flex-wrap: wrap; margin-bottom: .25rem;
  }
  .evidence-card p { margin: 0; color: #bac2de; line-height: 1.35; }
  .error-text { color: rgb(243, 139, 168) !important; }

  /* Generic */
  .kind { font-size: .65rem; text-transform: uppercase; letter-spacing: .05em; }
  .badge {
    display: inline-block; padding: 0 .35rem; border-radius: 3px;
    background: #1f2733; font-size: .7rem;
  }
  .badge.tiny { font-size: .65rem; padding: 0 .3rem; }
  .badge.danger { background: rgba(243, 139, 168, .18); color: rgb(243, 139, 168); }
  .badge.warning { background: rgba(249, 226, 175, .15); color: rgb(249, 226, 175); }
  .badge.conf-high { background: rgba(166, 227, 161, .18); color: rgb(166, 227, 161); }
  .badge.conf-medium { background: rgba(249, 226, 175, .15); color: rgb(249, 226, 175); }
  .badge.conf-low { background: rgba(108, 112, 134, .2); color: #9aa3b8; }
  .badge.rank-highest { background: rgba(166,227,161,.2); color: rgb(166,227,161); }
  .badge.rank-high { background: rgba(137,180,250,.18); color: rgb(137,180,250); }
  .badge.rank-medium { background: rgba(249,226,175,.15); color: rgb(249,226,175); }
  .badge.rank-low { background: rgba(108,112,134,.2); color: #9aa3b8; }
  .badge.state-actionable,
  .badge.state-ready_for_review { background: rgba(166,227,161,.18); color: rgb(166,227,161); }
  .badge.state-waiting_on_data,
  .badge.state-evaluating,
  .badge.state-queued { background: rgba(137,180,250,.16); color: rgb(137,180,250); }
  .badge.state-blocked { background: rgba(243,139,168,.18); color: rgb(243,139,168); }
  .badge.state-operator_deferred,
  .badge.state-dismissed,
  .badge.state-resolved { background: rgba(108,112,134,.2); color: #9aa3b8; }
  .badge.owner-operator { background: rgba(166,227,161,.12); color: rgb(166,227,161); }
  .badge.owner-source,
  .badge.owner-cognition,
  .badge.owner-system { background: rgba(137,180,250,.12); color: rgb(137,180,250); }
  .badge.owner-risk { background: rgba(249,226,175,.15); color: rgb(249,226,175); }
  .badge.sev-blocked  { background: rgba(243,139,168,.18); color: rgb(243,139,168); }
  .badge.sev-decision { background: rgba(137,180,250,.18); color: rgb(137,180,250); }
  .badge.sev-review   { background: rgba(249,226,175,.15); color: rgb(249,226,175); }
  .badge.sev-info     { background: rgba(108,112,134,.2);  color: #9aa3b8; }
  .badge.health-ok { background: rgba(166,227,161,.18); color: rgb(166,227,161); }
  .badge.health-no_new_rows { background: rgba(137,180,250,.16); color: rgb(137,180,250); }
  .badge.health-running { background: rgba(249,226,175,.15); color: rgb(249,226,175); }
  .badge.health-failed { background: rgba(243,139,168,.18); color: rgb(243,139,168); }
  .badge.health-rate_limited { background: rgba(243,139,168,.18); color: rgb(243,139,168); }
  .badge.brain-fresh,
  .badge.brain-source-fresh { background: rgba(166,227,161,.18); color: rgb(166,227,161); }
  .badge.brain-fresh-fresh,
  .badge.brain-dir-risk_on,
  .badge.brain-dir-bullish { background: rgba(166,227,161,.18); color: rgb(166,227,161); }
  .badge.brain-due,
  .badge.brain-stale,
  .badge.brain-waiting_on_evidence,
  .badge.brain-source-stale,
  .badge.brain-source-missing,
  .badge.brain-source-running { background: rgba(249,226,175,.15); color: rgb(249,226,175); }
  .badge.brain-fresh-stale,
  .badge.brain-fresh-missing,
  .badge.brain-dir-neutral,
  .badge.brain-dir-mixed { background: rgba(249,226,175,.15); color: rgb(249,226,175); }
  .badge.brain-blocked,
  .badge.brain-source-failed,
  .badge.brain-source-rate_limited { background: rgba(243,139,168,.18); color: rgb(243,139,168); }
  .badge.brain-dir-risk_off,
  .badge.brain-dir-bearish { background: rgba(243,139,168,.18); color: rgb(243,139,168); }

  /* Attention queue (#86) — grouped card design */
  .att-toolbar { display: flex; gap: .5rem; align-items: baseline; margin-bottom: .5rem; flex-wrap: wrap; }
  .att-filters { display: flex; gap: .25rem; flex-wrap: wrap; }
  .att-filters button {
    background: #11161f; color: #6c7693; border: 1px solid #1f2733;
    border-radius: 3px; padding: .12rem .45rem; font: inherit; font-size: .7rem;
    cursor: pointer; text-transform: lowercase;
  }
  .att-filters button.active { background: #2a3548; color: #cdd6f4; border-color: #45567a; }
  .att-toolbar .reset {
    margin-left: auto;
    background: transparent; color: #6c7693; border: 1px solid #2a3548; border-radius: 3px;
    cursor: pointer; padding: 0 .35rem; font: inherit; font-size: .8rem;
  }
  .att-list { list-style: none; padding: 0; margin: 0; display: flex; flex-direction: column; gap: .5rem; }
  .att-section { margin-top: .55rem; }
  .att-section:first-of-type { margin-top: 0; }
  .att-section-head {
    display: flex;
    gap: .45rem;
    align-items: center;
    margin: 0 0 .35rem;
    font-size: .75rem;
    text-transform: lowercase;
  }
  .att-section-head strong { color: #cdd6f4; }
  .att-card {
    background: #0a0d14; border: 1px solid #1f2733; border-radius: 4px;
    padding: .55rem .7rem;
    border-left: 3px solid #2a3548;
  }
  .att-card.sev-blocked  { border-left-color: rgb(243,139,168); }
  .att-card.sev-decision { border-left-color: rgb(137,180,250); }
  .att-card.sev-review   { border-left-color: rgb(249,226,175); }
  .att-card.sev-info     { border-left-color: #6c7693; }

  /* Row 1: TICKER (large, bold) + tier (small, muted) | time (right) */
  .att-row1 {
    display: flex; align-items: baseline; gap: .5rem; margin-bottom: .1rem;
  }
  .att-symbol {
    font-size: 1rem; letter-spacing: .02em; cursor: pointer;
  }
  .att-symbol:hover { color: #89b4fa; }
  .att-tier { font-size: .7rem; text-transform: uppercase; letter-spacing: .05em; }
  .att-time { margin-left: auto; font-size: .75rem; }

  /* Row 2: status line — "candidate · 3 signals over 14d", "thesis ready", etc. */
  .att-status {
    font-size: .75rem; margin-bottom: .35rem;
  }

  /* Row 3: bullet list of reasons */
  .att-reasons {
    list-style: none; padding: 0; margin: 0 0 .35rem 0;
    display: flex; flex-direction: column; gap: .1rem;
    font-size: .8rem;
  }
  .att-reasons li { line-height: 1.35; }

  /* Optional middle "Fits → checkboxes" row */
  .att-fits {
    display: flex; flex-wrap: wrap; gap: .35rem; align-items: baseline;
    margin: .35rem 0; padding: .35rem .45rem;
    background: rgba(180,190,254,.04); border-radius: 3px;
    font-size: .75rem;
  }
  .att-fits .muted { margin-right: .15rem; }
  .att-pick {
    display: flex; align-items: baseline; gap: .25rem;
    padding: .1rem .35rem; border: 1px solid #1f2733; border-radius: 3px;
    cursor: pointer; background: #11161f;
  }
  .att-pick:hover { background: #1b2230; }

  /* Row 4: action buttons */
  .att-actions { display: flex; gap: .35rem; flex-wrap: wrap; margin-top: .25rem; }
  .att-actions button {
    background: #1b2230; color: #cdd6f4; border: 1px solid #2a3548;
    border-radius: 3px; padding: .25rem .65rem; font: inherit; font-size: .8rem; cursor: pointer;
  }
  .att-actions .confirm {
    background: rgba(166,227,161,.12); border-color: rgba(166,227,161,.35); color: rgb(166,227,161);
  }
  .att-actions .confirm:hover { background: rgba(166,227,161,.2); }
  .att-actions .reject { background: rgba(243,139,168,.08); border-color: rgba(243,139,168,.3); color: rgb(243,139,168); }
  .att-actions .reject:hover { background: rgba(243,139,168,.15); }

  /* Reject-with-reason dropdown panel */
  .att-reject-menu {
    margin-top: .4rem; padding: .35rem .45rem;
    background: rgba(243,139,168,.06); border: 1px dashed rgba(243,139,168,.3); border-radius: 3px;
    display: flex; flex-wrap: wrap; gap: .25rem; align-items: baseline;
  }
  .reject-reason {
    background: #1b2230; color: rgb(243,139,168); border: 1px solid rgba(243,139,168,.25);
    border-radius: 3px; padding: .15rem .5rem; font: inherit; font-size: .7rem; cursor: pointer;
    text-transform: lowercase;
  }
  .reject-reason:hover { background: rgba(243,139,168,.18); }

  /* Narrow viewport polish (#57 PR5). At <= 760px wide, stack everything
     vertically: chart on top, drawer in middle, sidebar at bottom. paneforge
     gracefully degrades when the outer PaneGroup is flex-column. */
  @media (max-width: 760px) {
    :global([data-pane-group][data-direction="horizontal"]) {
      flex-direction: column !important;
    }
    :global(.split-v) {
      width: auto !important; height: 8px !important;
      cursor: row-resize !important;
    }
    :global(.split-v::before) {
      top: 50% !important; bottom: auto !important;
      left: 50% !important;
      width: 40px !important; height: 3px !important;
      transform: translate(-50%, -50%) !important;
    }
    .top {
      flex-wrap: wrap; height: auto; padding: .35rem .5rem;
      gap: .5rem;
    }
    .symbol-box input { width: 90px; }
    .calibration { display: none; }
  }

  .decisions { list-style: none; padding: 0; margin: 0; display: flex; flex-direction: column; gap: .2rem; }
  .decisions li {
    display: flex; align-items: baseline; gap: .35rem; flex-wrap: wrap;
    padding: .25rem .4rem; border: 1px solid #1f2733; border-radius: 3px;
    font-size: .8rem;
  }
  .positions { list-style: none; padding: 0; margin: .15rem 0 .8rem; display: flex; flex-direction: column; gap: .25rem; }
  .positions li {
    border: 1px solid #263144; border-radius: 4px; padding: .35rem .45rem;
    font-size: .8rem;
  }
  .positions li.closed { opacity: .72; }
  .pos-line { display: flex; align-items: baseline; gap: .35rem; flex-wrap: wrap; }
  .pos-risk { margin-top: .2rem; font-size: .72rem; }
  .pnl-win { color: rgb(166,227,161); }
  .pnl-loss { color: rgb(243,139,168); }
  .dec-sizing { font-size: .7rem; margin: 0; color: #6c7693; background: transparent; padding: 0; }
  .badge.dec-enter   { background: rgba(166,227,161,.18); color: rgb(166,227,161); }
  .badge.dec-exit    { background: rgba(243,139,168,.18); color: rgb(243,139,168); }
  .badge.dec-skip    { background: rgba(108,112,134,.2);  color: #9aa3b8; }
  .badge.dec-resize  { background: rgba(249,226,175,.18); color: rgb(249,226,175); }
  .thesis-down { color: rgb(243,139,168); }
  .thesis-up { color: rgb(166,227,161); }
  .thesis-neutral { color: rgb(249,226,175); }
  .thesis-none { color: #6c7693; }
  .badge.thesis-down { background: rgba(243,139,168,.16); color: rgb(243,139,168); }
  .badge.thesis-up { background: rgba(166,227,161,.16); color: rgb(166,227,161); }
  .badge.thesis-neutral { background: rgba(249,226,175,.15); color: rgb(249,226,175); }
  .badge.thesis-none { background: rgba(108,112,134,.16); color: #9aa3b8; }
  .link-mini {
    background: transparent; color: #89b4fa; border: none; cursor: pointer;
    font: inherit; font-size: .75rem; padding: 0;
  }
  .link-mini:hover { text-decoration: underline; }
  .hint { margin-top: .35rem; font-size: .75rem; }

  /* #92 diagnostics tab */
  .diag-grid {
    display: grid; grid-template-columns: 1fr 1fr; gap: 0.75rem;
  }
  .diag.wide { grid-column: 1 / -1; }
  .diag {
    background: #11161f; border: 1px solid #1f2733; border-radius: 4px;
    padding: 0.5rem 0.75rem;
  }
  .diag h5 { margin: 0 0 0.5rem 0; font-size: 0.8rem; color: #bac2de; font-weight: 600; }
  .diag-tbl { width: 100%; border-collapse: collapse; font-size: 0.78rem; }
  .diag-tbl th { text-align: left; color: #6c7086; font-weight: 400; padding: 0.2rem 0.4rem 0.2rem 0; }
  .diag-tbl td { padding: 0.15rem 0.4rem 0.15rem 0; }
  .diag-tbl code { font-size: 0.78rem; color: #cdd6f4; background: #0a0d14; padding: 0 0.25rem; border-radius: 3px; }
  .chips {
    display: flex; flex-wrap: wrap; gap: 0.3rem; list-style: none;
    margin: 0.3rem 0 0 0; padding: 0;
  }
  .chip {
    background: rgba(137, 180, 250, 0.08); color: #cdd6f4;
    border: 1px solid #1f2733; border-radius: 3px;
    padding: 0.1rem 0.4rem; font-size: 0.72rem;
  }
  .err { color: #f38ba8; font-size: 0.85rem; }
</style>
