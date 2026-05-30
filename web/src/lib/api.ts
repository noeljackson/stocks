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
