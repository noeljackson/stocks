export interface Alert {
  id: number;
  thesis_id?: string | null;
  symbol?: string | null;
  kind: string;
  payload?: Record<string, unknown>;
  acknowledged: boolean;
  created_at: string;
}

export interface StreamEvent {
  subject: string;
  kind: string;
  payload: Record<string, unknown>;
}

export interface MarketState {
  as_of?: string;
  regime: "risk_on" | "neutral" | "risk_off" | "unknown";
  capitulation: boolean;
  indicators: Record<string, number>;
}

export interface Ticker {
  symbol: string;
  cluster_id: string;
  cluster_name?: string | null;
  tier: number;
  options_eligible: boolean;
  domain_fit?: number | null;
  added_at: string;
  open_theses: number;
  latest_thesis_id?: string | null;
  thesis_state?: string | null;
  thesis_direction?: string | null;
  technical_state?: string | null;
  entry_stance?: string | null;
  technical_pct_vs_200d?: number | null;
  freshness_status?: string | null;
  open_attention?: number;
  attention_states?: WatchlistAttentionState[];
  attention_owners?: WatchlistAttentionOwner[];
  open_evidence?: number;
  blocking_evidence?: number;
  due_source_tasks?: number;
  parent_themes?: WatchlistParentTheme[];
}

export async function fetchAlerts(opts?: { unacked?: boolean }): Promise<Alert[]> {
  const q = opts?.unacked ? "?unacked=true" : "";
  const r = await fetch(`/api/alerts${q}`);
  if (!r.ok) throw new Error(`alerts ${r.status}`);
  return ((await r.json()) as Alert[] | null) ?? [];
}

export async function ackAlert(id: number): Promise<void> {
  const r = await fetch(`/api/alerts/${id}/ack`, { method: "POST" });
  if (!r.ok && r.status !== 204) throw new Error(`ack ${r.status}`);
}

export async function fetchRegime(): Promise<MarketState> {
  const r = await fetch("/api/regime");
  if (!r.ok) throw new Error(`regime ${r.status}`);
  return (await r.json()) as MarketState;
}

export async function fetchTickers(): Promise<Ticker[]> {
  const r = await fetch("/api/tickers");
  if (!r.ok) throw new Error(`tickers ${r.status}`);
  return ((await r.json()) as Ticker[] | null) ?? [];
}

export interface Condition {
  type: "quantitative" | "narrative";
  name: string;
  expr?: string;
  assertion?: string;
}

export interface ThesisVersionEvent {
  version: number;
  weakens_invalidation: boolean;
  diff: Record<string, unknown>;
  rationale?: string | null;
  at: string;
}

export interface WellFormedCondCounts {
  conviction: number;
  trigger: number;
  invalidation: number;
  fulfillment: number;
}

export interface ThesisSubstance {
  score: number;
  max_score: number;
  missing: string[];
  blocked_at: string | null;
  well_formed: WellFormedCondCounts;
}

export interface ThesisDetail {
  thesis_id: string;
  symbol: string;
  cluster_id?: string | null;
  cluster_thesis?: string | null;
  state: string;
  edge_rationale: string;
  bull_case?: string | null;
  bear_case?: string | null;
  forecast: Record<string, unknown> | null;
  conviction_conditions: Condition[];
  trigger_conditions: Condition[];
  invalidation_conditions: Condition[];
  fulfillment_conditions: Condition[];
  conviction_tier?: string | null;
  instrument?: string | null;
  intended_size: Record<string, unknown> | null;
  version: number;
  immutable_original: {
    edge_rationale?: string;
    invalidation_conditions?: Condition[];
    [key: string]: unknown;
  };
  created_at: string;
  updated_at: string;
  last_evaluated_at?: string | null;
  history: ThesisVersionEvent[];
  evidence_items?: EvidenceItem[];
  substance?: ThesisSubstance | null;
}

