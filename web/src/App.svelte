<script lang="ts">
  // Workspace shell (#57 PR1). Single-symbol model: pick a symbol on the
  // right, see everything about it in the right detail panel; workflows
  // (events, discovery, decisions, calibration) live in the bottom drawer.
  // Chart in the main area is a placeholder — PR2 wires a real chart.
  import { onMount } from "svelte";
  import {
    ackAlert,
    addToWatchlist,
    approveAutomationPermission,
    confirmCandidate,
    createWatchlist,
    fetchAlerts,
    fetchBrainJournal,
    fetchBrainOverview,
    fetchBrainStatus,
    fetchCalibration,
    fetchAttention,
    fetchAttentionReviewPacket,
    dismissAttention,
    fetchDecisions,
    fetchDiscoveryPool,
    fetchDecisionReplay,
    fetchEvidenceItems,
    fetchEvidenceRequirements,
    fetchPendingCandidates,
    fetchPositions,
    fetchRegime,
    fetchResearchEvidence,
    fetchSymbolWorkflow,
    fetchTechnicalState,
    fetchThesisDeclines,
    fetchTheses,
    fetchTickerContext,
    fetchTickers,
    fetchWatchlistMembers,
    fetchWatchlists,
    postDecision,
    promoteTicker,
    rejectCandidate,
    removeFromWatchlist,
    startSymbolResearch,
    subscribe,
    transitionAttention,
    type Alert,
    type AttentionItem,
    type AttentionReviewPacket,
    type BrainJournal,
    type BrainJournalEntry,
    type BrainLinkedTicker,
    type BrainOverview,
    type CognitionRun,
    type BrainSourceStatus,
    type BrainStatus,
    type BrainThesis,
    type Calibration,
    type DecisionRow,
    type DecisionReplay,
    type EvidenceItem,
    type EvidenceRequirement,
    type MarketState,
    type PendingCandidate,
    type PoolMember,
    type PositionRow,
    type ResearchEvidence,
    type ReviewPacketAction,
    type ReviewPacketActionPayload,
    type StreamEvent,
    type SymbolWorkflow as ApiSymbolWorkflow,
    type SymbolWorkflowAttention,
    type SymbolWorkflowStep,
    type WorkflowActionKind,
    type TechnicalState,
    type ThesisDetail,
    type ThesisDecline,
    type Ticker,
    type TickerContext,
    type Watchlist,
    type WatchlistMember,
    type WatchlistParentTheme,
  } from "./lib/api";
  import AnalystPanel from "./lib/AnalystPanel.svelte";
  import ContextPanel from "./lib/ContextPanel.svelte";
  import BrainJournalPage from "./lib/BrainJournalPage.svelte";
  import ReviewPacket from "./lib/ReviewPacket.svelte";
  import TechnicalStatePanel from "./lib/TechnicalStatePanel.svelte";
  import ThesisDetails from "./lib/ThesisDetails.svelte";
  import ChartPanel from "./lib/ChartPanel.svelte";
  import PriceAlertsPanel from "./lib/PriceAlertsPanel.svelte";
  import AutomationPanel from "./lib/AutomationPanel.svelte";
  import AutonomousTradingPage from "./lib/AutonomousTradingPage.svelte";
  import { PaneGroup, Pane, PaneResizer } from "paneforge";

  // ---------- workspace state ----------
  type RightTab = "overview" | "analyst" | "technical" | "context" | "evidence" | "theses" | "alerts" | "decisions";
  const RIGHT_TABS: RightTab[] = ["overview", "analyst", "technical", "context", "evidence", "theses", "alerts", "decisions"];
  type BottomMode = "brain" | "attention" | "events" | "discovery" | "automation" | "decisions" | "calibration" | "diagnostics";
  type AppPage = "workspace" | "journal" | "automation";
  type WorkflowAction = WorkflowActionKind;
  type SymbolWorkflow = {
    state: string;
    tone: string;
    reason: string;
    primary: string;
    action: WorkflowAction;
    primaryDetail?: string | null;
    reviewPacketAttentionId?: number | null;
    attentionItems?: SymbolWorkflowAttention[];
    steps?: SymbolWorkflowStep[];
    status: string;
    attention: string;
    evidence: string;
    thesis: string;
    decision: string;
  };
  type SymbolPlacement = {
    label: string;
    short: string;
    tone: string;
    location: string;
    detail: string;
  };
  const DISAGREEMENT_REASONS = [
    { value: "wrong_cluster", label: "wrong cluster" },
    { value: "not_my_edge", label: "not my edge" },
    { value: "signal_too_weak", label: "signal too weak" },
    { value: "valuation_priced", label: "valuation priced" },
    { value: "data_stale", label: "data stale" },
    { value: "llm_overreached", label: "LLM overreached" },
    { value: "risk_too_high", label: "risk too high" },
    { value: "other", label: "other" },
  ];

  let selectedSymbol = $state<string | null>(null);
  let routePage = $state<AppPage>("workspace");
  let automationSymbol = $state<string | null>(null);
  let rightTab = $state<RightTab>("overview");
  let bottomMode = $state<BottomMode>("attention");
  let bottomOpen = $state(true);

  // ---------- global data ----------
  let regime = $state<MarketState | null>(null);
  let brainOverview = $state<BrainOverview | null>(null);
  let brainJournal = $state<BrainJournal | null>(null);
  let journalDate = $state(todayIsoDate());
  let journalPage = $state(1);
  let journalLoading = $state(false);
  let journalError = $state<string | null>(null);
  let journalLoadSeq = 0;
  const JOURNAL_PER_PAGE = 50;
  const JOURNAL_REFRESH_MS = 10000;
  let journalRefreshTimer: ReturnType<typeof setInterval> | null = null;
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
  let chartState = $state<{ interval: string; range: string }>({ interval: "1D", range: "ALL" });
  let pending = $state<PendingCandidate[]>([]);
  let watchlists = $state<Watchlist[]>([]);
  let watchlistMembers = $state<Record<string, WatchlistMember[]>>({});
  let pool = $state<PoolMember[]>([]);
  let attention = $state<AttentionItem[]>([]);
  let attentionFilter = $state<string>("all");
  let reviewPacket = $state<AttentionReviewPacket | null>(null);
  let reviewPacketLoading = $state(false);
  let reviewPacketError = $state<string | null>(null);
  let reviewPacketDecisionOpen = $state(false);
  let promotionBusy = $state(false);
  let promotionStatus = $state<string | null>(null);
  let researchKickoffBusy = $state(false);
  let researchKickoffSymbol = $state<string | null>(null);
  let researchKickoffStatus = $state<string | null>(null);

  async function refreshAttention() {
    try {
      attention = await fetchAttention("open");
    } catch {}
  }
  function focusReviewPacket() {
    setTimeout(() => {
      document.querySelector('[data-testid="review-packet"]')?.scrollIntoView({ behavior: "smooth", block: "start" });
    }, 50);
  }
  async function openReviewPacketById(id: number, fallbackSymbol?: string | null) {
    reviewPacketLoading = true;
    reviewPacketError = null;
    reviewPacketDecisionOpen = false;
    promotionStatus = null;
    try {
      reviewPacket = await fetchAttentionReviewPacket(id);
      const symbol = reviewPacket.attention.symbol ?? fallbackSymbol;
      if (symbol) await selectSymbol(symbol);
      focusReviewPacket();
    } catch (e) {
      reviewPacketError = String(e);
    } finally {
      reviewPacketLoading = false;
    }
  }
  async function openReviewPacketFor(item: AttentionItem) {
    await openReviewPacketById(item.id, item.symbol);
  }
  async function confirmReviewPacketCandidate(candidateId: number, watchlistIds: string[]) {
    promotionBusy = true;
    promotionStatus = null;
    try {
      await confirmCandidate(candidateId, watchlistIds);
      await Promise.all([
        refreshAttention(),
        refreshPending(),
        refreshWatchlists(),
        refreshSelectedWatchlistMembers(watchlistIds),
        fetchTickers().then((t) => (tickers = t)),
      ]);
      await reloadSelectedSymbolDetails();
      const destination = watchlistIds.length > 0
        ? `Universe + ${watchlistIds.length} watchlist${watchlistIds.length === 1 ? "" : "s"}`
        : "Universe";
      promotionStatus = `Research started in ${destination}; context and thesis work will refresh.`;
    } catch (e) {
      error = String(e);
    } finally {
      promotionBusy = false;
    }
  }
  async function handleReviewPacketAction(
    action: ReviewPacketAction,
    packet: AttentionReviewPacket,
    payload: ReviewPacketActionPayload = {},
  ) {
    const item = packet.attention;
    if (action.kind === "candidate_confirm" && item.candidate_id) {
      await confirmReviewPacketCandidate(item.candidate_id, payload.watchlistIds ?? []);
      return;
    }
    if (action.kind === "candidate_reject" && item.candidate_id) {
      await rejectGroup([item.candidate_id], "not_my_edge");
      reviewPacket = null;
      return;
    }
    if (action.kind === "attention_defer") {
      await deferOne(item.id);
      reviewPacket = null;
      return;
    }
    if (action.kind === "attention_dismiss") {
      await dismissOne(item.id, "review_packet");
      reviewPacket = null;
      return;
    }
    if (action.kind === "automation_approve") {
      if (!item.symbol) {
        error = "automation approval needs a symbol";
        return;
      }
      promotionBusy = true;
      promotionStatus = null;
      try {
        await approveAutomationPermission({
          symbol: item.symbol,
          strategyId: action.strategy_id || "thesis_timing",
          strategyVersion: action.strategy_version || "0.1.0",
          environmentScope: action.environment_scope || "shadow",
          sourceRef: {
            attention_id: item.id,
            attention_kind: item.kind,
            thesis_id: item.thesis_id,
            action_id: action.id,
          },
        });
        await refreshAttention();
        promotionStatus = `${item.symbol} approved for shadow automation.`;
        openAutomationPage(item.symbol);
      } catch (e) {
        error = e instanceof Error ? e.message : String(e);
      } finally {
        promotionBusy = false;
      }
      return;
    }
    if (item.symbol) await selectSymbol(item.symbol);
    if (action.kind === "decision") {
      openReviewPacketDecision(reviewPacketDecisionAction(item), item);
      return;
    }
    if (action.kind === "decision_skip") {
      openReviewPacketDecision("skip", item);
      return;
    }
    if (action.kind === "open_evidence") {
      rightTab = "evidence";
      return;
    }
    rightTab = item.thesis_id ? "theses" : "overview";
  }

  async function startResearchForSymbol(
    rawSymbol: string,
    opts: { openEvidence?: boolean; refreshJournal?: boolean } = {},
  ) {
    const symbol = normalizeSymbol(rawSymbol);
    if (!symbol || researchKickoffBusy) return;
    researchKickoffBusy = true;
    researchKickoffSymbol = symbol;
    researchKickoffStatus = null;
    error = null;
    try {
      const res = await startSymbolResearch(symbol);
      researchKickoffStatus = `${res.symbol}: ${res.queued} research task${res.queued === 1 ? "" : "s"} queued`;
      if (opts.openEvidence) {
        if (selectedSymbol !== symbol) await selectSymbol(symbol);
        rightTab = "evidence";
        await reloadSelectedSymbolDetails();
      } else if (selectedSymbol === symbol) {
        await reloadSelectedSymbolDetails();
      }
      await Promise.all([
        fetchTickers().then((t) => (tickers = t)).catch(() => {}),
        refreshAttention(),
        fetchBrainOverview().then((b) => (brainOverview = b)).catch(() => {}),
      ]);
      if (opts.refreshJournal || routePage === "journal") {
        await loadBrainJournal(journalDate, journalPage, { silent: true });
      }
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      researchKickoffBusy = false;
      researchKickoffSymbol = null;
    }
  }

  async function startResearchForSelected() {
    if (!selectedSymbol) return;
    await startResearchForSymbol(selectedSymbol, { openEvidence: true });
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
    if (candidateIds.length === 0) return;
    promotionBusy = true;
    promotionStatus = null;
    try {
      // Iterate; backend resolves the matching attention item per candidate.
      for (const id of candidateIds) await rejectCandidate(id);
      await Promise.all([refreshAttention(), refreshPending()]);
      await reloadSelectedSymbolDetails();
      promotionStatus = "Rejected nomination and removed it from the review queue.";
    } catch (e) {
      error = String(e);
    } finally {
      promotionBusy = false;
    }
  }
  async function confirmGroup(candidateIds: number[]) {
    if (candidateIds.length === 0) return;
    const lists = new Set<string>();
    for (const cid of candidateIds) {
      const inner = chosenLists[cid] ?? {};
      for (const [wlId, on] of Object.entries(inner)) if (on) lists.add(wlId);
    }
    const ids = [...lists];
    promotionBusy = true;
    promotionStatus = null;
    try {
      // Confirm always promotes the ticker. Optional checked lists add
      // watchlist memberships; empty list selection means Universe only.
      for (const cid of candidateIds) await confirmCandidate(cid, ids);
      await Promise.all([
        refreshAttention(),
        refreshPending(),
        refreshWatchlists(),
        refreshSelectedWatchlistMembers(ids),
        fetchTickers().then((t) => (tickers = t)),
      ]);
      await reloadSelectedSymbolDetails();
      const destination = ids.length > 0 ? `Universe + ${ids.length} watchlist${ids.length === 1 ? "" : "s"}` : "Universe";
      promotionStatus = `Research started in ${destination}; cognition will refresh context and thesis.`;
    } catch (e) {
      error = String(e);
    } finally {
      promotionBusy = false;
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

  function candidateAvailableData(item: AttentionItem | null): string[] {
    const available = item?.source_ref?.available_data;
    if (!available || typeof available !== "object" || Array.isArray(available)) return [];
    return Object.entries(available as Record<string, unknown>)
      .filter(([, value]) => value === true)
      .map(([key]) => key.replace(/_/g, " "));
  }

  function candidateNominationReason(item: AttentionItem | null): string {
    if (!item) return "";
    const nomination = item.source_ref?.nomination_reasons;
    if (nomination && typeof nomination === "object" && !Array.isArray(nomination)) {
      const n = nomination as Record<string, unknown>;
      const theme = typeof n.theme === "string" ? n.theme : "";
      const fit = typeof n.business_fit === "string" ? n.business_fit : "";
      if (theme && fit) return `${theme}: ${fit}`;
      if (theme || fit) return theme || fit;
    }
    return displayReason(item.reason ?? item.title);
  }

  function candidateAcceptanceText(item: AttentionItem | null): string {
    const nomination = item?.source_ref?.nomination_reasons;
    if (nomination && typeof nomination === "object" && !Array.isArray(nomination)) {
      const text = (nomination as Record<string, unknown>).acceptance_effect;
      if (typeof text === "string" && text.trim()) return text;
    }
    return "add to monitored universe and run context/thesis";
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
    if (status === "stale_running") return "stale running";
    if (status === "no_new_rows") return "checked, no new rows";
    if (status === "ok") return "new data";
    if (status === "rate_limited") return "rate limited";
    return status;
  }

  function databaseStatusLabel(status?: string | null): string {
    if (status === "ok") return "reachable";
    if (status === "unreachable") return "unreachable";
    return "unknown";
  }

  function brainStatusLabel(status: string): string {
    return status.replace(/_/g, " ");
  }

  function brainActionLabel(action: string): string {
    if (action === "add_to_universe") return "add to universe";
    if (action === "reevaluate_after_source_update") return "re-evaluate after source update";
    if (action === "draft_after_source_update") return "draft after source update";
    if (action === "reevaluate_after_evidence_update") return "re-evaluate after evidence update";
    if (action === "draft_after_evidence_update") return "draft after evidence update";
    return action.replace(/_/g, " ");
  }

  function cognitionRunLabel(status: string): string {
    if (status === "blocked_on_evidence") return "blocked";
    if (status === "context_refreshed") return "context refreshed";
    return status.replace(/_/g, " ");
  }

  function cognitionRunTime(run: CognitionRun): string {
    const parts = [`started ${relativeTime(run.started_at)}`];
    if (run.finished_at) parts.push(`finished ${relativeTime(run.finished_at)}`);
    if (run.next_retry_at) parts.push(`retry ${relativeTime(run.next_retry_at)}`);
    return parts.join(" · ");
  }

  function cognitionTriggerLabel(trigger: string | null | undefined): string {
    if (trigger === "source_task_delta") return "source data changed";
    if (trigger === "evidence_delta") return "evidence changed";
    if (trigger === "open_thesis_update_loop") return "thesis refresh";
    if (trigger === "evidence_state_bootstrap") return "evidence bootstrap";
    if (trigger === "maintenance_sweep") return "maintenance";
    if (trigger === "discovery.confirmed") return "confirmed symbol";
    return trigger ? trigger.replace(/_/g, " ") : "scheduled";
  }

  function cognitionSweepReasonLabel(reason: string | null | undefined): string {
    if (reason === "source_task_changed") return "source data newer than thesis";
    if (reason === "source_task_changed_retry") return "source data newer than decline";
    if (reason === "evidence_item_changed") return "evidence newer than thesis";
    if (reason === "evidence_item_changed_retry") return "evidence newer than decline";
    if (reason === "open_thesis_due") return "thesis due";
    if (reason === "context_missing") return "context missing";
    if (reason === "context_missing_market") return "market context missing";
    if (reason === "evidence_state_missing") return "evidence checklist missing";
    if (reason === "evidence_retry_due") return "evidence retry due";
    if (reason === "evidence_satisfied_retry") return "evidence satisfied";
    if (reason === "context_stale") return "context stale";
    if (reason === "thesis_retry_due") return "thesis retry due";
    if (reason === "maintenance_sweep") return "maintenance";
    return reason ? reason.replace(/_/g, " ") : "";
  }

  function cognitionRunDriver(run: CognitionRun): string {
    const sourceRef = run.source_ref ?? {};
    const sourceTaskAt = typeof sourceRef.source_task_at === "string" ? sourceRef.source_task_at : "";
    const evidenceItemAt = typeof sourceRef.evidence_item_at === "string" ? sourceRef.evidence_item_at : "";
    const parts = [
      cognitionSweepReasonLabel(run.sweep_reason) || cognitionTriggerLabel(run.trigger),
      sourceTaskAt ? `source ${relativeTime(sourceTaskAt)}` : "",
      evidenceItemAt ? `evidence ${relativeTime(evidenceItemAt)}` : "",
    ].filter(Boolean);
    return parts.join(" · ");
  }

  function cognitionRunReason(run: CognitionRun): string {
    const bits = [
      cognitionRunDriver(run),
      run.reason,
      run.thesis_classification ? `classification ${run.thesis_classification}` : "",
      run.evidence_open_count ? `${run.evidence_open_count} open evidence` : "",
      run.evidence_blocking_count ? `${run.evidence_blocking_count} blocking` : "",
    ].filter(Boolean);
    return bits.join(" · ");
  }

  function sourceLabel(source: string): string {
    return source.replace(/_/g, " ");
  }

  function sourceTime(source: BrainSourceStatus): string {
    const parts: string[] = [];
    if (source.last_checked_at) {
      parts.push(`${source.source === "thesis" ? "evaluated" : "checked"} ${relativeTime(source.last_checked_at)}`);
    }
    if (source.last_changed_at) {
      parts.push(`changed ${relativeTime(source.last_changed_at)}`);
    }
    if (source.retry_after_at) {
      parts.push(`retry ${relativeTime(source.retry_after_at)}`);
    }
    return parts.length ? parts.join(" · ") : "not seen";
  }

  function formatCompact(value: number): string {
    return new Intl.NumberFormat("en-US", {
      notation: "compact",
      maximumFractionDigits: 1,
    }).format(value);
  }

  function sourceDetail(source: BrainSourceStatus): string {
    const parts: string[] = [];
    const detail = source.detail ?? {};
    const sourceHealth = source.source_health ?? {};
    const tasks = source.source_tasks ?? [];
    const latestSession = detail.latest_session ?? detail.latest_price_session ?? detail.actual_latest_session;
    const expectedSession = detail.expected_session ?? detail.expected_price_session ?? detail.expected_latest_session;
    const publishedAt = detail.latest_published_at ?? detail.latest_news_published_at;
    const contextAge = detail.context_age_minutes;
    const normalizedItems = detail.normalized_items;
    const evidenceDelta = detail.evidence_delta;
    const companyName = detail.company_name;
    const sector = detail.sector;
    const industry = detail.industry;
    const marketCap = detail.market_cap;
    const earningsEvents = detail.earnings_events;
    const nextEarningsDate = detail.next_earnings_date;
    const rowsSeen = sourceHealth.rows_seen;
    const rowsInserted = sourceHealth.rows_inserted;
    const opinionCounts = [
      ["targets", detail.price_target_snapshots],
      ["ratings", detail.recommendation_snapshots],
      ["target events", detail.price_target_events],
      ["grade changes", detail.rating_events],
    ]
      .filter(([, v]) => typeof v === "number")
      .map(([label, v]) => `${Number(v)} ${label}`);

    if (source.version !== null && source.version !== undefined) parts.push(`v${source.version}`);
    if (source.state) parts.push(source.direction ? `${source.state} ${source.direction}` : source.state);
    if (expectedSession || latestSession) parts.push(`session ${String(latestSession ?? "none")}/${String(expectedSession ?? "expected")}`);
    if (companyName) parts.push(String(companyName));
    if (sector || industry) parts.push([sector, industry].filter(Boolean).join(" / "));
    if (typeof marketCap === "number") parts.push(`market cap ${formatCompact(marketCap)}`);
    if (typeof earningsEvents === "number") parts.push(`${earningsEvents} earnings events`);
    if (nextEarningsDate) parts.push(`next ${String(nextEarningsDate)}`);
    if (publishedAt) parts.push(`published ${relativeTime(String(publishedAt))}`);
    if (opinionCounts.length) parts.push(opinionCounts.join(" · "));
    if (typeof contextAge === "number") parts.push(`context ${Math.round(contextAge)}m old`);
    if (typeof normalizedItems === "number") parts.push(`${normalizedItems} items`);
    if (evidenceDelta === true) parts.push("newer than thesis");
    if (typeof rowsSeen === "number" || typeof rowsInserted === "number") {
      parts.push(`${Number(rowsInserted ?? 0)} new / ${Number(rowsSeen ?? 0)} seen`);
    }
    if (tasks.length) {
      const taskText = tasks
        .slice(0, 3)
        .map((task) => {
          const due = task.due_at ? ` ${relativeTime(task.due_at)}` : "";
          const result = task.result ? ` ${task.result.replace(/_/g, " ")}` : "";
          return `${task.action.replace(/_/g, " ")} ${task.state.replace(/_/g, " ")}${result}${due}`;
        })
        .join(" · ");
      parts.push(`tasks ${taskText}${tasks.length > 3 ? ` +${tasks.length - 3}` : ""}`);
    }
    if (source.max_age_minutes) parts.push(`SLA ${source.max_age_minutes}m`);
    return parts.join(" · ");
  }

  function brainDirectionLabel(direction: string): string {
    if (direction === "risk_on") return "risk on";
    if (direction === "risk_off") return "risk off";
    return direction.replace(/_/g, " ");
  }

  function brainTickerConvictionLabel(ticker: BrainLinkedTicker): string | null {
    const thesisConviction = ticker.thesis_conviction_tier ?? ticker.thesis_system_confidence;
    if (thesisConviction) return thesisConviction.replace(/_/g, " ");
    if (typeof ticker.conviction === "number") return `${Math.round(ticker.conviction)}`;
    return null;
  }

  function brainTickerTitle(ticker: BrainLinkedTicker): string {
    const parts = [`${ticker.symbol} · ${ticker.role.replace(/_/g, " ")}`];
    const conviction = brainTickerConvictionLabel(ticker);
    if (conviction) parts.push(`live conviction ${conviction}`);
    if (typeof ticker.mapping_conviction === "number") {
      parts.push(`mapping ${Math.round(ticker.mapping_conviction)}`);
    }
    if (ticker.link_stale) parts.push("parent link predates latest thesis");
    return parts.join(" · ");
  }

  async function openJournalEntry(entry: BrainJournalEntry) {
    if (!entry.symbol) return;
    routePage = "workspace";
    await selectSymbol(entry.symbol);
    rightTab = entry.thesis_id ? "theses" : "overview";
  }

  async function openJournalSymbol(symbol: string, panel: RightTab = "overview") {
    routePage = "workspace";
    rightTab = panel;
    await selectSymbol(symbol);
    if (panel === "technical") {
      syncSymbolRoute(symbol, true);
      return;
    }
    const item = attention.find((a) => a.symbol === symbol);
    if (item) {
      await openReviewPacketFor(item);
    } else {
      rightTab = "overview";
    }
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

  function asRecord(value: unknown): Record<string, unknown> | null {
    return value && typeof value === "object" && !Array.isArray(value)
      ? value as Record<string, unknown>
      : null;
  }

  function num(value: unknown): number | null {
    const n = Number(value);
    return Number.isFinite(n) ? n : null;
  }

  function pct01(value: unknown): string {
    const n = num(value);
    return n === null ? "n/a" : `${(n * 100).toFixed(0)}%`;
  }

  function macroIndicators(macro: BrainThesis): Record<string, unknown> {
    const maintainerState = asRecord(brainMaintainer(macro.source_ref)?.market_state);
    const fromMaintainer = asRecord(maintainerState?.indicators);
    if (fromMaintainer) return fromMaintainer;
    for (const item of macro.evidence ?? []) {
      const evidence = asRecord(item);
      if (evidence?.kind !== "macro_source_freshness") continue;
      const marketState = asRecord(evidence.market_state);
      const indicators = asRecord(marketState?.indicators);
      if (indicators) return indicators;
    }
    return {};
  }

  function macroMetricChips(macro: BrainThesis): { label: string; value: string; detail?: string }[] {
    const indicators = macroIndicators(macro);
    const breadth = asRecord(indicators.market_breadth_internals);
    const earnings = asRecord(indicators.earnings_breadth);
    const credit = asRecord(indicators.credit_internals_trend);
    const sector = asRecord(indicators.sector_relative_strength);
    const chips: { label: string; value: string; detail?: string }[] = [];
    if (breadth) {
      chips.push({
        label: "breadth",
        value: `${pct01(breadth.pct_above_200d)} >200D`,
        detail: `${Number(breadth.advancers ?? 0)} up / ${Number(breadth.decliners ?? 0)} down`,
      });
    }
    if (earnings) {
      chips.push({
        label: "earnings",
        value: `${Number(earnings.symbol_count ?? 0)} symbols`,
        detail: `net ${pct01(earnings.net_revision_breadth)}`,
      });
    }
    if (sector) {
      const leaders = Array.isArray(sector.leaders_20d)
        ? sector.leaders_20d.filter((item): item is string => typeof item === "string")
        : [];
      if (leaders.length) {
        chips.push({ label: "sector RS", value: leaders.slice(0, 3).join(" / ") });
      }
    }
    if (credit) {
      chips.push({
        label: "credit",
        value: String(credit.trend ?? "unknown"),
        detail: `HY OAS ${Number(credit.latest_hy_oas_pct ?? 0).toFixed(2)}%`,
      });
    }
    return chips;
  }

  type DislocationBucketKey = "loved_mania" | "ignored_indifference" | "hated_avoided";
  type DislocationItem = {
    name?: string;
    score?: number | null;
    reasons?: unknown[];
    interpretation?: string;
    metrics?: Record<string, unknown>;
  };

  function macroDislocationMap(macro: BrainThesis): Record<string, unknown> | null {
    const maintainerMap = asRecord(brainMaintainer(macro.source_ref)?.dislocation_map);
    if (maintainerMap) return maintainerMap;
    for (const item of macro.evidence ?? []) {
      const evidence = asRecord(item);
      if (evidence?.kind !== "macro_dislocation_map") continue;
      const map = asRecord(evidence.dislocation_map);
      if (map) return map;
    }
    return null;
  }

  function dislocationBuckets(macro: BrainThesis): { key: DislocationBucketKey; label: string; items: DislocationItem[] }[] {
    const map = macroDislocationMap(macro);
    const buckets = asRecord(map?.buckets);
    const labels: Record<DislocationBucketKey, string> = {
      loved_mania: "Loved / mania",
      ignored_indifference: "Ignored",
      hated_avoided: "Hated / avoided",
    };
    return (Object.keys(labels) as DislocationBucketKey[]).map((key) => ({
      key,
      label: labels[key],
      items: Array.isArray(buckets?.[key])
        ? (buckets[key] as unknown[]).filter((item): item is DislocationItem =>
          !!item && typeof item === "object" && !Array.isArray(item)
        )
        : [],
    })).filter((bucket) => bucket.items.length > 0);
  }

  function dislocationReason(item: DislocationItem): string {
    const reasons = Array.isArray(item.reasons)
      ? item.reasons.filter((reason): reason is string => typeof reason === "string")
      : [];
    return reasons.slice(0, 2).join(" · ") || item.interpretation || "";
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
      company_profile: "company_profiles",
      company_facts: "company_facts",
      earnings_calendar: "earnings_calendar_events",
      recent_news: "recent_news",
      analyst_estimates: "estimate_snapshots",
      analyst_opinion: "analyst_price_target_snapshots",
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

  function productResearchRequirement(): EvidenceRequirement | null {
    return symbolEvidence?.find((req) => req.requirement_key === "product_research") ?? null;
  }

  function sourceTaskTone(state: string): string {
    if (state === "queued" || state === "fetching") return "running";
    if (state === "rate_limited" || state === "failed" || state === "blocked") return "blocked";
    if (state === "satisfied") return "satisfied";
    return "waiting";
  }

  function sourceTaskLabel(task: NonNullable<EvidenceRequirement["source_tasks"]>[number]): string {
    const provider = task.provider.replace(/_/g, " ");
    const state = task.state.replace(/_/g, " ");
    const due = task.next_retry_at ?? task.due_at;
    return due ? `${provider}: ${state} ${relativeTime(due)}` : `${provider}: ${state}`;
  }

  function evidenceItemTone(item: EvidenceItem): string {
    if (item.polarity === null || item.polarity === undefined) return "neutral";
    if (item.polarity > 0.15) return "positive";
    if (item.polarity < -0.15) return "negative";
    return "neutral";
  }

  function evidenceItemMeta(item: EvidenceItem): string {
    const parts = [
      item.kind.replace(/_/g, " "),
      item.source.replace(/_/g, " "),
      relativeTime(item.observed_at),
    ];
    if (item.strength !== null && item.strength !== undefined) {
      parts.push(`strength ${Math.round(item.strength * 100)}`);
    }
    if (item.polarity !== null && item.polarity !== undefined) {
      const polarity = item.polarity > 0 ? `+${item.polarity.toFixed(2)}` : item.polarity.toFixed(2);
      parts.push(`polarity ${polarity}`);
    }
    return parts.join(" · ");
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
  let symbolEvidenceItems = $state<EvidenceItem[] | undefined>(undefined);
  let symbolResearch = $state<ResearchEvidence[] | undefined>(undefined);
  let symbolTechnical = $state<TechnicalState | null | undefined>(undefined);
  let symbolBrain = $state<BrainStatus | null | undefined>(undefined);
  let symbolWorkflow = $state<ApiSymbolWorkflow | null | undefined>(undefined);
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
  let selectedSymbolAttention = $derived<AttentionItem[]>(
    attention.filter((item) => item.symbol === selectedSymbol),
  );
  let selectedCandidateReviews = $derived<AttentionItem[]>(
    selectedSymbolAttention.filter((item) => item.kind === "candidate_review"),
  );
  let selectedCandidateIds = $derived<number[]>([
    ...new Set(
      selectedCandidateReviews
        .map((item) => item.candidate_id)
        .filter((id): id is number => typeof id === "number"),
    ),
  ]);
  let selectedCandidateReview = $derived<AttentionItem | null>(selectedCandidateReviews[0] ?? null);
  let selectedPendingCandidates = $derived<PendingCandidate[]>(
    selectedCandidateIds
      .map((id) => pending.find((candidate) => candidate.id === id))
      .filter((candidate): candidate is PendingCandidate => !!candidate),
  );
  let selectedPromotionLists = $derived.by(() => {
    const byId = new Map<string, { watchlist_id: string; watchlist_name: string; confidence: string; rationale: string }>();
    for (const candidate of selectedPendingCandidates) {
      for (const proposed of candidate.proposed_lists) {
        if (!proposed.watchlist_id) continue;
        byId.set(proposed.watchlist_id, {
          watchlist_id: proposed.watchlist_id,
          watchlist_name: proposed.watchlist_name,
          confidence: proposed.confidence,
          rationale: proposed.rationale,
        });
      }
    }
    return [...byId.values()];
  });
  let selectedPromotionRawSignals = $derived.by<string[]>(() => [
    ...new Set(selectedCandidateReviews.flatMap(rawSignals).map((signal) => signal.replace(/_/g, " "))),
  ]);
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
  let showAcked = $state(false);

  // ---------- discovery review state (still uses the same model) ----------
  let chosenLists = $state<Record<number, Record<string, boolean>>>({});

  // ---------- watchlist controls ----------
  let newListName = $state("");
  let addSymbolFor = $state<Record<string, string>>({});
  let expandedListIds = $state<Record<string, boolean>>({});
  let watchlistStatusFilter = $state("all");
  let watchlistDirectionFilter = $state("all");
  let watchlistTechnicalFilter = $state("all");
  let watchlistFreshnessFilter = $state("all");
  let watchlistAttentionFilter = $state("all");
  let watchlistThemeFilter = $state("all");
  let poolPromotionBusy = $state(false);
  let poolPromotionStatus = $state<string | null>(null);
  let poolPromotionLists = $state<Record<string, boolean>>({});

  // ---------- decision form (in bottom drawer) ----------
  let decThesisId = $state("");
  let decAction = $state("skip");
  let decSide = $state("none");
  let decInstrument = $state("equity");
  let decChoice = $state("deferred");
  let decDisagreementReason = $state("");
  let decDisagreementDetail = $state("");
  let decHumanConviction = $state("");
  let decReason = $state("");
  let decStatus = $state<string | null>(null);
  let replay = $state<DecisionReplay | null>(null);
  let replayStatus = $state<string | null>(null);
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
  let decNeedsDisagreement = $derived(decAction === "skip" || decChoice === "rejected");

  // Synthetic "Universe" pseudo-list — all active symbols. Computed on the
  // fly from /api/tickers so we don't need a DB-side system list.
  const UNIVERSE_ID = "__universe__";
  let universeList = $derived<Watchlist>({
    id: UNIVERSE_ID,
    name: "Universe",
    description: "All active symbols",
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
      technical_state: t.technical_state,
      entry_stance: t.entry_stance,
      technical_pct_vs_200d: t.technical_pct_vs_200d,
      open_theses: t.open_theses,
      freshness_status: t.freshness_status,
      open_attention: t.open_attention,
      attention_states: t.attention_states,
      attention_owners: t.attention_owners,
      open_evidence: t.open_evidence,
      blocking_evidence: t.blocking_evidence,
      due_source_tasks: t.due_source_tasks,
      parent_themes: t.parent_themes,
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
      technical_state: p.technical_state,
      entry_stance: p.entry_stance,
      technical_pct_vs_200d: p.technical_pct_vs_200d,
      open_theses: p.open_theses ?? 0,
      freshness_status: p.freshness_status,
      open_attention: p.open_attention,
      attention_states: p.attention_states,
      attention_owners: p.attention_owners,
      open_evidence: p.open_evidence,
      blocking_evidence: p.blocking_evidence,
      due_source_tasks: p.due_source_tasks,
      parent_themes: p.parent_themes,
    })),
  );
  let allWatchlists = $derived<Watchlist[]>([...watchlists, universeList, poolList]);
  let watchlistThemeOptions = $derived.by<WatchlistParentTheme[]>(() => {
    const map = new Map<string, WatchlistParentTheme>();
    const remember = (theme: WatchlistParentTheme) => {
      if (!theme.key || map.has(theme.key)) return;
      map.set(theme.key, theme);
    };
    for (const thesis of brainOverview?.sectors ?? []) {
      remember({
        key: thesis.key,
        name: thesis.name,
        scope: thesis.scope,
        state: thesis.state,
        direction: thesis.direction,
        role: "parent",
        conviction: null,
      });
    }
    for (const members of Object.values(watchlistMembers)) {
      for (const m of members) for (const theme of m.parent_themes ?? []) remember(theme);
    }
    for (const m of universeMembers) for (const theme of m.parent_themes ?? []) remember(theme);
    return [...map.values()].sort((a, b) => a.name.localeCompare(b.name));
  });

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

  function decisionActionForThesis(thesis: ThesisDetail): "enter" | "skip" {
    return ["actionable", "armed", "building_conviction"].includes(thesis.state) ? "enter" : "skip";
  }

  function prefillDecision(action = "skip", thesisId: string | null = null) {
    if (thesisId) decThesisId = thesisId;
    else if (currentSymbolThesis) decThesisId = currentSymbolThesis.thesis_id;
    decAction = action;
    if (action === "enter") decChoice = "confirmed";
    if (action === "skip") decChoice = "deferred";
  }

  function openDecisionDrawer(action = "skip", thesisId: string | null = null) {
    prefillDecision(action, thesisId);
    bottomMode = "decisions";
    if (!bottomOpen) bottomPane?.expand();
  }

  function openThesisDecision(thesis: ThesisDetail) {
    openDecisionDrawer(decisionActionForThesis(thesis), thesis.thesis_id);
  }

  function reviewPacketDecisionAction(item: AttentionItem): "enter" | "skip" {
    return item.kind === "thesis_actionable" ? "enter" : "skip";
  }

  function openReviewPacketDecision(action: "enter" | "skip", item: AttentionItem) {
    prefillDecision(action, item.thesis_id ?? null);
    reviewPacketDecisionOpen = true;
    setTimeout(() => {
      document.querySelector('[data-testid="review-packet-decision-form"]')?.scrollIntoView({ behavior: "smooth", block: "start" });
    }, 50);
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

  function freshnessLabel(status: string | null | undefined): string {
    return status ? status.replace(/_/g, " ") : "missing";
  }

  function freshnessClass(status: string | null | undefined): string {
    return `fresh-${status ?? "missing"}`;
  }

  function freshnessTitle(m: WatchlistMember): string {
    const parts = [
      `${freshnessLabel(m.freshness_status)} brain inputs`,
      `${m.open_evidence ?? 0} open evidence`,
      `${m.blocking_evidence ?? 0} blocking evidence`,
      `${m.due_source_tasks ?? 0} due source tasks`,
    ];
    return parts.join(" · ");
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

  function technicalStateLabel(state: string | null | undefined): string {
    return state ? state.replace(/_/g, " ") : "no technicals";
  }

  function technicalStateClass(state: string | null | undefined): string {
    return `tech-${state ?? "none"}`;
  }

  function entryStanceLabel(stance: string | null | undefined): string {
    return stance ? stance.replace(/_/g, " ") : "wait data";
  }

  function entryStanceClass(stance: string | null | undefined): string {
    return `stance-${stance ?? "none"}`;
  }

  function themesForMember(m: WatchlistMember): WatchlistParentTheme[] {
    if (m.parent_themes?.length) return m.parent_themes;
    const matches: WatchlistParentTheme[] = [];
    for (const thesis of brainOverview?.sectors ?? []) {
      const linked = thesis.tickers.find((t) => t.symbol === m.symbol);
      if (!linked) continue;
      matches.push({
        key: thesis.key,
        name: thesis.name,
        scope: thesis.scope,
        state: thesis.state,
        direction: thesis.direction,
        role: linked.role,
        conviction: linked.conviction ?? null,
      });
    }
    return matches;
  }

  function themeShortName(theme: WatchlistParentTheme): string {
    return theme.name
      .replace(/\btheme\b/gi, "")
      .replace(/\bsector\b/gi, "")
      .replace(/\s+/g, " ")
      .trim();
  }

  function hasAttentionOwner(m: WatchlistMember, owner: string): boolean {
    return (m.attention_owners ?? []).some((item) => item.owner === owner && item.count > 0);
  }

  function hasAttentionState(m: WatchlistMember, state: string): boolean {
    return (m.attention_states ?? []).some((item) => item.state === state && item.count > 0);
  }

  function attentionLabel(m: WatchlistMember): string {
    const n = m.open_attention ?? 0;
    return n === 1 ? "1 attention" : `${n} attention`;
  }

  function selectedAttentionCount(): number {
    return selectedSymbolAttention.length || selectedTicker?.open_attention || 0;
  }

  function blockingEvidenceCount(): number {
    return (symbolEvidence ?? []).filter((req) =>
      req.priority === "blocking" && req.blocking_state !== "satisfied"
    ).length;
  }

  function openEvidenceCount(): number {
    return (symbolEvidence ?? []).filter((req) => req.blocking_state !== "satisfied").length;
  }

  function workflowEvidenceText(): string {
    if (symbolEvidence === undefined || symbolBrain === undefined) return "loading evidence";
    const blocking = blockingEvidenceCount();
    const open = openEvidenceCount();
    if (blocking > 0) return `${blocking} blocking evidence`;
    if (open > 0) return `${open} open evidence`;
    if (symbolBrain) return `${symbolBrain.evidence.rows} facts · ${symbolBrain.status}`;
    return "evidence ready";
  }

  function workflowThesisText(): string {
    if (symbolTheses === undefined || symbolDeclines === undefined) return "loading thesis";
    if (currentSymbolThesis) {
      const direction = thesisDirectionLabel(forecastDirectionFrom(currentSymbolThesis.forecast));
      return `${currentSymbolThesis.state.replace(/_/g, " ")} · ${direction}`;
    }
    if (selectedCandidateReview) return "nominated";
    if ((symbolDeclines ?? []).length > 0) return "declined attempt";
    return "no thesis";
  }

  function workflowDecisionText(): string {
    if (symbolDecisions === undefined || symbolPositions === undefined) return "loading decisions";
    if (openSymbolPositions.length > 0) return `${openSymbolPositions.length} open position`;
    const pendingFill = pendingManualFillDecision();
    if (pendingFill) return "manual fill needed";
    if ((symbolDecisions ?? []).length > 0) return `${symbolDecisions?.length ?? 0} decision`;
    return "no decision";
  }

  function pendingManualFillDecision(): DecisionRow | null {
    if (openSymbolPositions.length > 0) return null;
    return (symbolDecisions ?? []).find((d) =>
      ["enter", "resize"].includes(d.action) && d.user_choice === "confirmed"
    ) ?? null;
  }

  function workflowAttentionText(): string {
    const n = selectedAttentionCount();
    if (n > 0) return n === 1 ? "1 attention" : `${n} attention`;
    return "no attention";
  }

  function apiWorkflowStep(workflow: ApiSymbolWorkflow, key: string, fallback: string): string {
    return workflow.steps.find((step) => step.key === key)?.value ?? fallback;
  }

  function workflowFromApi(workflow: ApiSymbolWorkflow): SymbolWorkflow {
    return {
      state: workflow.state_label,
      tone: workflow.tone,
      reason: workflow.reason,
      primary: workflow.primary_action.label,
      action: workflow.primary_action.kind,
      primaryDetail: workflow.primary_action.detail ?? null,
      reviewPacketAttentionId: workflow.primary_action.attention_id ?? workflow.review_packet_attention_id ?? null,
      attentionItems: workflow.attention ?? [],
      steps: workflow.steps,
      status: apiWorkflowStep(workflow, "status", "unknown"),
      attention: apiWorkflowStep(workflow, "attention", "no attention"),
      evidence: apiWorkflowStep(workflow, "evidence", "evidence ready"),
      thesis: apiWorkflowStep(workflow, "thesis", "no thesis"),
      decision: apiWorkflowStep(workflow, "decision", "no decision"),
    };
  }

  function workflowLoading(): boolean {
    return [
      symbolContext,
      symbolEvidence,
      symbolEvidenceItems,
      symbolResearch,
      symbolTechnical,
      symbolBrain,
      symbolTheses,
      symbolDeclines,
      symbolDecisions,
      symbolPositions,
    ].some((value) => value === undefined);
  }

  function buildWorkflow(): SymbolWorkflow {
    const defaultWorkflow: SymbolWorkflow = {
      state: "No symbol",
      tone: "missing",
      reason: "Pick a symbol to inspect.",
      primary: "Overview",
      action: "overview",
      status: "not selected",
      attention: "no attention",
      evidence: "no evidence",
      thesis: "no thesis",
      decision: "no decision",
    };
    if (!selectedSymbol) return defaultWorkflow;
    if (symbolWorkflow && symbolWorkflow.symbol.toUpperCase() === selectedSymbol.toUpperCase()) {
      return workflowFromApi(symbolWorkflow);
    }

    const attentionText = workflowAttentionText();
    const evidenceText = workflowEvidenceText();
    const thesisText = workflowThesisText();
    const decisionText = workflowDecisionText();
    const statusText = selectedPlacement.short;
    const inPool = pool.some((item) => item.symbol === selectedSymbol);

    if (selectedCandidateReview && !currentSymbolThesis) {
      return {
        state: "Nominated, not active",
        tone: "candidate",
        reason: candidateNominationReason(selectedCandidateReview),
        primary: "Start research",
        action: "promote",
        status: statusText,
        attention: attentionText,
        evidence: evidenceText,
        thesis: thesisText,
        decision: decisionText,
      };
    }
    if (!selectedTicker) {
      return {
        state: inPool ? "Pool candidate" : "Not active",
        tone: "candidate",
        reason: selectedPlacement.detail,
        primary: "Start research",
        action: "promote",
        status: statusText,
        attention: attentionText,
        evidence: evidenceText,
        thesis: thesisText,
        decision: decisionText,
      };
    }
    if (workflowLoading()) {
      return {
        state: "Loading symbol",
        tone: "monitoring",
        reason: "Loading context, evidence, thesis, and decision state.",
        primary: "Overview",
        action: "overview",
        status: statusText,
        attention: attentionText,
        evidence: evidenceText,
        thesis: thesisText,
        decision: decisionText,
      };
    }
    if (blockingEvidenceCount() > 0 || !symbolContext) {
      return {
        state: symbolContext ? "Enriching evidence" : "Context missing",
        tone: "blocked",
        reason: symbolBrain?.reason ?? "Evidence/context is not ready for thesis work.",
        primary: selectedTicker ? "Start research" : "Open evidence",
        action: selectedTicker ? "research" : "evidence",
        status: statusText,
        attention: attentionText,
        evidence: evidenceText,
        thesis: thesisText,
        decision: decisionText,
      };
    }
    if (openSymbolPositions.length > 0) {
      return {
        state: "Position tracking",
        tone: "tracking",
        reason: "A position is open; conditions and exits matter now.",
        primary: "Track position",
        action: "tracking",
        status: statusText,
        attention: attentionText,
        evidence: evidenceText,
        thesis: thesisText,
        decision: decisionText,
      };
    }
    if (pendingManualFillDecision()) {
      return {
        state: "Fill needed",
        tone: "actionable",
        reason: "A confirmed decision exists, but no open position is recorded yet.",
        primary: "Record fill",
        action: "decision",
        status: statusText,
        attention: attentionText,
        evidence: evidenceText,
        thesis: thesisText,
        decision: decisionText,
      };
    }
    if ((symbolDecisions ?? []).length > 0) {
      return {
        state: "Decision recorded",
        tone: "tracking",
        reason: "A decision exists; review replay and follow-up conditions.",
        primary: "Track decision",
        action: "tracking",
        status: statusText,
        attention: attentionText,
        evidence: evidenceText,
        thesis: thesisText,
        decision: decisionText,
      };
    }
    if (currentSymbolThesis) {
      const state = currentSymbolThesis.state;
      const isActionable = ["actionable", "armed", "building_conviction"].includes(state);
      return {
        state: isActionable ? "Actionable thesis" : "Monitoring thesis",
        tone: isActionable ? "actionable" : "monitoring",
        reason: currentSymbolThesis.edge_rationale,
        primary: isActionable ? "Record decision" : "Review thesis",
        action: isActionable ? "decision" : "thesis",
        status: statusText,
        attention: attentionText,
        evidence: evidenceText,
        thesis: thesisText,
        decision: decisionText,
      };
    }
    if ((symbolDeclines ?? []).length > 0) {
      return {
        state: "Declined thesis",
        tone: "declined",
        reason: symbolDeclines?.[0]?.reason ?? "The system declined to invent an edge.",
        primary: "Review decline",
        action: "thesis",
        status: statusText,
        attention: attentionText,
        evidence: evidenceText,
        thesis: thesisText,
        decision: decisionText,
      };
    }
    return {
      state: "Context ready",
      tone: "ready",
      reason: symbolBrain?.reason ?? "Context exists; cognition should draft or decline a thesis.",
      primary: "Check cognition",
      action: "overview",
      status: statusText,
      attention: attentionText,
      evidence: evidenceText,
      thesis: thesisText,
      decision: decisionText,
    };
  }

  function runWorkflowAction(action: WorkflowAction) {
    if (action === "promote") {
      void promoteSelectedToUniverse();
      return;
    }
    if (action === "research") {
      void startResearchForSelected();
      return;
    }
    if (action === "promotion") {
      rightTab = "overview";
      return;
    }
    if (action === "attention") {
      bottomMode = selectedSymbolAttention.length > 0 ? "attention" : "discovery";
      if (!bottomOpen) bottomPane?.expand();
      return;
    }
    if (action === "evidence") {
      rightTab = "evidence";
      return;
    }
    if (action === "thesis") {
      rightTab = "theses";
      return;
    }
    if (action === "decision") {
      openDecisionDrawer("enter");
      return;
    }
    if (action === "tracking") {
      rightTab = "decisions";
      return;
    }
    rightTab = "overview";
  }

  function runSelectedWorkflowPrimary() {
    if (selectedWorkflow.reviewPacketAttentionId && selectedWorkflow.action === "attention") {
      void openReviewPacketById(selectedWorkflow.reviewPacketAttentionId, selectedSymbol);
      return;
    }
    runWorkflowAction(selectedWorkflow.action);
  }

  function pctCompact(value: number | null | undefined): string {
    if (value === null || value === undefined || Number.isNaN(value)) return "";
    const sign = value > 0 ? "+" : "";
    return `${sign}${value.toFixed(0)}%`;
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

  function disagreementLabel(value: string | null | undefined): string {
    if (!value) return "";
    return DISAGREEMENT_REASONS.find((r) => r.value === value)?.label ?? value.replace(/_/g, " ");
  }

  function visibleSizing(d: DecisionRow): Record<string, unknown> | null {
    const entries = Object.entries(d.sizing ?? {}).filter(
      ([k]) => !["side", "instrument", "thesis_direction"].includes(k),
    );
    return entries.length > 0 ? Object.fromEntries(entries) : null;
  }

  function updateChartState(next: { interval: string; range: string }) {
    chartState = next;
  }

  async function openReplay(decisionId: string) {
    replayStatus = "loading replay…";
    replay = null;
    try {
      replay = await fetchDecisionReplay(decisionId);
      replayStatus = null;
    } catch (e) {
      replayStatus = `replay unavailable: ${e}`;
    }
  }

  function replayThesisText(r: DecisionReplay | null): string {
    const thesis = r?.thesis_snapshot ?? {};
    const state = typeof thesis.state === "string" ? thesis.state.replace(/_/g, " ") : "unknown";
    const version = typeof thesis.version === "number" ? `v${thesis.version}` : "v?";
    const direction = (thesis.forecast as Record<string, unknown> | undefined)?.direction;
    return [version, state, typeof direction === "string" ? direction : null].filter(Boolean).join(" · ");
  }

  function replayRiskText(r: DecisionReplay | null): string {
    const risk = r?.risk_verdict ?? {};
    const status = typeof risk.status === "string" ? risk.status : "not captured";
    const reasons = Array.isArray(risk.reasons) ? risk.reasons.filter((x): x is string => typeof x === "string") : [];
    const warnings = Array.isArray(risk.warnings) ? risk.warnings.filter((x): x is string => typeof x === "string") : [];
    const detail = [...reasons, ...warnings].slice(0, 2).join(" · ");
    return detail ? `${status}: ${detail}` : status;
  }

  function replaySnapshotString(r: DecisionReplay | null, key: string): string {
    const value = r?.decision_snapshot?.[key];
    return typeof value === "string" ? value : "";
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

  function watchlistsForSymbol(symbol: string | null): Watchlist[] {
    if (!symbol) return [];
    return watchlists.filter((w) =>
      (watchlistMembers[w.id] ?? []).some((m) => m.symbol === symbol),
    );
  }

  function watchlistSummary(lists: Watchlist[]): string {
    if (lists.length === 0) return "no watchlist";
    return lists.map((w) => w.name).slice(0, 3).join(", ") + (lists.length > 3 ? ` +${lists.length - 3}` : "");
  }

  function placementForSelectedSymbol(): SymbolPlacement {
    if (!selectedSymbol) {
      return {
        label: "No symbol",
        short: "not selected",
        tone: "missing",
        location: "None",
        detail: "Pick a symbol to inspect.",
      };
    }
    if (selectedTicker) {
      return {
        label: "Active Universe",
        short: `Universe T${selectedTicker.tier}`,
        tone: "active",
        location: "Active Universe",
        detail: "The scheduled brain loop may refresh context, evidence, thesis, and decisions for this symbol.",
      };
    }
    if (selectedCandidateReview) {
      return {
        label: "Nominated",
        short: "nomination",
        tone: "candidate",
        location: "Attention queue",
        detail: "Discovery queued this symbol for promotion review. It is not monitored until promotion adds it to the active Universe.",
      };
    }
    if (selectedPoolMember) {
      return {
        label: "Discovery Pool",
        short: "pool only",
        tone: "pool",
        location: "Discovery pool",
        detail: "This symbol is known to discovery, but the scheduled context/thesis loop will not run until it is promoted.",
      };
    }
    if (selectedWatchlistPlacements.length > 0) {
      return {
        label: "Watchlisted Only",
        short: "watchlist only",
        tone: "pool",
        location: "Watchlist",
        detail: "This symbol is in a watchlist, but it is not in the active Universe, so the scheduled context/thesis loop will not run until it is promoted.",
      };
    }
    return {
      label: "Not Tracked",
      short: "not tracked",
      tone: "unknown",
      location: "Outside current system",
      detail: "The chart may load if price data exists, but this symbol is not in the active Universe, discovery pool, or watchlists.",
    };
  }

  function filteredMembersFor(listId: string): WatchlistMember[] {
    return membersFor(listId).filter((m) => {
      if (watchlistStatusFilter !== "all") {
        const status = m.thesis_state ?? "none";
        if (status !== watchlistStatusFilter) return false;
      }
      if (watchlistDirectionFilter !== "all") {
        const direction = m.thesis_direction ?? "none";
        if (direction !== watchlistDirectionFilter) return false;
      }
      if (watchlistTechnicalFilter !== "all") {
        const state = m.technical_state ?? "unknown";
        if (state !== watchlistTechnicalFilter) return false;
      }
      if (watchlistFreshnessFilter !== "all") {
        const freshness = m.freshness_status ?? "missing";
        if (watchlistFreshnessFilter === "stale_missing") {
          if (!["stale", "missing", "blocked"].includes(freshness)) return false;
        } else if (freshness !== watchlistFreshnessFilter) {
          return false;
        }
      }
      if (watchlistAttentionFilter !== "all") {
        if (watchlistAttentionFilter === "open") {
          if ((m.open_attention ?? 0) <= 0) return false;
        } else if (watchlistAttentionFilter.startsWith("owner:")) {
          if (!hasAttentionOwner(m, watchlistAttentionFilter.slice("owner:".length))) return false;
        } else if (watchlistAttentionFilter.startsWith("state:")) {
          if (!hasAttentionState(m, watchlistAttentionFilter.slice("state:".length))) return false;
        }
      }
      if (watchlistThemeFilter !== "all") {
        if (!themesForMember(m).some((theme) => theme.key === watchlistThemeFilter)) return false;
      }
      return true;
    });
  }

  function resetWatchlistFilters() {
    watchlistStatusFilter = "all";
    watchlistDirectionFilter = "all";
    watchlistTechnicalFilter = "all";
    watchlistFreshnessFilter = "all";
    watchlistAttentionFilter = "all";
    watchlistThemeFilter = "all";
  }

  function watchlistFiltersActive(): boolean {
    return [
      watchlistStatusFilter,
      watchlistDirectionFilter,
      watchlistTechnicalFilter,
      watchlistFreshnessFilter,
      watchlistAttentionFilter,
      watchlistThemeFilter,
    ].some((value) => value !== "all");
  }

  function normalizeSymbol(value: string | null | undefined): string | null {
    const symbol = (value ?? "").trim().toUpperCase();
    if (!/^(?=.{1,14}$)[A-Z0-9]+(?:[.-][A-Z0-9]+)*$/.test(symbol)) return null;
    return symbol;
  }

  function todayIsoDate(): string {
    return new Date().toISOString().slice(0, 10);
  }

  function normalizeDate(value: string | null | undefined): string | null {
    if (!value || !/^\d{4}-\d{2}-\d{2}$/.test(value)) return null;
    return value;
  }

  function symbolFromRoute(): string | null {
    const match = window.location.pathname.match(/^\/symbol\/([^/]+)\/?$/);
    return match ? normalizeSymbol(decodeURIComponent(match[1])) : null;
  }

  function panelFromRoute(): RightTab {
    const panel = new URLSearchParams(window.location.search).get("p");
    return RIGHT_TABS.includes(panel as RightTab) ? panel as RightTab : "overview";
  }

  function journalFromRoute(): { date: string; page: number } | null {
    const match = window.location.pathname.match(/^\/journal(?:\/(\d{4}-\d{2}-\d{2}))?\/?$/);
    if (!match) return null;
    const params = new URLSearchParams(window.location.search);
    const parsedPage = Number(params.get("page") ?? "1");
    return {
      date: normalizeDate(match[1]) ?? todayIsoDate(),
      page: Number.isFinite(parsedPage) && parsedPage > 0 ? Math.floor(parsedPage) : 1,
    };
  }

  function automationFromRoute(): { symbol: string | null } | null {
    const match = window.location.pathname.match(/^\/automation(?:\/([^/]+))?\/?$/);
    if (!match) return null;
    return { symbol: match[1] ? normalizeSymbol(decodeURIComponent(match[1])) : null };
  }

  function syncSymbolRoute(symbol: string, replace = false) {
    const path = `/symbol/${encodeURIComponent(symbol)}?p=${encodeURIComponent(rightTab)}`;
    if (`${window.location.pathname}${window.location.search}` === path) return;
    const method = replace ? "replaceState" : "pushState";
    window.history[method](null, "", path);
  }

  function syncJournalRoute(date: string, page = 1, replace = false) {
    const query = page > 1 ? `?page=${page}` : "";
    const path = `/journal/${date}${query}`;
    if (`${window.location.pathname}${window.location.search}` === path) return;
    const method = replace ? "replaceState" : "pushState";
    window.history[method](null, "", path);
  }

  function syncAutomationRoute(symbol: string | null = automationSymbol, replace = false) {
    const path = symbol ? `/automation/${encodeURIComponent(symbol)}` : "/automation";
    if (`${window.location.pathname}${window.location.search}` === path) return;
    const method = replace ? "replaceState" : "pushState";
    window.history[method](null, "", path);
  }

  async function loadBrainJournal(date = journalDate, page = journalPage, opts: { silent?: boolean } = {}) {
    const seq = ++journalLoadSeq;
    if (!opts.silent) journalLoading = true;
    journalError = null;
    try {
      const next = await fetchBrainJournal({ date, page, perPage: JOURNAL_PER_PAGE });
      if (seq !== journalLoadSeq) return;
      brainJournal = next;
      journalDate = next.date;
      journalPage = next.pagination?.page ?? page;
    } catch (e) {
      if (seq !== journalLoadSeq) return;
      journalError = e instanceof Error ? e.message : String(e);
    } finally {
      if (seq === journalLoadSeq) journalLoading = false;
    }
  }

  function openJournalPage(date = journalDate, page = journalPage, replace = false) {
    routePage = "journal";
    journalDate = normalizeDate(date) ?? todayIsoDate();
    journalPage = page > 0 ? page : 1;
    syncJournalRoute(journalDate, journalPage, replace);
    void loadBrainJournal(journalDate, journalPage);
  }

  function openAutomationPage(symbol: string | null = automationSymbol, replace = false) {
    routePage = "automation";
    automationSymbol = symbol ? normalizeSymbol(symbol) : null;
    syncAutomationRoute(automationSymbol, replace);
  }

  function changeJournalDate(date: string) {
    openJournalPage(date, 1);
  }

  function changeJournalPage(page: number) {
    openJournalPage(journalDate, page);
  }

  function openWorkspace(replace = false) {
    routePage = "workspace";
    const path = selectedSymbol ? `/symbol/${encodeURIComponent(selectedSymbol)}?p=${encodeURIComponent(rightTab)}` : "/";
    const current = `${window.location.pathname}${window.location.search}`;
    if (current === path) return;
    const method = replace ? "replaceState" : "pushState";
    window.history[method](null, "", path);
  }

  function clearSelectedSymbolDetails() {
    symbolContext = undefined;
    symbolEvidence = undefined;
    symbolEvidenceItems = undefined;
    symbolResearch = undefined;
    symbolTechnical = undefined;
    symbolBrain = undefined;
    symbolWorkflow = undefined;
    symbolTheses = undefined;
    symbolDeclines = undefined;
    symbolDecisions = undefined;
    symbolPositions = undefined;
    replay = null;
    replayStatus = null;
  }

  async function loadSelectedSymbolDetails(symbol: string) {
    const [ctx, evidence, evidenceItems, research, technical, brain, workflow, theses, declines, decisions, positions] = await Promise.all([
      fetchTickerContext(symbol).catch(() => null),
      fetchEvidenceRequirements(symbol).catch(() => []),
      fetchEvidenceItems(symbol).catch(() => []),
      fetchResearchEvidence(symbol).catch(() => []),
      fetchTechnicalState(symbol).catch(() => null),
      fetchBrainStatus(symbol).catch(() => null),
      fetchSymbolWorkflow(symbol).catch(() => null),
      fetchTheses(symbol).catch(() => []),
      fetchThesisDeclines(symbol).catch(() => []),
      fetchDecisions(symbol).catch(() => []),
      fetchPositions(symbol).catch(() => []),
    ]);
    if (selectedSymbol !== symbol) return;
    symbolContext = ctx;
    symbolEvidence = evidence;
    symbolEvidenceItems = evidenceItems;
    symbolResearch = research;
    symbolTechnical = technical;
    symbolBrain = brain;
    symbolWorkflow = workflow;
    symbolTheses = theses;
    symbolDeclines = declines;
    symbolDecisions = decisions;
    symbolPositions = positions;
  }

  async function reloadSelectedSymbolDetails() {
    if (!selectedSymbol) return;
    const symbol = selectedSymbol;
    clearSelectedSymbolDetails();
    await loadSelectedSymbolDetails(symbol);
  }

  async function refreshSelectedWatchlistMembers(ids: string[]) {
    const uniqueIds = [...new Set(ids)];
    if (uniqueIds.length === 0) return;
    const updates = await Promise.all(
      uniqueIds.map((id) =>
        fetchWatchlistMembers(id)
          .then((members) => [id, members] as const)
          .catch(() => null),
      ),
    );
    const next = { ...watchlistMembers };
    for (const update of updates) {
      if (update) next[update[0]] = update[1];
    }
    watchlistMembers = next;
  }

  async function refreshKnownWatchlistMembers() {
    const ids = watchlists.map((w) => w.id).filter((id) => !watchlistMembers[id]);
    if (ids.length === 0) return;
    await refreshSelectedWatchlistMembers(ids);
  }

  // ---------- selection logic ----------
  async function selectSymbol(
    value: string,
    opts: { updateRoute?: boolean; replaceRoute?: boolean } = {},
  ) {
    const symbol = normalizeSymbol(value);
    if (!symbol) return;
    if (opts.updateRoute ?? true) {
      routePage = "workspace";
      syncSymbolRoute(symbol, opts.replaceRoute ?? false);
    }
    if (selectedSymbol === symbol) return;
    selectedSymbol = symbol;
    promotionStatus = null;
    poolPromotionStatus = null;
    poolPromotionLists = {};
    clearSelectedSymbolDetails();
    await Promise.all([
      loadSelectedSymbolDetails(symbol),
      refreshKnownWatchlistMembers(),
    ]);
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
  function selectedPromotionListChecked(wlId: string): boolean {
    return selectedCandidateIds.some((cid) => chosenLists[cid]?.[wlId]);
  }
  function setSelectedPromotionList(wlId: string, checked: boolean) {
    const next = { ...chosenLists };
    for (const cid of selectedCandidateIds) {
      next[cid] = { ...(next[cid] ?? {}), [wlId]: checked };
    }
    chosenLists = next;
  }
  function selectedPoolPromotionWatchlists(): string[] {
    return Object.entries(poolPromotionLists).filter(([, checked]) => checked).map(([id]) => id);
  }
  function setPoolPromotionList(wlId: string, checked: boolean) {
    poolPromotionLists = { ...poolPromotionLists, [wlId]: checked };
  }
  async function promoteSelectedToUniverse() {
    if (!selectedSymbol || selectedTicker) return;
    if (selectedCandidateIds.length > 0) {
      await confirmGroup(selectedCandidateIds);
      return;
    }
    await promoteSelectedPoolCandidate();
  }
  async function promoteSelectedPoolCandidate() {
    if (!selectedSymbol) return;
    const watchlistIds = selectedPoolPromotionWatchlists();
    poolPromotionBusy = true;
    poolPromotionStatus = null;
    try {
      await promoteTicker(selectedSymbol, watchlistIds);
      await Promise.all([
        fetchTickers().then((t) => (tickers = t)),
        refreshSelectedWatchlistMembers(watchlistIds),
        refreshAttention(),
        refreshPending(),
        fetchDiscoveryPool().then((p) => (pool = p)).catch(() => {}),
      ]);
      await reloadSelectedSymbolDetails();
      poolPromotionStatus = watchlistIds.length > 0
        ? `Research started in Universe + ${watchlistIds.length} watchlist${watchlistIds.length === 1 ? "" : "s"}.`
        : "Research started in Universe.";
    } catch (e) {
      error = String(e);
    } finally {
      poolPromotionBusy = false;
    }
  }
  async function confirmOne(candId: number) {
    const inner = chosenLists[candId] ?? {};
    const ids = Object.entries(inner).filter(([, v]) => v).map(([k]) => k);
    try {
      await confirmCandidate(candId, ids);
      await Promise.all([
        refreshPending(),
        refreshWatchlists(),
        refreshSelectedWatchlistMembers(ids),
        fetchTickers().then((t) => (tickers = t)),
      ]);
      await reloadSelectedSymbolDetails();
    } catch (e) {
      error = String(e);
    }
  }
  async function rejectOne(candId: number) {
    try {
      await rejectCandidate(candId);
      await refreshPending();
      await reloadSelectedSymbolDetails();
    } catch (e) {
      error = String(e);
    }
  }

  // ---------- watchlists CRUD ----------
  async function refreshWatchlists() {
    try {
      watchlists = await fetchWatchlists();
      if (selectedSymbol) await refreshKnownWatchlistMembers();
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
    if (decNeedsDisagreement && !decDisagreementReason) {
      decStatus = "choose why you disagree";
      return;
    }
    if (decDisagreementReason === "other" && !decDisagreementDetail.trim()) {
      decStatus = "describe the disagreement";
      return;
    }
    if (!decHumanConviction) {
      decStatus = "choose human conviction";
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
        disagreement_reason: decDisagreementReason || undefined,
        disagreement_detail: decDisagreementDetail.trim() || undefined,
        human_conviction: decHumanConviction,
        reason: decReason.trim() || undefined,
        sizing: Object.keys(sizing).length > 0 ? sizing : undefined,
        manual_fill,
        chart_range_seen: `${chartState.range} ${chartState.interval}`,
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
          routePage === "journal" ? loadBrainJournal(journalDate, journalPage) : Promise.resolve(),
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
    const shouldPoll = routePage === "journal" && journalDate === todayIsoDate();
    if (shouldPoll) {
      journalRefreshTimer = setInterval(() => {
        void loadBrainJournal(journalDate, journalPage, { silent: true });
      }, JOURNAL_REFRESH_MS);
      return () => {
        if (journalRefreshTimer) { clearInterval(journalRefreshTimer); journalRefreshTimer = null; }
      };
    }
  });

  $effect(() => {
    // Once tickers and watchlists arrive, auto-pick the first symbol.
    if (routePage === "workspace" && !selectedSymbol && (tickers.length > 0 || watchlists.length > 0)) {
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

  $effect(() => {
    if (!decNeedsDisagreement) {
      decDisagreementReason = "";
      decDisagreementDetail = "";
    }
  });

  onMount(() => {
    const routedAutomation = automationFromRoute();
    const routedJournal = journalFromRoute();
    if (routedAutomation) {
      openAutomationPage(routedAutomation.symbol, true);
    } else if (routedJournal) {
      openJournalPage(routedJournal.date, routedJournal.page, true);
    } else {
      const routedSymbol = symbolFromRoute();
      rightTab = panelFromRoute();
      if (routedSymbol) void selectSymbol(routedSymbol, { replaceRoute: true });
    }
    refreshAll();
    const onPopState = () => {
      const automation = automationFromRoute();
      if (automation) {
        routePage = "automation";
        automationSymbol = automation.symbol;
        return;
      }
      const journal = journalFromRoute();
      if (journal) {
        routePage = "journal";
        journalDate = journal.date;
        journalPage = journal.page;
        void loadBrainJournal(journal.date, journal.page);
        return;
      }
      routePage = "workspace";
      rightTab = panelFromRoute();
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
          if (routePage === "journal") void loadBrainJournal(journalDate, journalPage);
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

  $effect(() => {
    if (routePage === "workspace" && selectedSymbol) {
      syncSymbolRoute(selectedSymbol, true);
    }
  });

  let selectedTicker = $derived(tickerFor(selectedSymbol));
  let selectedPoolMember = $derived(pool.find((item) => item.symbol === selectedSymbol) ?? null);
  let selectedWatchlistPlacements = $derived.by<Watchlist[]>(() => watchlistsForSymbol(selectedSymbol));
  let selectedPlacement = $derived.by<SymbolPlacement>(() => placementForSelectedSymbol());
  let selectedWorkflow = $derived.by<SymbolWorkflow>(() => buildWorkflow());
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

{#snippet decisionForm()}
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
        <option value="none">choose side...</option>
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
    <label>
      Human conviction
      <select bind:value={decHumanConviction}>
        <option value="">choose conviction...</option>
        <option value="low">low</option>
        <option value="medium">medium</option>
        <option value="high">high</option>
      </select>
    </label>
    <label class="wide">
      Decision reason
      <textarea
        bind:value={decReason}
        rows="2"
        placeholder="optional operator rationale"
      ></textarea>
    </label>
    {#if decNeedsDisagreement}
      <label>
        Why
        <select bind:value={decDisagreementReason}>
          <option value="">choose reason...</option>
          {#each DISAGREEMENT_REASONS as reason (reason.value)}
            <option value={reason.value}>{reason.label}</option>
          {/each}
        </select>
      </label>
      <label class="wide">
        Detail
        <textarea
          bind:value={decDisagreementDetail}
          rows="2"
          placeholder={decDisagreementReason === "other" ? "required for other" : "optional"}
        ></textarea>
      </label>
    {/if}
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
{/snippet}

<div class="workspace">
  <!-- Top bar: symbol + regime + connection -->
  <header class="top">
    <div class="brand">stocks <span class="muted">intel</span></div>

    <nav class="top-nav" aria-label="Primary">
      <button type="button" class:active={routePage === "workspace"} onclick={() => openWorkspace()}>
        Workspace
      </button>
      <button type="button" class:active={routePage === "journal"} onclick={() => openJournalPage()}>
        Journal
      </button>
      <button type="button" class:active={routePage === "automation"} onclick={() => openAutomationPage(selectedSymbol)}>
        Autonomous
      </button>
    </nav>

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

  {#if routePage === "journal"}
    <BrainJournalPage
      journal={brainJournal}
      date={journalDate}
      today={todayIsoDate()}
      loading={journalLoading}
      error={journalError}
      onDateChange={changeJournalDate}
      onPageChange={changeJournalPage}
      onOpenEntry={openJournalEntry}
      onOpenSymbol={openJournalSymbol}
      onOpenReviewPacket={(id, symbol) => openReviewPacketById(id, symbol)}
      onStartResearch={(symbol) => startResearchForSymbol(symbol, { refreshJournal: true })}
      researchBusySymbol={researchKickoffSymbol}
      researchStatus={researchKickoffStatus}
      onBack={() => openWorkspace()}
    />
  {:else if routePage === "automation"}
    <AutonomousTradingPage
      symbol={automationSymbol}
      onFilterSymbol={(nextSymbol) => openAutomationPage(nextSymbol)}
      onOpenWorkspace={(nextSymbol) => {
        void selectSymbol(nextSymbol);
      }}
      onBack={() => openWorkspace()}
    />
  {:else}
  <section class={`workflow-strip tone-${selectedWorkflow.tone}`} data-testid="workflow-strip">
    <div class="workflow-main">
      <div class="workflow-copy">
        <span class="workflow-kicker">workflow</span>
        <strong>{selectedSymbol ?? "No symbol"} · {selectedWorkflow.state}</strong>
        <p title={selectedWorkflow.reason}>{selectedWorkflow.reason}</p>
      </div>
      <button
        type="button"
        class="workflow-primary"
        data-testid="workflow-primary"
        title={selectedWorkflow.primaryDetail ?? selectedWorkflow.reason}
        disabled={(researchKickoffBusy && selectedWorkflow.action === "research") || (reviewPacketLoading && selectedWorkflow.action === "attention")}
        onclick={runSelectedWorkflowPrimary}
      >
        {researchKickoffBusy && selectedWorkflow.action === "research"
          ? "Starting..."
          : reviewPacketLoading && selectedWorkflow.action === "attention"
            ? "Opening..."
            : selectedWorkflow.primary}
      </button>
    </div>
    {#if researchKickoffStatus}
      <p class="workflow-status">{researchKickoffStatus}</p>
    {/if}

    <div class="workflow-rail" aria-label="Selected symbol workflow">
      <button type="button" class="workflow-step" onclick={() => runWorkflowAction("overview")}>
        <span>Status</span>
        <strong>{selectedWorkflow.status}</strong>
      </button>
      <button type="button" class="workflow-step" onclick={() => runWorkflowAction("attention")}>
        <span>Attention</span>
        <strong>{selectedWorkflow.attention}</strong>
      </button>
      <button type="button" class="workflow-step" onclick={() => runWorkflowAction("evidence")}>
        <span>Evidence</span>
        <strong>{selectedWorkflow.evidence}</strong>
      </button>
      <button type="button" class="workflow-step" onclick={() => runWorkflowAction("thesis")}>
        <span>Thesis</span>
        <strong>{selectedWorkflow.thesis}</strong>
      </button>
      <button type="button" class="workflow-step" onclick={() => runWorkflowAction("tracking")}>
        <span>Decision</span>
        <strong>{selectedWorkflow.decision}</strong>
      </button>
    </div>

    {#if (selectedWorkflow.attentionItems ?? []).length}
      <div class="workflow-attention" data-testid="workflow-attention">
        {#each (selectedWorkflow.attentionItems ?? []).slice(0, 3) as item (item.id)}
          <button type="button" onclick={() => openReviewPacketById(item.id, selectedSymbol)}>
            <span>{attentionStateLabel(item.kind)}</span>
            <strong>{item.title}</strong>
            {#if item.reason}
              <small>{item.reason}</small>
            {/if}
          </button>
        {/each}
      </div>
    {/if}

    {#if selectedCandidateReview}
      {@const availableData = candidateAvailableData(selectedCandidateReview)}
      {@const firstPendingCandidate = selectedPendingCandidates[0]}
      <section class="promotion-review" data-testid="promotion-review">
        <div class="promotion-head">
          <div>
            <span class="workflow-kicker">research review</span>
            <strong>Start research for {selectedSymbol}</strong>
          </div>
          <span class="badge tiny state-{selectedCandidateReview.fsm_state ?? 'ready_for_review'}">
            {attentionStateLabel(selectedCandidateReview.fsm_state ?? "ready_for_review")}
          </span>
          <span class="muted">{shortTs(selectedCandidateReview.created_at)}</span>
        </div>
        <div class="promotion-grid">
          <div>
            <span class="promotion-label">What happened</span>
            <p>Discovery nominated {selectedSymbol} for operator review.</p>
          </div>
          <div>
            <span class="promotion-label">Why queued</span>
            <p>{candidateNominationReason(selectedCandidateReview)}</p>
            {#if firstPendingCandidate?.rank_reasons?.length}
              <p class="muted">{firstPendingCandidate.rank_bucket ?? "ranked"} {Math.round(firstPendingCandidate.rank_score ?? 0)} · {firstPendingCandidate.rank_reasons.slice(0, 3).join(" · ")}</p>
            {/if}
          </div>
          <div>
            <span class="promotion-label">Evidence attached</span>
            {#if availableData.length > 0}
              <div class="promotion-tokens">
                {#each availableData as item}
                  <span class="brain-token">{item}</span>
                {/each}
              </div>
            {:else if selectedPromotionRawSignals.length > 0}
              <p>{selectedPromotionRawSignals.join(", ")}</p>
            {:else}
              <p class="muted">No source checklist was attached to this nomination yet.</p>
            {/if}
          </div>
          <div>
            <span class="promotion-label">What starting does</span>
            <p>Records the candidate as confirmed, resolves the attention item, publishes discovery.confirmed, and starts research, context, and thesis work.</p>
          </div>
        </div>
        <div class="promotion-destinations">
          <span class="promotion-label">Destination</span>
          <span class="badge tiny">Universe always included</span>
          {#if selectedPromotionLists.length > 0}
            {#each selectedPromotionLists as proposed (proposed.watchlist_id)}
              <label class="att-pick promotion-pick">
                <input
                  type="checkbox"
                  checked={selectedPromotionListChecked(proposed.watchlist_id)}
                  onchange={(event) => setSelectedPromotionList(proposed.watchlist_id, (event.currentTarget as HTMLInputElement).checked)}
                />
                {proposed.watchlist_name}
                <span class="badge tiny conf-{proposed.confidence}">{proposed.confidence}</span>
                <span class="muted">{proposed.rationale}</span>
              </label>
            {/each}
          {:else}
            <span class="muted">No watchlist match attached; start as Universe-only.</span>
          {/if}
        </div>
        <div class="promotion-actions">
          <button
            class="confirm"
            disabled={promotionBusy || selectedCandidateIds.length === 0}
            onclick={() => confirmGroup(selectedCandidateIds)}
          >Start research</button>
          <button
            class="reject"
            disabled={promotionBusy || selectedCandidateIds.length === 0}
            onclick={() => rejectGroup(selectedCandidateIds, "not_my_edge")}
          >Reject nomination</button>
          <button type="button" class="text-action" onclick={() => {
            bottomMode = "attention";
            if (!bottomOpen) bottomPane?.expand();
          }}>open queue</button>
          {#if promotionStatus}
            <span class="muted">{promotionStatus}</span>
          {/if}
        </div>
      </section>
    {/if}

    {#if reviewPacket || reviewPacketLoading || reviewPacketError}
      <ReviewPacket
        packet={reviewPacket}
        loading={reviewPacketLoading}
        error={reviewPacketError}
        busy={promotionBusy}
        status={promotionStatus}
        onAction={handleReviewPacketAction}
      />
      {#if reviewPacketDecisionOpen && reviewPacket}
        <section class="review-decision-panel" data-testid="review-packet-decision-form">
          <div class="review-decision-head">
            <span class="workflow-kicker">decision</span>
            <strong>{reviewPacket.attention.symbol ?? selectedSymbol} · reviewed thesis</strong>
          </div>
          {@render decisionForm()}
        </section>
      {/if}
    {/if}
  </section>

  <!-- Body: left column (chart + bottom drawer stacked) + vertical splitter + right panel (full height) -->
  <PaneGroup direction="horizontal" autoSaveId="ws.v3.outer" class="body">
    <Pane defaultSize={72} minSize={40}>
      <PaneGroup direction="vertical" autoSaveId="ws.v3.left" class="main-col">
        <Pane defaultSize={70} minSize={30}>
          <ChartPanel symbol={selectedSymbol} liveEvents={live} streamConnected={connected} onStateChange={updateChartState} />
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
      {#each ["brain", "attention", "events", "discovery", "automation", "decisions", "calibration", "diagnostics"] as BottomMode[] as m}
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
                {@const macroMetrics = macroMetricChips(macro)}
                {@const dislocationGroups = dislocationBuckets(macro)}
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
                  {#if macroMetrics.length}
                    <div class="macro-metrics">
                      {#each macroMetrics as metric (metric.label)}
                        <span class="brain-token macro-metric">
                          <strong>{metric.label}</strong> {metric.value}
                          {#if metric.detail}<small>{metric.detail}</small>{/if}
                        </span>
                      {/each}
                    </div>
                  {/if}
                  {#if dislocationGroups.length}
                    <div class="dislocation-map">
                      <strong>Dislocation Map</strong>
                      {#each dislocationGroups as bucket (bucket.key)}
                        <div class="dislocation-bucket bucket-{bucket.key}">
                          <span class="muted">{bucket.label}</span>
                          {#each bucket.items.slice(0, 3) as item}
                            <span class="brain-token" title={item.interpretation ?? ""}>
                              {item.name}
                              {#if item.score !== null && item.score !== undefined}
                                <small>{Math.round(Number(item.score))}</small>
                              {/if}
                              {#if dislocationReason(item)}
                                <small>{dislocationReason(item)}</small>
                              {/if}
                            </span>
                          {/each}
                        </div>
                      {/each}
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
                          {@const liveConviction = brainTickerConvictionLabel(t)}
                          <button
                            type="button"
                            class:stale-link={Boolean(t.link_stale)}
                            class="brain-ticker"
                            title={brainTickerTitle(t)}
                            onclick={() => selectSymbol(t.symbol)}
                          >
                            <strong>{t.symbol}</strong>
                            <span class="brain-ticker-role">{t.role.replace(/_/g, " ")}</span>
                            <span class="brain-ticker-badges">
                              {#if t.thesis_state}
                                <span class="wl-thesis-state">{thesisStatusLabel(t.thesis_state)}</span>
                              {/if}
                              <span class={`badge tiny ${thesisDirectionClass(t.thesis_direction)}`}>
                                {thesisDirectionLabel(t.thesis_direction)}
                              </span>
                              {#if liveConviction}
                                <span class="badge tiny brain-conviction">{liveConviction}</span>
                              {/if}
                              {#if t.link_stale}
                                <span class="badge tiny warning">link stale</span>
                              {/if}
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
              {#each ["all", "candidate_review", "thesis_review", "thesis_actionable", "risk_review"] as f}
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
                      <button
                        type="button"
                        class="att-symbol link-symbol"
                        onclick={() => g.symbol && selectSymbol(g.symbol)}
                      >{g.symbol}</button>
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
                    {:else if g.kind === "thesis_review"}
                      thesis changed
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
                    <button class="confirm" onclick={() => openReviewPacketFor(g.items[0])}>Review packet</button>
                    {#if g.kind === "candidate_review"}
                      <button class="confirm" disabled={promotionBusy} onclick={() => confirmGroup(g.candidateIds)}>Start research</button>
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
                    {:else if g.kind === "thesis_review"}
                      <button class="confirm" onclick={() => {
                        if (g.symbol) {
                          selectSymbol(g.symbol);
                          rightTab = "theses";
                        }
                      }}>Review</button>
                      <button class="reject" onclick={() => g.items.forEach((it) => deferOne(it.id))}>Defer 7d</button>
                      <button class="reject" onclick={() => g.items.forEach((it) => dismissOne(it.id))}>Dismiss</button>
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
                <li class:linkable={!!p.symbol}>
                  {#if p.symbol}
                    <button type="button" class="event-link" onclick={() => selectSymbol(p.symbol as string)}>
                      <span class="kind" style="color:{kindColor(e.kind, p)}">{e.kind}</span>
                      <code>{e.subject}</code>
                      <strong>{p.symbol as string}</strong>
                      {#if e.kind === "risk" && p.veto}<span class="badge danger tiny">VETO {(p.reasons as string[])?.join(", ")}</span>{/if}
                    </button>
                  {:else}
                    <span class="kind" style="color:{kindColor(e.kind, p)}">{e.kind}</span>
                    <code>{e.subject}</code>
                    {#if e.kind === "risk" && p.veto}<span class="badge danger tiny">VETO {(p.reasons as string[])?.join(", ")}</span>{/if}
                  {/if}
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
                    <button type="button" class="link-symbol" onclick={() => selectSymbol(c.symbol)}>{c.symbol}</button>
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
                  {#if c.parent_themes?.length}
                    <p class="muted disc-rank">
                      parent themes: {c.parent_themes
                        .slice(0, 3)
                        .map((t) => `${t.name} (${t.role})`)
                        .join(" · ")}
                    </p>
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
                    <button onclick={() => confirmOne(c.id)}>Start research</button>
                    <button class="reject" onclick={() => rejectOne(c.id)}>Reject</button>
                  </div>
                </li>
              {/each}
            </ul>
          {/if}
        {:else if bottomMode === "automation"}
          <div class="automation-preview-actions">
            <button type="button" onclick={() => openAutomationPage(selectedSymbol)}>
              Open full cockpit
            </button>
          </div>
          <AutomationPanel symbol={selectedSymbol} />
        {:else if bottomMode === "decisions"}
          {@render decisionForm()}
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
            {#if calibration.parent_themes?.length}
              <section class="calibration-themes">
                <h4>Parent Theme Calibration</h4>
                <ul>
                  {#each calibration.parent_themes as theme (`${theme.key}:${theme.role}`)}
                    <li>
                      <div>
                        <strong>{theme.name}</strong>
                        <span class="badge tiny">{theme.scope.replace(/_/g, " ")}</span>
                        <span class="badge tiny">{theme.role.replace(/_/g, " ")}</span>
                      </div>
                      <span>
                        {theme.outcomes_scored}/{theme.predictions_total}
                        {#if theme.mean_brier !== null}
                          · brier {theme.mean_brier.toFixed(3)}
                        {/if}
                        {#if theme.mean_lead_time_days !== null}
                          · lead {theme.mean_lead_time_days.toFixed(1)}d
                        {/if}
                      </span>
                    </li>
                  {/each}
                </ul>
              </section>
            {/if}
            {#if calibration.technical_timing?.length}
              <section class="calibration-themes">
                <h4>Technical Timing Calibration</h4>
                <ul>
                  {#each calibration.technical_timing as row (`${row.technical_state}:${row.setup_kind}:${row.benchmark_symbol}`)}
                    <li>
                      <div>
                        <strong>{row.setup_kind.replace(/_/g, " ")}</strong>
                        <span class="badge tiny">{row.entry_stance.replace(/_/g, " ")}</span>
                        <span class="badge tiny">vs {row.benchmark_symbol}</span>
                      </div>
                      <span>
                        {row.outcomes_scored}/{row.observations_total}
                        {#if row.positive_return_rate != null}
                          · win {pct01(row.positive_return_rate)}
                        {/if}
                        {#if row.outperform_rate != null}
                          · outperform {pct01(row.outperform_rate)}
                        {/if}
                        {#if row.mean_forward_return_pct != null}
                          · ret {pctCompact(row.mean_forward_return_pct)}
                        {/if}
                        {#if row.mean_max_drawdown_pct != null}
                          · dd {pctCompact(row.mean_max_drawdown_pct)}
                        {/if}
                        {#if row.mean_excess_return_pct != null}
                          · excess {pctCompact(row.mean_excess_return_pct)}
                        {/if}
                      </span>
                    </li>
                  {/each}
                </ul>
              </section>
            {/if}
          {:else}
            <p class="muted">No calibration data yet.</p>
          {/if}
        {:else if bottomMode === "diagnostics"}
          {#if sysStatus}
            {@const ing = (sysStatus.ingest ?? {}) as Record<string, { last_at: string|null; count_24h: number; symbols_24h?: number }>}
            {@const disc = sysStatus.discovery as { last_pass_at: string|null; open_candidates: number; by_signal: { signal: string; count: number }[]; pool_size: number }}
            {@const cog = sysStatus.cognition as { contexts_24h: number; contexts_total_symbols: number; thesis_by_state: { state: string; count: number }[]; runs_24h?: number; runs_by_status?: { status: string; count: number }[]; latest_runs?: CognitionRun[] }}
            {@const ev = sysStatus.evidence as { open_requirements: number; source_tasks_due?: number; source_tasks_stale_fetching?: number; by_state: { state: string; count: number }[]; by_reason?: { reason: string; count: number }[]; source_tasks_by_state?: { state: string; count: number }[]; source_tasks_by_action?: { provider: string; action: string; state: string; count: number; due_count?: number; stale_fetching_count?: number; next_due_at?: string|null; last_updated_at?: string|null; sample_targets?: string[] }[] }}
            {@const der = (sysStatus.derived_refresh ?? {}) as { due_count?: number; queued_count?: number; scheduled_count?: number; next_due_at?: string|null; stale_running?: number; by_state?: { state: string; count: number }[]; by_target?: { target_kind: string; state: string; count: number }[]; recent?: { id: number; target_kind: string; target_id: string; symbol?: string|null; reason: string; dependency_kind: string; priority: string; state: string; attempts: number; due_at: string; updated_at: string; last_error?: string|null }[] }}
            {@const att = sysStatus.attention as { open_items: number; deferred_items?: number; by_kind: { kind: string; count: number }[]; by_state?: { state: string; count: number }[]; by_owner?: { owner: string; count: number }[] }}
            {@const llm = sysStatus.llm as { calls_24h: number; avg_latency_ms: number|null; by_prompt: { prompt: string; count: number; avg_ms: number|null; last_at: string|null }[] }}
            {@const health = (sysStatus.source_health ?? []) as { source: string; last_status: string; effective_status?: string; stale_running?: boolean; running_age_minutes?: number|null; last_started_at: string|null; last_success_at: string|null; last_failure_at: string|null; last_failure_kind?: string|null; last_error?: string|null; retry_after_at?: string|null; rows_seen: number; rows_inserted: number; symbols_attempted: number; symbols_failed: number }[]}
            {@const priceFresh = sysStatus.price_freshness as { expected_latest_session?: string|null; actual_latest_session?: string|null; symbols_total?: number; symbols_fresh?: number; status?: string }}
            {@const db = (sysStatus.database ?? {}) as { status?: string|null; reachable?: boolean; database?: string|null; checked_at?: string|null; latency_ms?: number|null; reason?: string|null }}
            <div class="diag-grid">
              <section class={`diag ${db.reachable === false ? "diag-alert" : ""}`}>
                <h5>Database <span class={`badge tiny db-${db.status ?? "unknown"}`}>{databaseStatusLabel(db.status)}</span></h5>
                <dl class="meta-list inline">
                  <dt>name</dt><dd>{db.database ?? "—"}</dd>
                  <dt>checked</dt><dd>{db.checked_at ? relativeTime(db.checked_at) : "—"}</dd>
                  <dt>latency</dt><dd>{db.latency_ms ?? "—"}ms</dd>
                </dl>
                {#if db.reason}
                  <p class="diag-reason">{db.reason}</p>
                {/if}
              </section>

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
                  <thead><tr><th>source</th><th>status</th><th>started</th><th>last result</th><th>checked rows</th><th>new rows</th><th>symbols</th><th>retry</th></tr></thead>
                  <tbody>
                    {#each health as h (h.source)}
                      {@const effectiveStatus = h.effective_status ?? h.last_status}
                      <tr title={h.last_error ?? ""}>
                        <td><strong>{h.source}</strong></td>
                        <td><span class={`badge tiny health-${effectiveStatus}`}>{healthLabel(effectiveStatus, h.last_failure_kind)}</span></td>
                        <td class="muted">{h.last_started_at ? relativeTime(h.last_started_at) : "—"}</td>
                        <td class="muted">{h.last_success_at ? relativeTime(h.last_success_at) : "—"}</td>
                        <td>{effectiveStatus === "running" && !h.last_success_at ? "checking" : h.rows_seen}</td>
                        <td>{effectiveStatus === "running" && !h.last_success_at ? "—" : h.rows_inserted}</td>
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
                  <dt>runs (24h)</dt><dd>{cog.runs_24h ?? 0}</dd>
                </dl>
                {#if cog.thesis_by_state?.length}
                  <ul class="chips">
                    {#each cog.thesis_by_state as s (s.state)}
                      <li class="chip">{s.state}: <strong>{s.count}</strong></li>
                    {/each}
                  </ul>
                {/if}
                {#if cog.runs_by_status?.length}
                  <ul class="chips">
                    {#each cog.runs_by_status as s (s.status)}
                      <li class="chip">{cognitionRunLabel(s.status)}: <strong>{s.count}</strong></li>
                    {/each}
                  </ul>
                {/if}
                {#if cog.latest_runs?.length}
                  <table class="diag-tbl compact-run-table">
                    <thead><tr><th>symbol</th><th>status</th><th>why</th><th>when</th></tr></thead>
                    <tbody>
                      {#each cog.latest_runs as run (run.id)}
                        <tr title={run.error ?? run.reason ?? ""}>
                          <td><strong>{run.symbol}</strong></td>
                          <td><span class={`badge tiny cognition-${run.status}`}>{cognitionRunLabel(run.status)}</span></td>
                          <td class="muted">{cognitionRunDriver(run)}</td>
                          <td class="muted">{relativeTime(run.started_at)}</td>
                        </tr>
                      {/each}
                    </tbody>
                  </table>
                {/if}
              </section>

              <section class="diag">
                <h5>Evidence</h5>
                <dl class="meta-list inline">
                  <dt>open requirements</dt><dd>{ev.open_requirements}</dd>
                  <dt>source tasks due</dt><dd>{ev.source_tasks_due ?? 0}</dd>
                  <dt>stale fetching</dt><dd>{ev.source_tasks_stale_fetching ?? 0}</dd>
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
                {#if ev.source_tasks_by_state?.length}
                  <ul class="chips">
                    {#each ev.source_tasks_by_state as s (s.state)}
                      <li class="chip">source {s.state}: <strong>{s.count}</strong></li>
                    {/each}
                  </ul>
                {/if}
                {#if ev.source_tasks_by_action?.length}
                  <table class="diag-tbl compact-run-table">
                    <thead><tr><th>provider/action</th><th>state</th><th>count</th><th>due</th><th>next</th><th>examples</th></tr></thead>
                    <tbody>
                      {#each ev.source_tasks_by_action.slice(0, 12) as task (`${task.provider}:${task.action}:${task.state}`)}
                        <tr title={task.last_updated_at ? `updated ${relativeTime(task.last_updated_at)}` : ""}>
                          <td><strong>{task.provider}</strong><br><span class="muted">{task.action.replace(/_/g, " ")}</span></td>
                          <td><span class={`badge tiny task-${task.state}`}>{task.state.replace(/_/g, " ")}</span></td>
                          <td>{task.count}</td>
                          <td>{task.due_count ?? 0}{#if task.stale_fetching_count} / {task.stale_fetching_count} stale{/if}</td>
                          <td class="muted">{task.next_due_at ? relativeTime(task.next_due_at) : "—"}</td>
                          <td class="muted">{task.sample_targets?.slice(0, 4).join(", ") || "—"}</td>
                        </tr>
                      {/each}
                    </tbody>
                  </table>
                {/if}
              </section>

              <section class="diag">
                <h5>Derived refresh</h5>
                <dl class="meta-list inline">
                  <dt>due now</dt><dd>{der.due_count ?? 0}</dd>
                  <dt>scheduled</dt><dd>{der.scheduled_count ?? 0}</dd>
                  <dt>queued</dt><dd>{der.queued_count ?? 0}</dd>
                  <dt>stale running</dt><dd>{der.stale_running ?? 0}</dd>
                  {#if der.next_due_at}
                    <dt>next</dt><dd>{relativeTime(der.next_due_at)}</dd>
                  {/if}
                </dl>
                {#if der.by_state?.length}
                  <ul class="chips">
                    {#each der.by_state as s (s.state)}
                      <li class="chip">derived {s.state}: <strong>{s.count}</strong></li>
                    {/each}
                  </ul>
                {/if}
                {#if der.by_target?.length}
                  <ul class="chips">
                    {#each der.by_target as t (`${t.target_kind}:${t.state}`)}
                      <li class="chip">{t.target_kind.replace(/_/g, " ")} {t.state}: <strong>{t.count}</strong></li>
                    {/each}
                  </ul>
                {/if}
                {#if der.recent?.length}
                  <table class="diag-tbl compact-run-table">
                    <thead><tr><th>target</th><th>state</th><th>why</th><th>due</th></tr></thead>
                    <tbody>
                      {#each der.recent.slice(0, 8) as task (task.id)}
                        <tr title={task.last_error ?? `${task.dependency_kind} changed`}>
                          <td><strong>{task.target_kind.replace(/_/g, " ")}</strong><br><span class="muted">{task.symbol ?? task.target_id}</span></td>
                          <td><span class={`badge tiny task-${task.state}`}>{task.state.replace(/_/g, " ")}</span></td>
                          <td class="muted">{task.reason.replace(/_/g, " ")}</td>
                          <td class="muted">{task.due_at ? relativeTime(task.due_at) : "—"}</td>
                        </tr>
                      {/each}
                    </tbody>
                  </table>
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
        <div class="wl-filters">
          <select bind:value={watchlistStatusFilter} aria-label="Thesis status filter">
            <option value="all">all statuses</option>
            <option value="forming">forming</option>
            <option value="building_conviction">building conviction</option>
            <option value="armed">armed</option>
            <option value="actionable">actionable</option>
            <option value="position_open">position open</option>
            <option value="none">no thesis</option>
          </select>
          <select bind:value={watchlistDirectionFilter} aria-label="Thesis direction filter">
            <option value="all">all directions</option>
            <option value="up">bull</option>
            <option value="down">bear</option>
            <option value="neutral">neutral</option>
            <option value="none">none</option>
          </select>
          <select bind:value={watchlistTechnicalFilter} aria-label="Technical filter">
            <option value="all">all technicals</option>
            <option value="extended">extended</option>
            <option value="constructive">constructive</option>
            <option value="base_building">base building</option>
            <option value="deteriorating">deteriorating</option>
            <option value="unknown">unknown</option>
          </select>
          <select bind:value={watchlistFreshnessFilter} aria-label="Freshness filter">
            <option value="all">all freshness</option>
            <option value="fresh">fresh</option>
            <option value="stale_missing">stale/missing</option>
            <option value="blocked">blocked</option>
          </select>
          <select bind:value={watchlistAttentionFilter} aria-label="Attention filter">
            <option value="all">all attention</option>
            <option value="open">open attention</option>
            <option value="owner:operator">owner operator</option>
            <option value="owner:source">owner source</option>
            <option value="owner:cognition">owner cognition</option>
            <option value="state:ready_for_review">ready review</option>
            <option value="state:waiting_on_data">waiting data</option>
            <option value="state:actionable">actionable</option>
            <option value="state:blocked">blocked</option>
          </select>
          <select bind:value={watchlistThemeFilter} aria-label="Parent brain theme filter">
            <option value="all">all themes</option>
            {#each watchlistThemeOptions as theme (theme.key)}
              <option value={theme.key}>{themeShortName(theme)}</option>
            {/each}
          </select>
          <button
            type="button"
            class="wl-reset"
            class:active={watchlistFiltersActive()}
            disabled={!watchlistFiltersActive()}
            onclick={resetWatchlistFilters}
            title="clear watchlist filters"
          >reset</button>
        </div>
        <ul class="wl-list">
          {#each allWatchlists as w (w.id)}
            {@const open = expandedListIds[w.id] ?? false}
            {@const rawMembers = membersFor(w.id)}
            {@const members = filteredMembersFor(w.id)}
            <li class="wl-item">
              <button type="button" class="wl-row" onclick={() => toggleListExpanded(w.id)}>
                <span class="caret">{open ? "▾" : "▸"}</span>
                <span class="wl-name" style={w.color ? `border-left: 3px solid ${w.color}; padding-left: .35rem` : ""}>{w.name}</span>
                <span class="muted">{members.length === rawMembers.length ? w.member_count : `${members.length}/${rawMembers.length}`}</span>
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
                    {@const themes = themesForMember(m)}
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
                        <span class={`badge tiny ${technicalStateClass(m.technical_state)}`}>
                          {technicalStateLabel(m.technical_state)}
                        </span>
                        <span class={`badge tiny ${entryStanceClass(m.entry_stance)}`}>
                          {entryStanceLabel(m.entry_stance)}
                        </span>
                        <span class={`badge tiny ${freshnessClass(m.freshness_status)}`} title={freshnessTitle(m)}>
                          {freshnessLabel(m.freshness_status)}
                        </span>
                        {#if (m.open_attention ?? 0) > 0}
                          <span class="badge tiny att-open" title={attentionLabel(m)}>
                            {m.open_attention}
                          </span>
                        {/if}
                        {#if themes.length > 0}
                          <span class="badge tiny theme" title={themes.map(themeShortName).join(" · ")}>
                            {themeShortName(themes[0])}
                          </span>
                        {/if}
                        {#if pctCompact(m.technical_pct_vs_200d)}
                          <span class="muted wl-distance">{pctCompact(m.technical_pct_vs_200d)} 200D</span>
                        {/if}
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
                    <li class="muted wl-empty">{rawMembers.length === 0 ? "empty" : "no matches"}</li>
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
            {#each RIGHT_TABS as t}
              <button class:active={rightTab === t} onclick={() => (rightTab = t)}>{t}</button>
            {/each}
          </nav>
          <div class="tab-body">
            {#if rightTab === "overview"}
              {#if selectedCandidateReview}
                {@const availableData = candidateAvailableData(selectedCandidateReview)}
                <section class="side-review-card" data-testid="side-candidate-review">
                  <div class="brain-hdr">
                    <span class="brain-title">Candidate Review</span>
                    <span class="badge tiny state-{selectedCandidateReview.fsm_state ?? 'ready_for_review'}">
                      {attentionStateLabel(selectedCandidateReview.fsm_state ?? "ready_for_review")}
                    </span>
                  </div>
                  <h4>Start research for {selectedSymbol}</h4>
                  <p>{candidateNominationReason(selectedCandidateReview)}</p>
                  {#if availableData.length > 0}
                    <div class="promotion-tokens">
                      {#each availableData as item}
                        <span class="brain-token">{item}</span>
                      {/each}
                    </div>
                  {/if}
                  <p class="muted">Starting research records the candidate, resolves attention, publishes discovery.confirmed, and starts evidence, context, and thesis work.</p>
                  {#if selectedPromotionLists.length > 0}
                    <div class="promotion-destinations">
                      <span class="badge tiny">Universe always included</span>
                      {#each selectedPromotionLists as proposed (proposed.watchlist_id)}
                        <label class="att-pick promotion-pick">
                          <input
                            type="checkbox"
                            checked={selectedPromotionListChecked(proposed.watchlist_id)}
                            onchange={(event) => setSelectedPromotionList(proposed.watchlist_id, (event.currentTarget as HTMLInputElement).checked)}
                          />
                          {proposed.watchlist_name}
                          <span class="badge tiny conf-{proposed.confidence}">{proposed.confidence}</span>
                        </label>
                      {/each}
                    </div>
                  {/if}
                  <div class="promotion-actions">
                    <button
                      class="confirm"
                      disabled={promotionBusy || selectedCandidateIds.length === 0}
                      onclick={() => confirmGroup(selectedCandidateIds)}
                    >Start research</button>
                    <button
                      class="reject"
                      disabled={promotionBusy || selectedCandidateIds.length === 0}
                      onclick={() => rejectGroup(selectedCandidateIds, "not_my_edge")}
                    >Reject nomination</button>
                  </div>
                </section>
              {:else if !selectedTicker && selectedPoolMember}
                <section class="side-review-card" data-testid="pool-candidate-review">
                  <div class="brain-hdr">
                    <span class="brain-title">Pool Candidate</span>
                    <span class="badge tiny brain-not_monitored">not monitored</span>
                  </div>
                  <h4>Review {selectedPoolMember.symbol}</h4>
                  <dl class="meta-list inline">
                    {#if selectedPoolMember.company_name}<dt>company</dt><dd>{selectedPoolMember.company_name}</dd>{/if}
                    {#if selectedPoolMember.sector || selectedPoolMember.industry}<dt>group</dt><dd>{[selectedPoolMember.sector, selectedPoolMember.industry].filter(Boolean).join(" / ")}</dd>{/if}
                    {#if selectedPoolMember.market_cap}<dt>market cap</dt><dd>{formatCompact(selectedPoolMember.market_cap)}</dd>{/if}
                    {#if selectedPoolMember.technical_state}<dt>technical</dt><dd>{selectedPoolMember.technical_state.replace(/_/g, " ")}</dd>{/if}
                  </dl>
                  <p>
                    This symbol is in the Discovery pool, not the active Universe.
                    The brain will not synthesize context or draft a thesis until you promote it.
                  </p>
                  <div class="promotion-destinations">
                    <span class="badge tiny">Universe always included</span>
                    {#if watchlists.length > 0}
                      {#each watchlists as w (w.id)}
                        <label class="att-pick promotion-pick">
                          <input
                            type="checkbox"
                            checked={poolPromotionLists[w.id] ?? false}
                            onchange={(event) => setPoolPromotionList(w.id, (event.currentTarget as HTMLInputElement).checked)}
                          />
                          {w.name}
                        </label>
                      {/each}
                    {:else}
                      <span class="muted">No watchlists yet; start as Universe-only.</span>
                    {/if}
                  </div>
                  <div class="promotion-actions">
                    <button
                      class="confirm"
                      disabled={poolPromotionBusy}
                      onclick={promoteSelectedPoolCandidate}
                    >Start research</button>
                    <button type="button" class="text-action" onclick={() => (rightTab = "context")}>open context</button>
                    {#if poolPromotionStatus}
                      <span class="muted">{poolPromotionStatus}</span>
                    {/if}
                  </div>
                </section>
              {/if}
              <section class="brain-card symbol-status-card placement-{selectedPlacement.tone}" data-testid="symbol-status-card">
                <div class="brain-hdr">
                  <span class="brain-title">System Placement</span>
                  <span class="badge tiny placement-{selectedPlacement.tone}">{selectedPlacement.label}</span>
                  {#if selectedTicker}<span class="muted">T{selectedTicker.tier}</span>{/if}
                </div>
                <p>{selectedPlacement.detail}</p>
                <dl class="meta-list inline">
                  <dt>location</dt><dd>{selectedPlacement.location}</dd>
                  <dt>watchlists</dt>
                  <dd>
                    {#if selectedWatchlistPlacements.length > 0}
                      <span class="placement-tags">
                        {#each selectedWatchlistPlacements as w (w.id)}
                          <span class="brain-token">{w.name}</span>
                        {/each}
                      </span>
                    {:else}
                      <span class="muted">none</span>
                    {/if}
                  </dd>
                  <dt>thesis</dt><dd>{workflowThesisText()}</dd>
                  <dt>attention</dt><dd>{workflowAttentionText()}</dd>
                </dl>
                {#if selectedSymbol && !selectedTicker}
                  <div class="placement-promote" data-testid="placement-promote">
                    <div class="promotion-destinations compact">
                      <span class="badge tiny">Universe always included</span>
                      {#if selectedCandidateIds.length > 0 && selectedPromotionLists.length > 0}
                        {#each selectedPromotionLists as proposed (proposed.watchlist_id)}
                          <label class="att-pick promotion-pick">
                            <input
                              type="checkbox"
                              checked={selectedPromotionListChecked(proposed.watchlist_id)}
                              onchange={(event) => setSelectedPromotionList(proposed.watchlist_id, (event.currentTarget as HTMLInputElement).checked)}
                            />
                            {proposed.watchlist_name}
                            <span class="badge tiny conf-{proposed.confidence}">{proposed.confidence}</span>
                          </label>
                        {/each}
                      {:else if selectedCandidateIds.length > 0}
                        <span class="muted">No watchlist match attached; promote as Universe-only.</span>
                      {:else if watchlists.length > 0}
                        {#each watchlists as w (w.id)}
                          <label class="att-pick promotion-pick">
                            <input
                              type="checkbox"
                              checked={poolPromotionLists[w.id] ?? false}
                              onchange={(event) => setPoolPromotionList(w.id, (event.currentTarget as HTMLInputElement).checked)}
                            />
                            {w.name}
                          </label>
                        {/each}
                      {:else}
                        <span class="muted">No watchlists yet; start as Universe-only.</span>
                      {/if}
                    </div>
                    <div class="promotion-actions">
                      <button
                        class="confirm"
                        disabled={promotionBusy || poolPromotionBusy}
                        onclick={promoteSelectedToUniverse}
                      >Start research</button>
                      {#if promotionStatus || poolPromotionStatus}
                        <span class="muted">{promotionStatus ?? poolPromotionStatus}</span>
                      {/if}
                    </div>
                  </div>
                {/if}
              </section>
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
                <p class="muted">Symbol metadata not loaded yet.</p>
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
                  {#if symbolBrain.cognition?.last_run}
                    {@const run = symbolBrain.cognition.last_run}
                    <div class="brain-run" title={run.error ?? run.reason ?? ""}>
                      <div class="brain-source-main">
                        <strong>Last cognition run</strong>
                        <span class={`badge tiny cognition-${run.status}`}>{cognitionRunLabel(run.status)}</span>
                        <span class="muted">{cognitionRunTime(run)}</span>
                      </div>
                      <div class="brain-source-detail">
                        {cognitionRunReason(run)}
                      </div>
                    </div>
                  {/if}
                  <ul class="brain-sources">
                    {#each symbolBrain.sources as s (s.source)}
                      <li title={s.last_error ?? ""}>
                        <div class="brain-source-main">
                          <strong>{sourceLabel(s.source)}</strong>
                          <span class="badge tiny brain-source-{s.status}">
                            {healthLabel(s.status, s.failure_kind)}
                          </span>
                          <span class="muted">{sourceTime(s)}</span>
                        </div>
                        {#if sourceDetail(s)}
                          <div class="brain-source-detail">{sourceDetail(s)}</div>
                        {/if}
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
              <section class="brain-card technical-overview">
                <div class="brain-hdr">
                  <span class="brain-title">Technical</span>
                  {#if symbolTechnical === undefined}
                    <span class="muted">loading</span>
                  {:else if symbolTechnical}
                    <span class="badge tiny tech-{symbolTechnical.state}">
                      {symbolTechnical.state.replace(/_/g, " ")}
                    </span>
                    {#if symbolTechnical.daily}
                      {@const sma200 = symbolTechnical.daily.sma.find((s) => s.window === 200)}
                      {#if sma200?.pct_vs !== null && sma200?.pct_vs !== undefined}
                        <span class="muted">{sma200.pct_vs > 0 ? "+" : ""}{sma200.pct_vs.toFixed(1)}% vs 200D</span>
                      {/if}
                    {/if}
                    <button type="button" class="text-action" onclick={() => (rightTab = "technical")}>open</button>
                  {:else}
                    <span class="muted">unavailable</span>
                  {/if}
                </div>
                {#if symbolTechnical}
                  <p>{symbolTechnical.summary}</p>
                {/if}
              </section>
            {:else if rightTab === "analyst"}
              <AnalystPanel symbol={selectedSymbol} />
            {:else if rightTab === "technical"}
              <TechnicalStatePanel state={symbolTechnical} />
            {:else if rightTab === "context"}
              {#if symbolContext === undefined}
                <p class="muted">Loading…</p>
              {:else}
                <ContextPanel
                  ctx={symbolContext ?? null}
                  symbol={selectedSymbol}
                  autoSynthesize={symbolBrain?.active_ticker ?? false}
                  blockedReason={symbolBrain?.active_ticker ? "" : (symbolBrain?.reason ?? "")}
                  onStarted={async () => {
                    rightTab = "evidence";
                    await reloadSelectedSymbolDetails();
                  }}
                />
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
                      {#if req.source_tasks?.length}
                        <div class="task-chips">
                          {#each req.source_tasks as task (task.id)}
                            <span class={`task-chip tone-${sourceTaskTone(task.state)}`} title={task.last_error ?? sourceTaskLabel(task)}>
                              {sourceTaskLabel(task)}
                            </span>
                          {/each}
                        </div>
                      {/if}
                      {#if req.blocking_state !== "satisfied" && evidenceCounts(req)}
                        <p class="muted">{evidenceCounts(req)}</p>
                      {/if}
                      {#if evidenceHealth(req)}
                        <p class="muted">{evidenceHealth(req)}</p>
                      {/if}
                      {#if req.requirement_key === "product_research" && selectedTicker && req.blocking_state !== "satisfied"}
                        <button type="button" class="inline-action" disabled={researchKickoffBusy} onclick={startResearchForSelected}>
                          {researchKickoffBusy ? "Starting..." : "Start research now"}
                        </button>
                      {/if}
                      {#if req.last_error}<p class="error-text">{req.last_error}</p>{/if}
                    </li>
                  {/each}
                </ul>
              {/if}
              {#if symbolEvidenceItems === undefined}
                <p class="muted">Loading evidence facts…</p>
              {:else if symbolEvidenceItems.length > 0}
                <section class="evidence-items">
                  <h4>Evidence facts</h4>
                  <ul class="evidence-list">
                    {#each symbolEvidenceItems.slice(0, 20) as item (item.id)}
                      <li class="evidence-card evidence-item tone-{evidenceItemTone(item)}">
                        <div class="evidence-row">
                          {#if item.url}
                            <a href={item.url} target="_blank" rel="noreferrer">{item.summary}</a>
                          {:else}
                            <strong>{item.summary}</strong>
                          {/if}
                          <span class="badge tiny">{item.kind.replace(/_/g, " ")}</span>
                        </div>
                        <p class="muted">{evidenceItemMeta(item)}</p>
                      </li>
                    {/each}
                  </ul>
                </section>
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
              {:else}
                <section class="research-sources empty-source">
                  <h4>Research sources</h4>
                  <p class="muted">No accepted research sources yet for <strong>{selectedSymbol}</strong>.</p>
                  {#if productResearchRequirement()?.source_tasks?.length}
                    <div class="task-chips">
                      {#each productResearchRequirement()?.source_tasks ?? [] as task (task.id)}
                        <span class={`task-chip tone-${sourceTaskTone(task.state)}`} title={task.last_error ?? sourceTaskLabel(task)}>
                          {sourceTaskLabel(task)}
                        </span>
                      {/each}
                    </div>
                  {/if}
                  {#if selectedTicker}
                    <button type="button" class="inline-action" disabled={researchKickoffBusy} onclick={startResearchForSelected}>
                      {researchKickoffBusy ? "Starting..." : "Start research now"}
                    </button>
                  {/if}
                </section>
              {/if}
            {:else if rightTab === "theses"}
              <section class="symbol-placement-strip placement-{selectedPlacement.tone}" data-testid="thesis-placement-strip">
                <span class="badge tiny placement-{selectedPlacement.tone}">{selectedPlacement.label}</span>
                <strong>{selectedPlacement.short}</strong>
                <span class="muted">{watchlistSummary(selectedWatchlistPlacements)}</span>
                {#if selectedSymbol && !selectedTicker}
                  <button
                    type="button"
                    class="confirm"
                    disabled={promotionBusy || poolPromotionBusy}
                    onclick={promoteSelectedToUniverse}
                  >Start research</button>
                {/if}
                <button type="button" class="text-action" onclick={() => (rightTab = "overview")}>overview</button>
              </section>
              {#if selectedSymbol && !selectedTicker}
                <section class="nomination-state" data-testid="thesis-promotion-panel">
                  <div class="nomination-hdr">
                    <span class="badge tiny placement-{selectedPlacement.tone}">{selectedPlacement.label}</span>
                    <strong>Not active yet</strong>
                  </div>
                  <p>{selectedPlacement.detail}</p>
                  <p class="muted">Start research to add this symbol to the active Universe before expecting context synthesis, thesis drafting, or thesis updates.</p>
                  <div class="att-actions">
                    <button
                      class="confirm"
                      disabled={promotionBusy || poolPromotionBusy}
                      onclick={promoteSelectedToUniverse}
                    >Start research</button>
                    <button type="button" class="text-action" onclick={() => (rightTab = "overview")}>choose watchlists</button>
                  </div>
                </section>
              {/if}
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
                    <ThesisDetails thesis={currentSymbolThesis} onRecordDecision={openThesisDecision} />
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
                {#if (!symbolTheses || symbolTheses.length === 0) && selectedCandidateReview}
                  {@const availableData = candidateAvailableData(selectedCandidateReview)}
                  <section class="nomination-state">
                    <div class="nomination-hdr">
                      <span class="badge tiny state-{selectedCandidateReview.fsm_state ?? 'ready_for_review'}">nominated</span>
                    <strong>Nominated, not active</strong>
                      <span class="muted">{shortTs(selectedCandidateReview.created_at)}</span>
                    </div>
                    <p>{candidateNominationReason(selectedCandidateReview)}</p>
                    {#if availableData.length > 0}
                      <div class="brain-line">
                        <span class="muted">available</span>
                        {#each availableData as item}
                          <span class="brain-token">{item}</span>
                        {/each}
                      </div>
                    {/if}
                    <p class="muted">Starting research will {candidateAcceptanceText(selectedCandidateReview)}.</p>
                    <div class="att-actions">
                      <button
                        class="confirm"
                        disabled={promotionBusy || selectedCandidateIds.length === 0}
                        onclick={() => confirmGroup(selectedCandidateIds)}
                      >Start research</button>
                      <button
                        class="reject"
                        disabled={promotionBusy || selectedCandidateIds.length === 0}
                        onclick={() => rejectGroup(selectedCandidateIds, "not_now")}
                      >Reject</button>
                    </div>
                  </section>
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
                {#if (!symbolTheses || symbolTheses.length === 0) && (!symbolDeclines || symbolDeclines.length === 0) && !selectedCandidateReview}
                  <p class="muted">
                    No thesis attempts for <strong>{selectedSymbol}</strong> yet.
                    The system should either draft a monitoring thesis or show a
                    declined attempt with a reason.
                  </p>
                {/if}
              {/if}
            {:else if rightTab === "alerts"}
              <PriceAlertsPanel symbol={selectedSymbol} liveEvents={live} />
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
                        {#if d.human_conviction}<span class="badge tiny">human {d.human_conviction}</span>{/if}
                        {#if d.disagreement_reason}
                          <span class="badge tiny reason">{disagreementLabel(d.disagreement_reason)}</span>
                        {/if}
                        <span class="muted">{shortTs(d.at)}</span>
                        {#if d.thesis_id}
                          <button
                            class="link-mini"
                            onclick={() => { decThesisId = d.thesis_id ?? ""; bottomMode = "decisions"; if (!bottomOpen) bottomPane?.expand(); }}
                            title="prefill the decision form with this thesis"
                          >use ↓</button>
                        {/if}
                        {#if d.has_replay}
                          <button
                            class="link-mini"
                            onclick={() => openReplay(d.decision_id)}
                            title="show point-in-time decision replay"
                          >replay</button>
                        {/if}
                        {#if extraSizing}
                          <pre class="dec-sizing">{JSON.stringify(extraSizing)}</pre>
                        {/if}
                        {#if d.disagreement_detail}
                          <p class="decision-detail">{d.disagreement_detail}</p>
                        {/if}
                        {#if d.reason}
                          <p class="decision-detail">reason: {d.reason}</p>
                        {/if}
                      </li>
                    {/each}
                  </ul>
                  {#if replayStatus}
                    <p class="muted hint">{replayStatus}</p>
                  {/if}
                  {#if replay}
                    <section class="decision-replay">
                      <div class="replay-head">
                        <strong>Decision replay</strong>
                        <span class="muted">{replay.symbol}</span>
                        <span class="muted">captured {shortTs(replay.captured_at)}</span>
                        <button class="link-mini" onclick={() => (replay = null)}>close</button>
                      </div>
                      <div class="replay-grid">
                        <div>
                          <span class="muted">thesis</span>
                          <strong>{replayThesisText(replay)}</strong>
                        </div>
                        <div>
                          <span class="muted">context</span>
                          <strong>{replay.context_version ? `v${replay.context_version}` : "missing"}</strong>
                        </div>
                        <div>
                          <span class="muted">consensus</span>
                          <strong>{replay.consensus_score === null || replay.consensus_score === undefined ? "n/a" : replay.consensus_score.toFixed(0)}</strong>
                        </div>
                        <div>
                          <span class="muted">chart</span>
                          <strong>{replay.chart_range_seen ?? "not captured"}</strong>
                        </div>
                      </div>
                      <p class="replay-risk">{replayRiskText(replay)}</p>
                      {#if replaySnapshotString(replay, "disagreement_reason")}
                        <p class="replay-risk">
                          disagreement: {disagreementLabel(replaySnapshotString(replay, "disagreement_reason"))}
                          {#if replaySnapshotString(replay, "disagreement_detail")}
                            · {replaySnapshotString(replay, "disagreement_detail")}
                          {/if}
                        </p>
                      {/if}
                      {#if replaySnapshotString(replay, "human_conviction") || replaySnapshotString(replay, "reason")}
                        <p class="replay-risk">
                          human conviction: {replaySnapshotString(replay, "human_conviction") || "n/a"}
                          {#if replaySnapshotString(replay, "reason")}
                            · {replaySnapshotString(replay, "reason")}
                          {/if}
                        </p>
                      {/if}
                      {#if replay.system_confidence}
                        <span class="badge tiny">system confidence {replay.system_confidence}</span>
                      {/if}
                      {#if replay.evidence_snapshot.length > 0}
                        <ul class="replay-evidence">
                          {#each replay.evidence_snapshot.slice(0, 5) as item (item.id)}
                            <li>
                              <span class="badge tiny">{item.kind.replace(/_/g, " ")}</span>
                              <span>{item.summary}</span>
                            </li>
                          {/each}
                        </ul>
                      {:else}
                        <p class="muted hint">No linked evidence was captured for this decision.</p>
                      {/if}
                    </section>
                  {/if}
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
  {/if}

</div>

<style>
  .workspace {
    /* Locked to viewport edges — no dependency on any parent chain. */
    position: fixed;
    inset: 0;
    display: grid;
    /* Top bar / symbol workflow / body. Error bar overlays via position:absolute. */
    grid-template-rows: 44px auto minmax(0, 1fr);
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
  .top-nav {
    display: flex;
    align-items: center;
    gap: .25rem;
  }
  .top-nav button {
    background: transparent;
    color: #9aa3b8;
    border: 1px solid transparent;
    border-radius: 4px;
    padding: .22rem .5rem;
    font: inherit;
    font-size: .82rem;
    cursor: pointer;
  }
  .top-nav button:hover {
    color: #cdd6f4;
    background: #162033;
  }
  .top-nav button.active {
    color: #cdd6f4;
    background: #0a0d14;
    border-color: #2a3548;
  }
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
  .calibration-themes {
    margin-top: .7rem;
  }
  .calibration-themes h4 {
    margin: 0 0 .45rem;
    font-size: .8rem;
    color: #bac2de;
  }
  .calibration-themes ul {
    margin: 0;
    padding: 0;
    list-style: none;
    display: grid;
    gap: .35rem;
  }
  .calibration-themes li {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: .75rem;
    padding: .35rem .45rem;
    border: 1px solid #1f2733;
    border-radius: 4px;
    background: #0a0d14;
  }
  .calibration-themes li > div {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: .35rem;
  }
  .calibration-themes li > span {
    color: #a6adc8;
    white-space: nowrap;
  }
  .automation-preview-actions {
    display: flex;
    justify-content: flex-end;
    margin-bottom: .45rem;
  }
  .automation-preview-actions button {
    background: #111827;
    color: #cdd6f4;
    border: 1px solid #2a3548;
    border-radius: 4px;
    padding: .24rem .55rem;
    font: inherit;
    font-size: .8rem;
    cursor: pointer;
  }
  .automation-preview-actions button:hover {
    background: #162033;
  }
  .status { margin-left: auto; font-size: .75rem; color: #f38ba8; }
  .status.on { color: #a6e3a1; }

  .error-bar { display: flex; align-items: center; gap: .5rem; }
  .error-bar .x {
    margin-left: auto;
    background: transparent; border: 1px solid currentColor; border-radius: 3px;
    color: inherit; cursor: pointer; padding: 0 .35rem;
  }

  .workflow-strip {
    display: grid;
    grid-template-columns: minmax(280px, .95fr) minmax(420px, 1.45fr);
    gap: .6rem;
    align-items: stretch;
    min-height: 76px;
    padding: .5rem .75rem;
    border-bottom: 1px solid #1f2733;
    background: #0d121b;
    min-width: 0;
  }
  .workflow-main,
  .workflow-rail {
    min-width: 0;
  }
  .workflow-main {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    gap: .65rem;
    align-items: center;
    border-left: 3px solid #45567a;
    background: #0a0d14;
    border-radius: 4px;
    padding: .45rem .55rem;
  }
  .workflow-copy {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: .12rem;
  }
  .workflow-kicker {
    color: #7f8aa3;
    font-size: .68rem;
    text-transform: uppercase;
    letter-spacing: 0;
  }
  .workflow-copy strong,
  .workflow-step strong {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .workflow-copy p {
    margin: 0;
    color: #9aa3b8;
    font-size: .78rem;
    line-height: 1.25;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .workflow-primary {
    background: #1b2230;
    color: #cdd6f4;
    border: 1px solid #45567a;
    border-radius: 4px;
    padding: .32rem .7rem;
    font: inherit;
    font-size: .78rem;
    cursor: pointer;
    white-space: nowrap;
  }
  .workflow-primary:hover {
    background: #263144;
  }
  .workflow-primary:disabled {
    cursor: wait;
    opacity: .65;
  }
  .workflow-status {
    grid-column: 1 / -1;
    margin: -.25rem 0 0 .35rem;
    color: #a6e3a1;
    font-size: .76rem;
  }
  .workflow-rail {
    display: grid;
    grid-template-columns: repeat(5, minmax(0, 1fr));
    gap: .35rem;
  }
  .workflow-attention {
    grid-column: 1 / -1;
    display: grid;
    grid-template-columns: repeat(3, minmax(0, 1fr));
    gap: .35rem;
  }
  .workflow-attention button {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    gap: .1rem .4rem;
    align-items: baseline;
    min-width: 0;
    border: 1px solid #263144;
    border-radius: 4px;
    background: #0a0d14;
    color: #cdd6f4;
    padding: .36rem .5rem;
    text-align: left;
    cursor: pointer;
    font: inherit;
  }
  .workflow-attention button:hover {
    border-color: #45567a;
    background: #111827;
  }
  .workflow-attention span {
    color: #7f8aa3;
    font-size: .66rem;
    text-transform: uppercase;
    letter-spacing: 0;
  }
  .workflow-attention strong {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: .78rem;
  }
  .workflow-attention small {
    grid-column: 1 / -1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: #9aa3b8;
    font-size: .72rem;
  }
  .workflow-step {
    display: flex;
    flex-direction: column;
    justify-content: center;
    gap: .12rem;
    min-width: 0;
    border: 1px solid #1f2733;
    border-radius: 4px;
    background: #0a0d14;
    color: #cdd6f4;
    padding: .42rem .5rem;
    text-align: left;
    cursor: pointer;
    font: inherit;
  }
  .workflow-step:hover {
    border-color: #45567a;
    background: #111827;
  }
  .workflow-step span {
    color: #7f8aa3;
    font-size: .68rem;
    text-transform: uppercase;
    letter-spacing: 0;
  }
  .workflow-step strong {
    font-size: .82rem;
  }
  .workflow-strip.tone-candidate .workflow-main { border-left-color: rgb(180,190,254); }
  .workflow-strip.tone-blocked .workflow-main { border-left-color: rgb(243,139,168); }
  .workflow-strip.tone-tracking .workflow-main { border-left-color: rgb(137,180,250); }
  .workflow-strip.tone-actionable .workflow-main { border-left-color: rgb(166,227,161); }
  .workflow-strip.tone-monitoring .workflow-main,
  .workflow-strip.tone-ready .workflow-main { border-left-color: rgb(249,226,175); }
  .workflow-strip.tone-declined .workflow-main,
  .workflow-strip.tone-missing .workflow-main { border-left-color: #6c7693; }
  .symbol-status-card {
    margin-bottom: .65rem;
  }
  .symbol-status-card.placement-active,
  .symbol-placement-strip.placement-active { border-left-color: rgb(166, 227, 161); }
  .symbol-status-card.placement-candidate,
  .symbol-placement-strip.placement-candidate { border-left-color: rgb(180, 190, 254); }
  .symbol-status-card.placement-pool,
  .symbol-placement-strip.placement-pool { border-left-color: rgb(249, 226, 175); }
  .symbol-status-card.placement-unknown,
  .symbol-status-card.placement-missing,
  .symbol-placement-strip.placement-unknown,
  .symbol-placement-strip.placement-missing { border-left-color: #6c7693; }
  .symbol-placement-strip {
    display: flex;
    align-items: center;
    gap: .45rem;
    flex-wrap: wrap;
    border: 1px solid #1f2733;
    border-left: 3px solid #6c7693;
    border-radius: 4px;
    background: #0a0d14;
    padding: .4rem .5rem;
    margin-bottom: .55rem;
  }
  .symbol-placement-strip strong {
    color: #cdd6f4;
  }
  .symbol-placement-strip .text-action {
    margin-left: auto;
  }
  .symbol-placement-strip .confirm,
  .promotion-actions .confirm {
    background: rgba(166, 227, 161, .12);
    color: #cdd6f4;
    border: 1px solid rgba(166, 227, 161, .42);
    border-radius: 4px;
    padding: .24rem .58rem;
    font: inherit;
    font-size: .76rem;
    cursor: pointer;
  }
  .symbol-placement-strip .confirm:hover,
  .promotion-actions .confirm:hover {
    background: rgba(166, 227, 161, .2);
  }
  .symbol-placement-strip .confirm:disabled,
  .promotion-actions .confirm:disabled {
    opacity: .55;
    cursor: default;
  }
  .placement-promote {
    display: grid;
    gap: .45rem;
    margin-top: .55rem;
    padding-top: .55rem;
    border-top: 1px solid #1f2733;
  }
  .promotion-destinations.compact {
    align-items: baseline;
  }
  .placement-tags {
    display: inline-flex;
    flex-wrap: wrap;
    gap: .25rem;
  }
  .badge.placement-active {
    color: #a6e3a1;
    background: rgba(166, 227, 161, .16);
  }
  .badge.placement-candidate {
    color: #b4befe;
    background: rgba(180, 190, 254, .16);
  }
  .badge.placement-pool {
    color: #f9e2af;
    background: rgba(249, 226, 175, .16);
  }
  .badge.placement-unknown,
  .badge.placement-missing {
    color: #9aa3b8;
    background: rgba(108, 118, 147, .16);
  }
  .promotion-review {
    grid-column: 1 / -1;
    display: grid;
    gap: .5rem;
    min-width: 0;
    border: 1px solid #2a3548;
    border-left: 3px solid rgb(180, 190, 254);
    border-radius: 4px;
    background: #0a0d14;
    padding: .55rem .65rem;
    font-size: .78rem;
  }
  .promotion-head {
    display: flex;
    align-items: baseline;
    gap: .45rem;
    flex-wrap: wrap;
    min-width: 0;
  }
  .promotion-head > div {
    display: flex;
    flex-direction: column;
    gap: .08rem;
    min-width: 220px;
  }
  .promotion-head strong {
    color: #cdd6f4;
  }
  .promotion-grid {
    display: grid;
    grid-template-columns: repeat(4, minmax(0, 1fr));
    gap: .45rem;
  }
  .promotion-grid > div {
    min-width: 0;
    border: 1px solid #1f2733;
    border-radius: 4px;
    background: #0d121b;
    padding: .42rem .5rem;
  }
  .promotion-label {
    display: block;
    margin-bottom: .2rem;
    color: #7f8aa3;
    font-size: .66rem;
    text-transform: uppercase;
    letter-spacing: 0;
  }
  .promotion-review p {
    margin: 0;
    color: #bac2de;
    line-height: 1.35;
  }
  .promotion-review p + p {
    margin-top: .28rem;
  }
  .promotion-tokens,
  .promotion-destinations,
  .promotion-actions {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: .35rem;
    min-width: 0;
  }
  .promotion-pick {
    max-width: 100%;
  }
  .promotion-pick .muted {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 260px;
  }
  .promotion-actions {
    border-top: 1px solid #1f2733;
    padding-top: .45rem;
  }
  .promotion-actions button {
    font-size: .76rem;
  }
  .side-review-card {
    border: 1px solid #2a3548;
    border-left: 3px solid rgb(180, 190, 254);
    border-radius: 4px;
    background: #0c1019;
    padding: .65rem .7rem;
    margin-bottom: .7rem;
    display: flex;
    flex-direction: column;
    gap: .45rem;
    font-size: .8rem;
  }
  .side-review-card h4 {
    margin: 0;
    color: #cdd6f4;
    font-size: .92rem;
  }
  .side-review-card p {
    margin: 0;
    color: #bac2de;
    line-height: 1.35;
  }
  @media (max-width: 1180px) {
    .workflow-strip {
      grid-template-columns: minmax(0, 1fr);
    }
    .promotion-grid {
      grid-template-columns: repeat(2, minmax(0, 1fr));
    }
  }
  @media (max-width: 760px) {
    .promotion-grid,
    .workflow-rail,
    .workflow-attention {
      grid-template-columns: minmax(0, 1fr);
    }
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
  .wl-filters {
    display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: .35rem;
    margin-bottom: .35rem;
  }
  .wl-filters select {
    min-width: 0; background: #0a0d14; color: #cdd6f4; border: 1px solid #2a3548;
    border-radius: 4px; padding: .2rem .3rem; font: inherit; font-size: .72rem;
  }
  .wl-reset {
    min-width: 0; background: transparent; color: #6c7693; border: 1px solid #2a3548;
    border-radius: 4px; padding: .2rem .3rem; font: inherit; font-size: .72rem;
    cursor: pointer;
  }
  .wl-reset.active { color: #cdd6f4; border-color: #45567a; }
  .wl-reset:disabled { cursor: default; opacity: .45; }
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
    min-width: 0; flex: 1; display: flex; gap: .28rem; align-items: baseline; flex-wrap: wrap;
    border: none; background: transparent; color: inherit; font: inherit;
    text-align: left; padding: 0; cursor: pointer;
  }
  .wl-mem-select strong { flex: 1 0 3.8rem; }
  .wl-mem-select .badge.theme {
    max-width: 9rem; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
  }
  .wl-distance { font-size: .68rem; white-space: nowrap; }
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
  .macro-metrics,
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
  .macro-metric {
    display: inline-flex;
    align-items: baseline;
    gap: .25rem;
  }
  .macro-metric small {
    color: #7f8aa3;
    font-size: .68rem;
  }
  .dislocation-map {
    border: 1px solid #1f2733;
    background: #0c1019;
    border-radius: 4px;
    padding: .45rem .55rem;
    display: flex;
    flex-direction: column;
    gap: .35rem;
  }
  .dislocation-bucket {
    display: flex;
    align-items: baseline;
    gap: .35rem;
    flex-wrap: wrap;
  }
  .dislocation-bucket > .muted {
    min-width: 7.4rem;
    font-size: .72rem;
    text-transform: uppercase;
  }
  .dislocation-bucket .brain-token {
    display: inline-flex;
    align-items: baseline;
    gap: .25rem;
  }
  .dislocation-bucket small {
    color: #7f8aa3;
    font-size: .68rem;
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
    grid-template-columns: auto minmax(0, 1fr) auto;
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
  .brain-ticker.stale-link {
    border-color: rgba(249, 226, 175, .4);
  }
  .brain-ticker span {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .brain-ticker-role {
    color: #8b95ad;
  }
  .brain-ticker .brain-ticker-badges {
    display: inline-flex;
    align-items: center;
    justify-content: flex-end;
    gap: .25rem;
    overflow: visible;
  }
  .brain-ticker .brain-ticker-badges > span {
    overflow: visible;
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
  .event-link {
    appearance: none; border: 0; background: transparent; color: inherit;
    padding: 0; margin: 0; font: inherit; cursor: pointer;
    display: flex; gap: .4rem; align-items: baseline; text-align: left;
  }

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
  .alert-toolbar { margin: .75rem 0 .4rem; }
  .toggle { display: flex; gap: .35rem; align-items: center; font-size: .75rem; color: #6c7693; cursor: pointer; }

  /* Discovery cards in drawer (same as before, compacted) */
  .disc-list { list-style: none; padding: 0; margin: 0; display: flex; flex-direction: column; gap: .5rem; }
  .disc-card {
    background: #0a0d14; border: 1px solid #1f2733; border-radius: 4px;
    padding: .5rem .6rem;
  }
  .disc-hdr { display: flex; gap: .4rem; align-items: baseline; flex-wrap: wrap; }
  .link-symbol {
    appearance: none; border: 0; background: transparent; color: inherit;
    padding: 0; margin: 0; font: inherit; font-weight: 700; cursor: pointer;
  }
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
  .review-decision-panel {
    border: 1px solid #273246;
    border-left: 3px solid #fab387;
    background: #0a0f18;
    border-radius: 4px;
    padding: .65rem;
    display: grid;
    gap: .55rem;
  }
  .review-decision-head {
    display: flex;
    gap: .5rem;
    align-items: baseline;
    flex-wrap: wrap;
  }
  .review-decision-panel .decform {
    max-width: none;
  }
  .decform {
    display: grid; grid-template-columns: 1fr 1fr; gap: .5rem; max-width: 760px;
    font-size: .85rem;
  }
  .decform label { display: flex; flex-direction: column; gap: .15rem; }
  .decform input, .decform select, .decform textarea {
    background: #0a0d14; color: #cdd6f4; border: 1px solid #2a3548; border-radius: 4px;
    padding: .25rem .4rem; font: inherit;
  }
  .decform textarea { resize: vertical; min-height: 3rem; }
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
  .nomination-state {
    border: 1px solid #1f2733;
    border-left: 3px solid rgb(180, 190, 254);
    border-radius: 4px;
    background: #0c1019;
    padding: .6rem .7rem;
    font-size: .8rem;
    display: flex;
    flex-direction: column;
    gap: .4rem;
  }
  .nomination-state p { margin: 0; color: #bac2de; line-height: 1.35; }
  .nomination-hdr {
    display: flex;
    align-items: baseline;
    gap: .4rem;
    flex-wrap: wrap;
  }
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
  .brain-card.brain-not_monitored { border-left-color: rgb(137,180,250); }
  .brain-card.brain-blocked { border-left-color: rgb(243,139,168); }
  .brain-card.technical-overview { border-left-color: #6c7693; }
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
    flex-direction: column;
    gap: .12rem;
    min-width: 0;
  }
  .brain-source-main {
    display: flex;
    align-items: baseline;
    gap: .35rem;
    flex-wrap: wrap;
    min-width: 0;
  }
  .brain-source-detail {
    color: #7f8aa3;
    font-size: .72rem;
    line-height: 1.25;
    overflow-wrap: anywhere;
  }
  .brain-run {
    border-top: 1px solid #1f2733;
    border-bottom: 1px solid #1f2733;
    padding: .4rem 0;
    margin: .45rem 0;
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
  .evidence-card.evidence-item {
    border-left: 3px solid #45567a;
  }
  .evidence-card.evidence-item.tone-positive { border-left-color: rgb(166,227,161); }
  .evidence-card.evidence-item.tone-negative { border-left-color: rgb(243,139,168); }
  .evidence-card.evidence-item.tone-neutral { border-left-color: rgb(137,180,250); }
  .evidence-row {
    display: flex; gap: .4rem; align-items: baseline; flex-wrap: wrap; margin-bottom: .25rem;
  }
  .evidence-card p { margin: 0; color: #bac2de; line-height: 1.35; }
  .task-chips {
    display: flex;
    flex-wrap: wrap;
    gap: .3rem;
    margin-top: .4rem;
  }
  .task-chip {
    border: 1px solid #2a3548;
    border-radius: 999px;
    padding: .1rem .45rem;
    color: #bac2de;
    background: #11161f;
    font-size: .68rem;
    line-height: 1.4;
  }
  .task-chip.tone-running {
    border-color: rgba(166,227,161,.4);
    color: rgb(166,227,161);
  }
  .task-chip.tone-blocked {
    border-color: rgba(243,139,168,.4);
    color: rgb(243,139,168);
  }
  .task-chip.tone-satisfied {
    border-color: rgba(137,180,250,.35);
    color: rgb(137,180,250);
  }
  .inline-action {
    margin-top: .5rem;
    border: 1px solid rgba(166,227,161,.55);
    background: rgba(166,227,161,.14);
    color: #dff7dc;
    border-radius: 4px;
    padding: .28rem .55rem;
    font: inherit;
    font-size: .75rem;
    cursor: pointer;
  }
  .inline-action:disabled {
    cursor: wait;
    opacity: .65;
  }
  .empty-source {
    margin-top: .55rem;
    border: 1px solid #1f2733;
    border-radius: 4px;
    padding: .55rem .65rem;
    background: #0c1019;
  }
  .empty-source h4 {
    margin: 0 0 .35rem;
  }
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
  .badge.health-stale_running { background: rgba(250,179,135,.18); color: rgb(250,179,135); }
  .badge.health-failed { background: rgba(243,139,168,.18); color: rgb(243,139,168); }
  .badge.health-rate_limited { background: rgba(243,139,168,.18); color: rgb(243,139,168); }
  .badge.db-ok { background: rgba(166,227,161,.18); color: rgb(166,227,161); }
  .badge.db-unreachable { background: rgba(243,139,168,.18); color: rgb(243,139,168); }
  .badge.db-unknown { background: rgba(108,112,134,.2); color: #9aa3b8; }
  .badge.brain-fresh,
  .badge.brain-source-fresh { background: rgba(166,227,161,.18); color: rgb(166,227,161); }
  .badge.brain-not_monitored { background: rgba(137,180,250,.16); color: rgb(137,180,250); }
  .badge.brain-fresh-fresh,
  .badge.brain-dir-risk_on,
  .badge.brain-conviction,
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
  .badge.cognition-drafted,
  .badge.cognition-reconciled,
  .badge.cognition-no_change,
  .badge.cognition-context_refreshed { background: rgba(166,227,161,.18); color: rgb(166,227,161); }
  .badge.cognition-running,
  .badge.cognition-declined,
  .badge.cognition-blocked_on_evidence { background: rgba(249,226,175,.15); color: rgb(249,226,175); }
  .badge.cognition-failed { background: rgba(243,139,168,.18); color: rgb(243,139,168); }
  .badge.brain-dir-risk_off,
  .badge.brain-dir-bearish { background: rgba(243,139,168,.18); color: rgb(243,139,168); }
  .badge.tech-constructive,
  .badge.tech-base_building { background: rgba(166,227,161,.18); color: rgb(166,227,161); }
  .badge.tech-extended { background: rgba(249,226,175,.15); color: rgb(249,226,175); }
  .badge.tech-deteriorating { background: rgba(243,139,168,.18); color: rgb(243,139,168); }
  .badge.tech-unknown { background: rgba(108,112,134,.2); color: #9aa3b8; }
  .badge.fresh-fresh { background: rgba(166,227,161,.16); color: rgb(166,227,161); }
  .badge.fresh-stale,
  .badge.fresh-missing { background: rgba(249,226,175,.15); color: rgb(249,226,175); }
  .badge.fresh-blocked { background: rgba(243,139,168,.18); color: rgb(243,139,168); }
  .badge.att-open { background: rgba(137,180,250,.16); color: rgb(137,180,250); }
  .badge.theme { background: rgba(180,190,254,.14); color: rgb(180,190,254); }

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
    .workspace {
      grid-template-rows: auto auto minmax(0, 1fr);
    }
    .workflow-strip {
      grid-template-columns: 1fr;
      min-height: 0;
      padding: .4rem .5rem;
    }
    .workflow-main {
      grid-template-columns: minmax(0, 1fr);
      gap: .35rem;
    }
    .workflow-primary {
      width: 100%;
    }
    .workflow-rail {
      grid-template-columns: repeat(2, minmax(0, 1fr));
    }
    .workflow-attention {
      grid-template-columns: minmax(0, 1fr);
    }
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
  .badge.reason { background: rgba(249,226,175,.15); color: rgb(249,226,175); }
  .decision-detail {
    flex-basis: 100%;
    margin: 0;
    color: #9aa3b8;
    font-size: .8rem;
  }
  .decision-replay {
    border: 1px solid #263144; border-left: 3px solid #89b4fa;
    border-radius: 4px; padding: .45rem .55rem; margin: .5rem 0;
    background: #0a0d14; font-size: .78rem;
  }
  .replay-head {
    display: flex; align-items: baseline; gap: .35rem; flex-wrap: wrap;
    margin-bottom: .4rem;
  }
  .replay-head .link-mini { margin-left: auto; }
  .replay-grid {
    display: grid; grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: .35rem; margin-bottom: .35rem;
  }
  .replay-grid > div { display: flex; flex-direction: column; gap: .05rem; }
  .replay-risk { margin: .3rem 0; color: #bac2de; }
  .replay-evidence {
    list-style: none; margin: .4rem 0 0; padding: 0;
    display: flex; flex-direction: column; gap: .25rem;
  }
  .replay-evidence li {
    display: flex; gap: .35rem; align-items: baseline;
    border-top: 1px solid #1f2733; padding-top: .25rem;
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
  .badge.tech-extended,
  .badge.stance-avoid_chase,
  .badge.stance-wait_breakout,
  .badge.stance-wait_data {
    background: rgba(249,226,175,.15); color: rgb(249,226,175);
  }
  .badge.tech-constructive,
  .badge.stance-constructive {
    background: rgba(166,227,161,.16); color: rgb(166,227,161);
  }
  .badge.tech-base_building {
    background: rgba(137,180,250,.15); color: rgb(137,180,250);
  }
  .badge.tech-deteriorating,
  .badge.stance-avoid {
    background: rgba(243,139,168,.16); color: rgb(243,139,168);
  }
  .badge.tech-unknown,
  .badge.tech-none,
  .badge.stance-none {
    background: rgba(108,112,134,.16); color: #9aa3b8;
  }
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
  .diag-alert { border-color: rgba(243,139,168,.45); background: rgba(243,139,168,.06); }
  .diag h5 { margin: 0 0 0.5rem 0; font-size: 0.8rem; color: #bac2de; font-weight: 600; }
  .diag-reason { margin: 0.4rem 0 0; color: #f38ba8; font-size: 0.78rem; overflow-wrap: anywhere; }
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
