export interface Alert {
  id: number;
  thesis_id?: string | null;
  symbol?: string;
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
}

export async function fetchAlerts(): Promise<Alert[]> {
  const r = await fetch("/api/alerts");
  if (!r.ok) throw new Error(`alerts ${r.status}`);
  return ((await r.json()) as Alert[] | null) ?? [];
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
  history: ThesisVersionEvent[];
}

export async function fetchTheses(symbol: string): Promise<ThesisDetail[]> {
  const r = await fetch(`/api/theses?symbol=${encodeURIComponent(symbol)}`);
  if (!r.ok) throw new Error(`theses ${r.status}`);
  return ((await r.json()) as ThesisDetail[] | null) ?? [];
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

export async function postDecision(d: {
  thesis_id?: string;
  action: string;
  user_choice: string;
  sizing?: unknown;
}): Promise<void> {
  const r = await fetch("/api/decisions", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(d),
  });
  if (!r.ok && r.status !== 204) throw new Error(`decision ${r.status}`);
}