export async function fetchTheses(symbol: string): Promise<ThesisDetail[]> {
  const r = await fetch(`/api/theses?symbol=${encodeURIComponent(symbol)}`);
  if (!r.ok) throw new Error(`theses ${r.status}`);
  return ((await r.json()) as ThesisDetail[] | null) ?? [];
}

export interface ThesisDecline {
  id: number;
  symbol?: string | null;
  candidate_id?: number | null;
  severity: string;
  status: "open" | "resolved" | "dismissed";
  title: string;
  reason?: string | null;
  source_ref: Record<string, unknown>;
  created_at: string;
  resolved_at?: string | null;
  resolution_kind?: string | null;
}

export async function fetchThesisDeclines(symbol: string): Promise<ThesisDecline[]> {
  const r = await fetch(`/api/thesis-declines?symbol=${encodeURIComponent(symbol)}`);
  if (!r.ok) throw new Error(`thesis declines ${r.status}`);
  return ((await r.json()) as ThesisDecline[] | null) ?? [];
}

export interface EvidenceRequirement {
  id: number;
  symbol: string;
  requirement_key: string;
  source_type: string;
  reason: string;
  priority: "low" | "medium" | "high" | "blocking";
  blocking_state: "missing" | "fetching" | "partial" | "blocked" | "satisfied";
  attempts: number;
  next_retry_at?: string | null;
  last_error?: string | null;
  source_ref: Record<string, unknown>;
  source_tasks?: {
    id: number;
    action: string;
    provider: string;
    state: string;
    priority: string;
    due_at?: string | null;
    next_retry_at?: string | null;
    attempts: number;
    last_error?: string | null;
    updated_at?: string | null;
  }[];
  created_at: string;
  updated_at: string;
  satisfied_at?: string | null;
}

export async function fetchEvidenceRequirements(symbol: string): Promise<EvidenceRequirement[]> {
  const r = await fetch(`/api/evidence-requirements?symbol=${encodeURIComponent(symbol)}`);
  if (!r.ok) throw new Error(`evidence requirements ${r.status}`);
  return ((await r.json()) as EvidenceRequirement[] | null) ?? [];
}

export interface ResearchEvidence {
  id: number;
  symbol: string;
  query: string;
  url: string;
  title: string;
  publisher?: string | null;
  published_at?: string | null;
  retrieved_at: string;
  provider: string;
  source_type: string;
  credibility: "primary" | "credible_media" | "industry" | "unknown";
  summary?: string | null;
  tags: string[];
}

export async function fetchResearchEvidence(symbol: string): Promise<ResearchEvidence[]> {
  const r = await fetch(`/api/research-evidence?symbol=${encodeURIComponent(symbol)}`);
  if (!r.ok) throw new Error(`research evidence ${r.status}`);
  return ((await r.json()) as ResearchEvidence[] | null) ?? [];
}

export interface EvidenceItem {
  id: number;
  symbol: string;
  kind: string;
  observed_at: string;
  source: string;
  source_id: string;
  source_ref: Record<string, unknown>;
  summary: string;
  strength?: number | null;
  polarity?: number | null;
  url?: string | null;
  created_at: string;
  updated_at?: string | null;
  weight?: number | null;
  added_by?: string | null;
}

export async function fetchEvidenceItems(symbol: string): Promise<EvidenceItem[]> {
  const r = await fetch(`/api/evidence-items?symbol=${encodeURIComponent(symbol)}`);
  if (!r.ok) throw new Error(`evidence items ${r.status}`);
  return ((await r.json()) as EvidenceItem[] | null) ?? [];
}

export interface BrainSourceStatus {
  source: string;
  status: "fresh" | "stale" | "missing" | "running" | "failed" | "rate_limited";
  last_changed_at?: string | null;
  last_checked_at?: string | null;
  retry_after_at?: string | null;
  failure_kind?: string | null;
  last_error?: string | null;
  max_age_minutes?: number | null;
  detail?: Record<string, unknown> | null;
  source_health?: Record<string, unknown> | null;
  source_tasks?: {
    requirement_key?: string | null;
    action: string;
    provider: string;
    state: string;
    priority: string;
    due_at?: string | null;
    next_retry_at?: string | null;
    attempts: number;
    last_error?: string | null;
    updated_at?: string | null;
  }[];
  version?: number | null;
  thesis_id?: string | null;
  state?: string | null;
  direction?: string | null;
}

export interface CognitionRun {
  id: number;
  symbol: string;
  trigger: string;
  sweep_reason?: string | null;
  status:
    | "running"
    | "context_refreshed"
    | "blocked_on_evidence"
    | "declined"
    | "drafted"
    | "reconciled"
    | "no_change"
    | "failed";
  reason?: string | null;
  context_version?: number | null;
  thesis_id?: string | null;
  thesis_classification?: string | null;
  evidence_open_count: number;
  evidence_blocking_count: number;
  started_at: string;
  finished_at?: string | null;
  next_retry_at?: string | null;
  error?: string | null;
  source_ref?: Record<string, unknown>;
}

export interface BrainStatus {
  symbol: string;
  as_of: string;
  active_ticker: boolean;
  status: "fresh" | "due" | "stale" | "waiting_on_evidence" | "blocked";
  next_action: string;
  reason: string;
  freshness_target_minutes: number;
  sources: BrainSourceStatus[];
  evidence: {
    rows: number;
    open: number;
    blocking: number;
    due: number;
    items?: number;
    latest_item_at?: string | null;
    delta?: boolean;
  };
  attention: {
    open: number;
    by_kind: { kind: string; count: number }[];
  };
  cognition: {
    last_run?: CognitionRun | null;
    recent_runs: CognitionRun[];
  };
}

export async function fetchBrainStatus(symbol: string): Promise<BrainStatus | null> {
  const r = await fetch(`/api/brain-status?symbol=${encodeURIComponent(symbol)}`);
  if (r.status === 204) return null;
  if (!r.ok) throw new Error(`brain-status ${r.status}`);
  return (await r.json()) as BrainStatus;
}

export interface BrainLinkedTicker {
  symbol: string;
  role: string;
  rationale?: string | null;
  conviction?: number | null;
  thesis_state?: string | null;
  thesis_direction?: string | null;
  open_theses?: number;
}

export interface BrainLinkedWatchlist {
  id: string;
  name: string;
  color?: string | null;
  is_system?: boolean;
}

export interface BrainNomination {
  candidate_id: number;
  symbol: string;
  signal_name: string;
  signal_value?: number | null;
  reasoning?: string | null;
  proposed_at: string;
}

export interface BrainThesisChange {
  version: number;
  rationale?: string | null;
  at: string;
}

export interface BrainThesis {
  id: string;
  scope: "macro" | "sector" | "theme";
  key: string;
  name: string;
  state: "forming" | "active" | "weakening" | "invalidated" | "archived";
  direction: "risk_on" | "risk_off" | "neutral" | "bullish" | "bearish" | "mixed";
  summary: string;
  core_claim: string;
  why_now?: string | null;
  evidence: unknown[];
  invalidation_conditions: unknown[];
  beneficiaries: unknown[];
  losers: unknown[];
  open_questions: unknown[];
  missing_evidence: unknown[];
  source_ref: Record<string, unknown>;
  freshness_target_minutes: number;
  last_evaluated_at?: string | null;
  version: number;
  created_at: string;
  updated_at: string;
  freshness: "fresh" | "stale" | "missing";
  tickers: BrainLinkedTicker[];
  watchlists: BrainLinkedWatchlist[];
  nominations: BrainNomination[];
  latest_changes: BrainThesisChange[];
}

export interface BrainOverview {
  as_of: string;
  market_state?: MarketState | null;
  macro?: BrainThesis | null;
  sectors: BrainThesis[];
  contradictions: { kind: string; summary: string; brain_thesis_key?: string | null }[];
  summary: {
    active_theses: number;
    stale_or_missing: number;
    open_nominations: number;
  };
}

export async function fetchBrainOverview(): Promise<BrainOverview> {
  const r = await fetch("/api/brain");
  if (!r.ok) throw new Error(`brain ${r.status}`);
  return (await r.json()) as BrainOverview;
}

export interface TickerContext {
  symbol: string;
  version: number;
  structural: Record<string, unknown>;
  structural_as_of?: string | null;
  narrative: Record<string, unknown>;
  narrative_as_of?: string | null;
  market: Record<string, unknown>;
  market_as_of?: string | null;
  created_at: string;
}

/** Returns `null` when there's no context yet (204 No Content). */
export async function fetchTickerContext(symbol: string): Promise<TickerContext | null> {
  const r = await fetch(`/api/ticker-context?symbol=${encodeURIComponent(symbol)}`);
  if (r.status === 204) return null;
  if (!r.ok) throw new Error(`ticker-context ${r.status}`);
  return (await r.json()) as TickerContext;
}

export interface SmaPoint {
  window: number;
  value?: number | null;
  pct_vs?: number | null;
}

export interface IntervalTechnical {
  interval: string;
  bars: number;
  as_of?: string | null;
  close?: number | null;
  rsi14?: number | null;
  rsi_zone: string;
  rsi_zone_bars: number;
  rsi_zone_since?: string | null;
}

export interface DailyTechnical {
  as_of: string;
  close: number;
  sma: SmaPoint[];
  pct_vs_252d_high?: number | null;
  pct_vs_252d_low?: number | null;
}

export interface CrossEvent {
  window: number;
  direction: string;
  at: string;
  close: number;
  sma: number;
}

export interface AnalogEvent {
  kind: string;
  at: string;
  close: number;
  rsi14: number;
  forward_return_20d_pct?: number | null;
  max_drawdown_20d_pct?: number | null;
}

export interface TechnicalState {
  symbol: string;
  as_of?: string | null;
  state: string;
  setup: {
    kind: string;
    entry_stance: string;
    summary: string;
  };
  summary: string;
  daily?: DailyTechnical | null;
  intervals: IntervalTechnical[];
  last_crosses: CrossEvent[];
  analog_events: AnalogEvent[];
}

export async function fetchTechnicalState(symbol: string): Promise<TechnicalState> {
  const r = await fetch(`/api/technical-state?symbol=${encodeURIComponent(symbol)}`);
  if (!r.ok) throw new Error(`technical-state ${r.status}`);
  return (await r.json()) as TechnicalState;
}

export interface ChatEvidenceRef {
  source: string;
  evidence_id?: number | null;
  summary: string;
  observed_at?: string | null;
}

export interface ChatRequestedEvidence {
  requirement_key: string;
  source_type: string;
  priority: "blocking" | "high" | "medium" | "low";
  reason: string;
}

export interface ChatAnalystAnswer {
  answer: string;
  confidence: "high" | "medium" | "low";
  evidence_used: ChatEvidenceRef[];
  technical_read: {
    state?: string | null;
    summary?: string | null;
    timing_implication?: string | null;
  };
  thesis_impact: {
    kind: "no_change" | "supports" | "weakens" | "contradicts" | "needs_reconciliation";
    reason?: string | null;
  };
  requested_evidence: ChatRequestedEvidence[];
  attention_request: {
    kind: "none" | "thesis_review" | "decision_review" | "source_followup";
    reason?: string | null;
  };
}

export interface ChatAnalystResponse {
  scope: "symbol" | "theme" | "macro" | "technical" | "decision";
  symbol?: string | null;
  answer: ChatAnalystAnswer;
  queued_evidence: number;
  used_fallback: boolean;
  fallback_reason?: string | null;
}

export async function askChatAnalyst(body: {
  question: string;
  symbol?: string | null;
  scope?: string | null;
}): Promise<ChatAnalystResponse> {
  const r = await fetch("/api/chat-analyst", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  });
  if (!r.ok) throw new Error(`chat analyst ${r.status}: ${await r.text()}`);
  return (await r.json()) as ChatAnalystResponse;
}

export interface Calibration {
  predictions_total: number;
  outcomes_scored: number;
  mean_brier: number | null;
  mean_lead_time_days: number | null;
  median_lead_time_days: number | null;
  parent_themes: {
    key: string;
    name: string;
    scope: string;
    role: string;
    predictions_total: number;
    outcomes_scored: number;
    mean_brier: number | null;
    mean_lead_time_days: number | null;
  }[];
}

export async function fetchCalibration(days = 90): Promise<Calibration> {
  const r = await fetch(`/api/calibration?days=${days}`);
  if (!r.ok) throw new Error(`calibration ${r.status}`);
  return (await r.json()) as Calibration;
}

export interface Watchlist {
  id: string;
  name: string;
  description?: string | null;
  color?: string | null;
  is_system: boolean;
  created_at: string;
  member_count: number;
}

export interface WatchlistAttentionState {
  state: string;
  count: number;
}

export interface WatchlistAttentionOwner {
  owner: string;
  count: number;
}

export interface WatchlistParentTheme {
  key: string;
  name: string;
  scope: string;
  state: string;
  direction: string;
  role: string;
  conviction?: number | null;
}

export interface WatchlistMember {
  watchlist_id: string;
  symbol: string;
  added_at: string;
  added_by?: string | null;
  latest_thesis_id?: string | null;
  thesis_state?: string | null;
  thesis_direction?: string | null;
  technical_state?: string | null;
  entry_stance?: string | null;
  technical_pct_vs_200d?: number | null;
  open_theses?: number;
  freshness_status?: string | null;
  open_attention?: number;
  attention_states?: WatchlistAttentionState[];
  attention_owners?: WatchlistAttentionOwner[];
  open_evidence?: number;
  blocking_evidence?: number;
  due_source_tasks?: number;
  parent_themes?: WatchlistParentTheme[];
}

export async function fetchWatchlists(): Promise<Watchlist[]> {
  const r = await fetch("/api/watchlists");
  if (!r.ok) throw new Error(`watchlists ${r.status}`);
  return ((await r.json()) as Watchlist[] | null) ?? [];
}

export async function fetchWatchlistMembers(id: string): Promise<WatchlistMember[]> {
  const r = await fetch(`/api/watchlists/${id}/members`);
  if (!r.ok) throw new Error(`watchlist members ${r.status}`);
  return ((await r.json()) as WatchlistMember[] | null) ?? [];
}

export async function createWatchlist(body: { name: string; description?: string; color?: string }): Promise<{ id: string }> {
  const r = await fetch("/api/watchlists", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  });
  if (!r.ok) throw new Error(`create watchlist ${r.status}`);
  return (await r.json()) as { id: string };
}

export async function addToWatchlist(id: string, symbol: string, addedBy = "user"): Promise<void> {
  const r = await fetch(`/api/watchlists/${id}/members`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ symbol, added_by: addedBy }),
  });
  if (!r.ok && r.status !== 204) throw new Error(`add member ${r.status}`);
}

export async function removeFromWatchlist(id: string, symbol: string): Promise<void> {
  const r = await fetch(`/api/watchlists/${id}/members/${encodeURIComponent(symbol)}`, {
    method: "DELETE",
  });
  if (!r.ok && r.status !== 204) throw new Error(`remove member ${r.status}`);
}

export async function deleteWatchlist(id: string): Promise<void> {
  const r = await fetch(`/api/watchlists/${id}`, { method: "DELETE" });
  if (!r.ok && r.status !== 204) throw new Error(`delete watchlist ${r.status}`);
}

export interface ProposedList {
  watchlist_id?: string | null;
  watchlist_name: string;
  confidence: string;
  rationale: string;
}

export interface SuggestedNewList {
  name: string;
  description: string;
  rationale: string;
}

export interface PendingCandidate {
  id: number;
  symbol: string;
  signal_name: string;
  signal_value: number | null;
  domain_fit?: number | null;
  parent_theme_fit?: number | null;
  parent_themes?: {
    key: string;
    name: string;
    scope: string;
    role: string;
    conviction: number | null;
    rationale: string | null;
  }[];
  proposed_tier?: number;
  reasoning: string | null;
  proposed_at: string;
  proposed_lists: ProposedList[];
  suggested_new_list: SuggestedNewList | null;
  rank_score?: number;
  rank_bucket?: "highest" | "high" | "medium" | "low";
  rank_reasons?: string[];
}

export async function fetchPendingCandidates(): Promise<PendingCandidate[]> {
  const r = await fetch("/api/discovery/candidates");
  if (!r.ok) throw new Error(`pending candidates ${r.status}`);
  return ((await r.json()) as PendingCandidate[] | null) ?? [];
}

export async function confirmCandidate(id: number, watchlistIds: string[]): Promise<void> {
  const r = await fetch(`/api/discovery/candidates/${id}/confirm`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ watchlist_ids: watchlistIds }),
  });
  if (!r.ok && r.status !== 204) throw new Error(`confirm ${r.status}`);
}

export async function rejectCandidate(id: number): Promise<void> {
  const r = await fetch(`/api/discovery/candidates/${id}/reject`, { method: "POST" });
  if (!r.ok && r.status !== 204) throw new Error(`reject ${r.status}`);
}

/** subscribe opens the SSE feed; returns a cleanup function. */
export function subscribe(
  onEvent: (e: StreamEvent) => void,
  onState?: (open: boolean) => void,
): () => void {
  const es = new EventSource("/api/stream");
  es.onopen = () => onState?.(true);
  es.onerror = () => onState?.(false);
  es.onmessage = (m) => {
    try {
      onEvent(JSON.parse(m.data) as StreamEvent);
    } catch {
      /* ignore non-JSON keepalive comments */
    }
  };
  return () => es.close();
}

export interface PoolMember {
  symbol: string;
  company_name?: string | null;
  sector?: string | null;
  industry?: string | null;
  market_cap?: number | null;
  first_seen_at: string;
  latest_thesis_id?: string | null;
  thesis_state?: string | null;
  thesis_direction?: string | null;
  technical_state?: string | null;
  entry_stance?: string | null;
  technical_pct_vs_200d?: number | null;
  open_theses?: number;
  freshness_status?: string | null;
  open_attention?: number;
  attention_states?: WatchlistAttentionState[];
  attention_owners?: WatchlistAttentionOwner[];
  open_evidence?: number;
  blocking_evidence?: number;
  due_source_tasks?: number;
  parent_themes?: WatchlistParentTheme[];
}

export async function fetchDiscoveryPool(): Promise<PoolMember[]> {
  const r = await fetch("/api/discovery-pool");
  if (!r.ok) throw new Error(`pool ${r.status}`);
  return ((await r.json()) as PoolMember[] | null) ?? [];
}

export interface AttentionItem {
  id: number;
  kind: string;
  symbol?: string | null;
  thesis_id?: string | null;
  candidate_id?: number | null;
  severity: string;
  status: string;
  fsm_state?: string | null;
  owner?: string | null;
  title: string;
  reason?: string | null;
  source: string;
  source_ref: Record<string, unknown>;
  created_at: string;
  resolved_at?: string | null;
  resolution_kind?: string | null;
  next_retry_at?: string | null;
  resurface_at?: string | null;
  state_reason?: string | null;
}

export async function fetchAttention(status = "open"): Promise<AttentionItem[]> {
  const r = await fetch(`/api/attention?status=${status}`);
  if (!r.ok) throw new Error(`attention ${r.status}`);
  return ((await r.json()) as AttentionItem[] | null) ?? [];
}

export async function dismissAttention(id: number, reason?: string): Promise<void> {
  const r = await fetch(`/api/attention/${id}/dismiss`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ reason }),
  });
  if (!r.ok && r.status !== 204) throw new Error(`dismiss ${r.status}`);
}

export interface AttentionTransitionRequest {
  to_state: string;
  owner?: string;
  reason?: string;
  next_retry_at?: string | null;
  resurface_at?: string | null;
  source_ref?: Record<string, unknown>;
}

export async function transitionAttention(
  id: number,
  body: AttentionTransitionRequest,
): Promise<void> {
  const r = await fetch(`/api/attention/${id}/transition`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  });
  if (!r.ok && r.status !== 204) throw new Error(`attention transition ${r.status}`);
}

export interface DecisionRow {
  decision_id: string;
  thesis_id?: string | null;
  action: string;
  user_choice?: string | null;
  sizing?: Record<string, unknown> | null;
  thesis_state?: string | null;
  thesis_direction?: string | null;
  side?: string | null;
  instrument?: string | null;
  has_replay?: boolean;
  at: string;
}

export interface DecisionReplay {
  decision_id: string;
  symbol: string;
  thesis_id?: string | null;
  context_version?: number | null;
  thesis_snapshot: Record<string, unknown>;
  consensus_score?: number | null;
  risk_verdict: Record<string, unknown>;
  evidence_ids: number[];
  evidence_snapshot: EvidenceItem[];
  system_confidence?: string | null;
  chart_range_seen?: string | null;
  decision_snapshot: Record<string, unknown>;
  captured_at: string;
}

export interface PositionRow {
  position_id: string;
  thesis_id?: string | null;
  symbol: string;
  side: string;
  instrument: string;
  qty: number;
  avg_price: number;
  delta_notional: number;
  premium_at_risk: number;
  opened_at: string;
  closed_at?: string | null;
  realized_pnl?: number | null;
  unrealized_pnl?: number | null;
  latest_price?: number | null;
  latest_price_at?: string | null;
  fill_count: number;
  thesis_state?: string | null;
  thesis_direction?: string | null;
}

export async function fetchDecisions(symbol: string): Promise<DecisionRow[]> {
  const r = await fetch(`/api/decisions?symbol=${encodeURIComponent(symbol)}`);
  if (!r.ok) throw new Error(`decisions ${r.status}`);
  return ((await r.json()) as DecisionRow[] | null) ?? [];
}

export async function fetchDecisionReplay(decisionId: string): Promise<DecisionReplay> {
  const r = await fetch(`/api/decisions/${encodeURIComponent(decisionId)}/replay`);
  if (!r.ok) throw new Error(`decision replay ${r.status}`);
  return (await r.json()) as DecisionReplay;
}

export async function fetchPositions(symbol: string): Promise<PositionRow[]> {
  const r = await fetch(`/api/positions?symbol=${encodeURIComponent(symbol)}`);
  if (!r.ok) throw new Error(`positions ${r.status}`);
  return ((await r.json()) as PositionRow[] | null) ?? [];
}

export async function postDecision(d: {
  thesis_id?: string;
  action: string;
  user_choice: string;
  sizing?: unknown;
  manual_fill?: unknown;
  chart_range_seen?: string;
}): Promise<Record<string, unknown>> {
  const r = await fetch("/api/decisions", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(d),
  });
  if (!r.ok && r.status !== 204) {
    const body = await r.text().catch(() => "");
    throw new Error(`decision ${r.status}${body ? `: ${body}` : ""}`);
  }
  if (r.status === 204) return {};
  return (await r.json()) as Record<string, unknown>;
}
