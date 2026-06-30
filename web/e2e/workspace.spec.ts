import { expect, type Page, type Route, test } from "@playwright/test";

type Calls = {
  candleUrls: URL[];
  confirmBody: unknown | null;
  promoteBody: unknown | null;
  decisionBody: unknown | null;
  addedSymbols: string[];
  refreshContextSymbols: string[];
  researchSymbols: string[];
  priceAlertBodies: unknown[];
};

type MockWatchlistMember = {
  watchlist_id: string;
  symbol: string;
  added_at: string;
  added_by: string;
  latest_thesis_id?: string | null;
  thesis_state?: string | null;
  thesis_direction?: string | null;
  technical_state?: string | null;
  entry_stance?: string | null;
  technical_pct_vs_200d?: number | null;
  open_theses?: number;
  freshness_status?: string | null;
  open_attention?: number;
  attention_states?: { state: string; count: number }[];
  attention_owners?: { owner: string; count: number }[];
  open_evidence?: number;
  blocking_evidence?: number;
  due_source_tasks?: number;
  parent_themes?: {
    key: string;
    name: string;
    scope: string;
    state: string;
    direction: string;
    role: string;
    conviction?: number | null;
  }[];
};

type MockTechnicalRead = {
  stance: string;
  label: string;
  reason: string;
  trend: string;
  momentum: string;
  relative_strength: string;
  volume: string;
  volatility: string;
  source: string;
};

const technicalRead = (
  stance: string,
  label: string,
  trend: string,
  momentum: string,
  relativeStrength: string,
  volume: string,
  volatility: string,
): MockTechnicalRead => ({
  stance,
  label,
  trend,
  momentum,
  relative_strength: relativeStrength,
  volume,
  volatility,
  reason: `trend ${trend}; momentum ${momentum}; relative strength ${relativeStrength}; volume ${volume}; volatility ${volatility}`,
  source: "cross_technical",
});

const constructiveRead = technicalRead("constructive", "constructive", "uptrend", "positive", "outperforming", "normal", "normal");
const avoidChaseRead = technicalRead("avoid_chase", "avoid chase", "extended chase", "extended", "outperforming", "normal", "wide");
const avoidBreakdownRead = technicalRead("avoid_breakdown", "avoid breakdown", "breakdown", "weak", "underperforming", "distribution", "expansion");

function isoDate(offset: number): string {
  const d = new Date(Date.UTC(2025, 0, 1 + offset));
  return d.toISOString().slice(0, 10);
}

function dailyCandles(count = 260) {
  return Array.from({ length: count }, (_, i) => ({
    time: isoDate(i),
    open: 100 + i * 0.4,
    high: 102 + i * 0.4,
    low: 99 + i * 0.4,
    close: 101 + i * 0.4,
    volume: 1_000_000 + i * 1000,
  }));
}

function hourlyCandles(count = 120) {
  const start = Date.UTC(2026, 0, 5, 14, 30);
  return Array.from({ length: count }, (_, i) => ({
    time: new Date(start + i * 60 * 60 * 1000).toISOString(),
    open: 180 + i * 0.2,
    high: 181 + i * 0.2,
    low: 179 + i * 0.2,
    close: 180.5 + i * 0.2,
    volume: 500_000 + i * 500,
  }));
}

async function json(route: Route, body: unknown, status = 200) {
  await route.fulfill({
    status,
    contentType: "application/json",
    body: JSON.stringify(body),
  });
}

async function mockApi(
  page: Page,
  options: {
    attentionItems?: Record<string, unknown>[];
    streamEvents?: Record<string, unknown>[];
  } = {},
): Promise<Calls> {
  const calls: Calls = {
    candleUrls: [],
    confirmBody: null,
    promoteBody: null,
    decisionBody: null,
    addedSymbols: [],
    refreshContextSymbols: [],
    researchSymbols: [],
    priceAlertBodies: [],
  };
  let attentionOpen = true;
  const attentionItems = options.attentionItems ?? [{
    id: 7001,
    kind: "candidate_review",
    symbol: "NVDA",
    thesis_id: null,
    candidate_id: 44,
    severity: "review",
    status: "open",
    fsm_state: "ready_for_review",
    owner: "operator",
    title: "NVDA via volume_anomaly",
    reason: "2.4x volume vs SMA",
    source: "discovery",
    source_ref: { raw_signals: ["volume_anomaly"], interpretation_kind: "volume_breakout" },
    created_at: "2026-06-01T00:00:00Z",
    resolved_at: null,
    resolution_kind: null,
    next_retry_at: null,
    resurface_at: null,
    state_reason: "candidate_review",
  }];
  const streamEvents = options.streamEvents ?? [];
  if (streamEvents.length > 0) {
    await page.addInitScript((events: Record<string, unknown>[]) => {
      class MockEventSource {
        static readonly CONNECTING = 0;
        static readonly OPEN = 1;
        static readonly CLOSED = 2;
        readonly CONNECTING = 0;
        readonly OPEN = 1;
        readonly CLOSED = 2;
        readonly url: string;
        readonly withCredentials = false;
        readyState = MockEventSource.CONNECTING;
        onopen: ((event: Event) => void) | null = null;
        onmessage: ((event: MessageEvent) => void) | null = null;
        onerror: ((event: Event) => void) | null = null;

        constructor(url: string) {
          this.url = url;
          window.setTimeout(() => {
            this.readyState = MockEventSource.OPEN;
            this.onopen?.(new Event("open"));
            [
              { subject: "stream.connected", kind: "stream", payload: { status: "open" } },
              ...events,
            ].forEach((event) => {
              this.onmessage?.(new MessageEvent("message", { data: JSON.stringify(event) }));
            });
          }, 0);
        }

        close() {
          this.readyState = MockEventSource.CLOSED;
        }

        addEventListener() {}
        removeEventListener() {}
        dispatchEvent() {
          return true;
        }
      }

      window.EventSource = MockEventSource as typeof EventSource;
    }, streamEvents);
  }
  const workflowAttentionFor = (symbol: string) =>
    attentionOpen
      ? attentionItems.filter((item) => typeof item.symbol === "string" && item.symbol === symbol)
      : [];
  const workflowStep = (key: string, label: string, value: string, action: string, tone = "muted") => ({
    key,
    label,
    value,
    action,
    tone,
  });
  const symbolWorkflow = (symbol: string) => {
    const items = workflowAttentionFor(symbol);
    const candidate = items.find((item) => item.kind === "candidate_review");
    if (symbol === "ORCL" && candidate) {
      return {
        symbol,
        state: "nominated",
        state_label: "Nominated, not active",
        tone: "candidate",
        reason: String(candidate.reason ?? "Discovery nominated this symbol for operator review."),
        primary_action: {
          kind: "attention",
          label: "Promote to Universe",
          detail: "Open the review packet and choose Universe/watchlist destinations.",
          attention_id: candidate.id,
        },
        steps: [
          workflowStep("status", "Status", "nominated", "overview", "candidate"),
          workflowStep("attention", "Attention", "1 attention", "attention", "actionable"),
          workflowStep("evidence", "Evidence", "evidence ready", "evidence", "ready"),
          workflowStep("thesis", "Thesis", "nominated", "thesis", "muted"),
          workflowStep("decision", "Decision", "no decision", "tracking", "muted"),
        ],
        attention: items,
        review_packet_attention_id: candidate.id,
        updated_at: "2026-06-01T00:00:00Z",
      };
    }
    if (symbol === "SNDK") {
      return {
        symbol,
        state: "pool_candidate",
        state_label: "Pool candidate",
        tone: "candidate",
        reason: "This symbol is in the discovery pool but not the active Universe.",
        primary_action: {
          kind: "promote",
          label: "Promote to Universe",
          detail: "Add this symbol to the monitored Universe before scheduled cognition runs.",
        },
        steps: [
          workflowStep("status", "Status", "Discovery Pool", "overview", "candidate"),
          workflowStep("attention", "Attention", "no attention", "attention"),
          workflowStep("evidence", "Evidence", "evidence ready", "evidence", "ready"),
          workflowStep("thesis", "Thesis", "no thesis", "thesis"),
          workflowStep("decision", "Decision", "no decision", "tracking"),
        ],
        attention: [],
        review_packet_attention_id: null,
        updated_at: "2026-06-01T00:00:00Z",
      };
    }
    if (symbol === "OKTA") {
      return {
        symbol,
        state: "position_tracking",
        state_label: "Position tracking",
        tone: "tracking",
        reason: "A position is open; conditions and exits matter now.",
        primary_action: {
          kind: "tracking",
          label: "Track position",
          detail: "Open the decision and position history for this symbol.",
        },
        steps: [
          workflowStep("status", "Status", "Universe T2", "overview", "tracking"),
          workflowStep("attention", "Attention", "1 attention", "attention", "actionable"),
          workflowStep("evidence", "Evidence", "1 open evidence", "evidence", "monitoring"),
          workflowStep("thesis", "Thesis", "forming · bull", "thesis", "monitoring"),
          workflowStep("decision", "Decision", "1 open position", "tracking", "tracking"),
        ],
        attention: [{
          id: 7601,
          kind: "thesis_review",
          title: "OKTA thesis changed",
          reason: "Fresh evidence changed the thesis.",
          severity: "review",
          fsm_state: "ready_for_review",
          owner: "operator",
          created_at: "2026-06-01T00:00:00Z",
        }],
        review_packet_attention_id: 7601,
        updated_at: "2026-06-01T00:00:00Z",
      };
    }
    if (symbol === "MSFT") {
      return {
        symbol,
        state: "declined",
        state_label: "Declined thesis",
        tone: "declined",
        reason: "No source-backed edge yet.",
        primary_action: {
          kind: "thesis",
          label: "Review decline",
          detail: "Open thesis attempts and review why cognition declined.",
        },
        steps: [
          workflowStep("status", "Status", "Universe T1", "overview", "declined"),
          workflowStep("attention", "Attention", "no attention", "attention"),
          workflowStep("evidence", "Evidence", "1 open evidence", "evidence", "monitoring"),
          workflowStep("thesis", "Thesis", "declined attempt", "thesis", "declined"),
          workflowStep("decision", "Decision", "no decision", "tracking"),
        ],
        attention: [],
        review_packet_attention_id: null,
        updated_at: "2026-06-01T00:00:00Z",
      };
    }
    return {
      symbol,
      state: "context_ready",
      state_label: "Context ready",
      tone: "ready",
      reason: "Context exists; cognition should draft or decline a thesis.",
      primary_action: {
        kind: "overview",
        label: "Check cognition",
        detail: "Review the latest context, evidence, and cognition status.",
      },
      steps: [
        workflowStep("status", "Status", "Universe T1", "overview", "ready"),
        workflowStep("attention", "Attention", `${items.length} attention`, "attention", items.length ? "actionable" : "muted"),
        workflowStep("evidence", "Evidence", "evidence ready", "evidence", "ready"),
        workflowStep("thesis", "Thesis", "no thesis", "thesis"),
        workflowStep("decision", "Decision", "no decision", "tracking"),
      ],
      attention: items,
      review_packet_attention_id: null,
      updated_at: "2026-06-01T00:00:00Z",
    };
  };
  const reviewPacketFor = (id: number) => {
    const item = attentionItems.find((candidate) => candidate.id === id)
      ?? (id === 7601 ? {
        id,
        kind: "thesis_review",
        symbol: "OKTA",
        thesis_id: "12ceaea3-9df3-416a-bfe5-107d3233dd59",
        candidate_id: null,
        severity: "review",
        status: "open",
        fsm_state: "ready_for_review",
        owner: "operator",
        title: "OKTA thesis changed",
        reason: "Fresh evidence changed the thesis.",
        source: "thesis",
        source_ref: {},
        created_at: "2026-06-01T00:00:00Z",
        resolved_at: null,
        resolution_kind: null,
        next_retry_at: null,
        resurface_at: null,
        state_reason: "thesis_review",
      } : id === 9102 ? {
        id,
        kind: "thesis_actionable",
        symbol: "CRDO",
        thesis_id: "22ceaea3-9df3-416a-bfe5-107d3233dd59",
        candidate_id: null,
        severity: "decision",
        status: "open",
        fsm_state: "ready_for_review",
        owner: "operator",
        title: "CRDO thesis actionable",
        reason: "Actionable up thesis with constructive setup.",
        source: "thesis",
        source_ref: {},
        created_at: "2026-06-01T00:00:00Z",
        resolved_at: null,
        resolution_kind: null,
        next_retry_at: null,
        resurface_at: null,
        state_reason: "thesis_actionable",
      } : null);
    if (!item) return null;
    const symbol = typeof item.symbol === "string" ? item.symbol : null;
    const candidateId = typeof item.candidate_id === "number" ? item.candidate_id : null;
    const isCandidate = item.kind === "candidate_review";
    const isActionable = item.kind === "thesis_actionable";
    const isThesisReview = item.kind === "thesis_review";
    const recordDecision = { id: "record_decision", label: "Record decision", kind: "decision", detail: "Open the thesis decision form." };
    const skipDecision = { id: "skip", label: "Skip / defer thesis", kind: "decision_skip", detail: "Record why this thesis is not being acted on now." };
    const deferAction = { id: "defer", label: "Defer", kind: "attention_defer", detail: "Resurface later." };
    const dismissAction = { id: "dismiss", label: "Dismiss", kind: "attention_dismiss", detail: "Mark as not actionable." };
    const primary = isCandidate
      ? { id: "promote", label: "Start research", kind: "candidate_confirm", detail: "Add to Universe and start context, evidence, and thesis work." }
      : isActionable
        ? recordDecision
      : { id: "open_symbol", label: "Open symbol", kind: "open_symbol", detail: "Inspect context and evidence." };
    const secondaryActions = isCandidate
      ? [deferAction, dismissAction]
      : isThesisReview
        ? [recordDecision, skipDecision, deferAction, dismissAction]
        : isActionable
          ? [deferAction, skipDecision, dismissAction]
          : [deferAction, dismissAction];
    return {
      attention: item,
      decision: {
        intent: isCandidate ? "promote_to_universe" : (isActionable ? "record_trade_decision" : "inspect_symbol"),
        headline: isCandidate ? `Start research for ${symbol}?` : (isActionable ? `Record a decision on ${symbol}` : `${symbol ?? "Symbol"} review`),
        primary_action: primary,
        secondary_actions: secondaryActions,
        blockers: [],
        consequences: isCandidate ? ["symbol promoted into Universe", "context and thesis work starts"] : [],
      },
      universe_status: {
        in_universe: ["MSFT", "NVDA", "OKTA"].includes(symbol ?? ""),
        tier: symbol === "OKTA" ? 2 : 1,
        added_at: "2026-01-01T00:00:00Z",
        open_theses: symbol === "OKTA" ? 1 : 0,
      },
      candidate: isCandidate && candidateId ? {
        id: candidateId,
        symbol,
        signal_name: "research_nomination",
        proposed_tier: 2,
        reasoning: String(item.reason ?? ""),
        proposed_at: "2026-06-01T00:00:00Z",
        proposed_lists: [],
        rank_score: 86,
        rank_bucket: "high",
        rank_reasons: ["AI infrastructure fit"],
      } : null,
      sections: [
        { key: "what_happened", title: "What happened", body: String(item.reason ?? item.title) },
        {
          key: "why_it_matters",
          title: "Why it matters",
          body: isCandidate
            ? "The symbol can enter the monitored Universe and start cognition work."
            : "The selected thesis needs operator review before the next action is recorded.",
        },
        { key: "evidence", title: "Evidence", items: ["Mock source-backed review packet fixture."] },
        {
          key: "recommendation",
          title: "Recommendation",
          body: isCandidate ? "Start research or reject the nomination." : "Review the thesis and record the operator decision.",
        },
        {
          key: "recorded_artifacts",
          title: "What will be recorded",
          items: ["attention state history", "operator action, deferral, or dismissal"],
        },
      ],
      allowed_actions: [
        primary,
        ...secondaryActions,
      ],
    };
  };
  const watchlistMembers: MockWatchlistMember[] = [{
    watchlist_id: "wl-core",
    symbol: "OKTA",
    added_at: "2026-01-01T00:00:00Z",
    added_by: "seed",
    latest_thesis_id: "12ceaea3-9df3-416a-bfe5-107d3233dd59",
    thesis_state: "forming",
    thesis_direction: "up",
    technical_state: "extended",
    entry_stance: "avoid_chase",
    technical_pct_vs_200d: 26.5,
    open_theses: 1,
    freshness_status: "stale",
    open_attention: 1,
    attention_states: [{ state: "ready_for_review", count: 1 }],
    attention_owners: [{ owner: "operator", count: 1 }],
    open_evidence: 1,
    blocking_evidence: 0,
    due_source_tasks: 1,
    parent_themes: [{
      key: "ai_compute_infrastructure",
      name: "AI Compute Infrastructure",
      scope: "theme",
      state: "forming",
      direction: "mixed",
      role: "candidate",
      conviction: 50,
    }],
  }];
  let nextPriceAlertId = 3003;
  let priceAlertRules = [
    {
      id: 3001,
      symbol: "MSFT",
      thesis_id: null,
      origin: "ai",
      intent: "entry",
      direction: "above",
      target_price: 208.5,
      label: "MSFT reclaim entry",
      rationale: "AI timing rule from constructive cross-technical state.",
      semantic_key: "msft_reclaim",
      status: "active",
      source_ref: {},
      expires_at: null,
      created_at: "2026-06-01T00:00:00Z",
      updated_at: "2026-06-01T00:00:00Z",
      triggered_at: null,
      disabled_at: null,
    },
    {
      id: 3002,
      symbol: "OKTA",
      thesis_id: "12ceaea3-9df3-416a-bfe5-107d3233dd59",
      origin: "manual",
      intent: "invalidation",
      direction: "below",
      target_price: 90,
      label: "OKTA invalidation",
      rationale: null,
      semantic_key: null,
      status: "triggered",
      source_ref: {},
      expires_at: null,
      created_at: "2026-06-01T00:00:00Z",
      updated_at: "2026-06-01T00:15:00Z",
      triggered_at: "2026-06-01T00:15:00Z",
      disabled_at: null,
    },
  ];
  const priceAlertEvents = [{
    id: 3101,
    rule_id: 3002,
    symbol: "OKTA",
    thesis_id: "12ceaea3-9df3-416a-bfe5-107d3233dd59",
    triggered_at: "2026-06-01T00:15:00Z",
    trigger_ts: "2026-06-01T00:15:00Z",
    trigger_interval: "1D",
    trigger_price: 89.5,
    rule_snapshot: priceAlertRules[1],
  }];
  const automationStatus = {
    as_of: "2026-06-01T12:00:00Z",
    kill_switch: {
      enabled: false,
      read_only: true,
      source: "not_configured",
      reason: "Automation mutation endpoints are not wired yet.",
    },
    summary: {
      permissions_total: 1,
      approved: 1,
      pending: 0,
      frozen: 1,
      expired: 0,
      paper_only: 1,
      live_capable: 0,
      incidents_open: 1,
      blocked_strategies: 1,
      readiness_ready: 0,
      readiness_blocked: 1,
      readiness_missing: 0,
    },
    permissions: [{
      permission_id: "39ad2b74-75e0-4a68-9e09-6e2b2f9b7f13",
      symbol: "OKTA",
      strategy_id: "technical_breakout",
      strategy_version: "v1",
      strategy_display_name: "Technical Breakout",
      strategy_family: "technical_timing",
      strategy_status: "paper",
      permission_status: "approved",
      derived_status: "frozen",
      instrument_scope: "equity_long_short",
      environment_scope: "paper",
      manual_freeze: true,
      freeze_reason: "operator reviewing first paper fills",
      approved_by: "operator",
      approved_at: "2026-06-01T08:00:00Z",
      expires_at: "2026-09-01T08:00:00Z",
      max_allocation_pct: 0.04,
      max_notional_usd: 5000,
      max_quantity: null,
      created_at: "2026-06-01T07:30:00Z",
      updated_at: "2026-06-01T09:00:00Z",
      sleeve: {
        sleeve_id: "cb8c78cb-3dac-43ee-abaf-062f049d61bd",
        sleeve_kind: "strategy",
        status: "frozen",
        current_side: "flat",
        current_quantity: 0,
        current_notional_usd: 0,
        allocated_notional_usd: 5000,
        realized_pnl: 0,
        updated_at: "2026-06-01T09:00:00Z",
      },
      desired_position: {
        desired_position_id: "e965661b-7d72-4914-b315-27d7bd5a202c",
        target_side: "long",
        target_quantity: 20,
        target_notional_usd: 1900,
        target_weight_pct: 0.02,
        rationale: "Breakout retest held above rising 50-day.",
        reason_codes: ["breakout_retest"],
        emitted_at: "2026-06-01T09:10:00Z",
      },
      latest_proof: {
        proof_id: "be432d6f-b7db-454a-a6f1-5154f1a3b7b2",
        result: "blocked",
        blocked_reasons: ["permission frozen", "market data stale"],
        evaluated_at: "2026-06-01T09:11:00Z",
      },
      readiness: {
        evaluation_id: "f0f4795d-3125-4c15-9ac2-cd52de16e0a0",
        lifecycle_stage: "paper",
        target_stage: "canary_live",
        status: "blocked",
        readiness_score: 0.42,
        approval_required: true,
        approval_valid: false,
        freeze_live_permissions: false,
        metrics: {
          observations_total: 12,
          outcomes_scored: 8,
          directional_outcomes_scored: 8,
          signal_quality_rate: 0.5,
          mean_forward_return_pct: 1.2,
          mean_max_drawdown_pct: -4.3,
          churn_rate: 0.2,
          proof_pass_rate: 0.72,
          incident_rate: 0.08,
          open_critical_incidents: 0,
          paper_orders_total: 3,
          paper_fill_quality_rate: 0.67,
          baseline_excess_return_pct: 0.5,
        },
        blockers: ["approval_missing", "insufficient_paper_orders", "proof_pass_rate_below_threshold"],
        warnings: [],
        lookback_days: 90,
        evaluated_at: "2026-06-01T09:15:00Z",
        approval: null,
      },
      reconciliation: {
        reconciliation_id: "7b8e42b8-33b0-49e2-bf59-970be41750db",
        status: "blocked",
        blocked_reasons: ["permission frozen"],
        updated_at: "2026-06-01T09:12:00Z",
      },
      broker_position: {
        open_positions: 1,
        broker_positions: 1,
        net_quantity: 12,
        delta_notional: 1056,
        premium_at_risk: 0,
        latest_sync_at: "2026-06-01T09:00:00Z",
      },
      incidents: [{
        incident_id: "f453d51a-fb4f-4cb4-bb7e-6254da39c733",
        severity: "warning",
        status: "open",
        kind: "stale_market_data",
        title: "Market data stale",
        created_at: "2026-06-01T09:12:00Z",
      }],
    }],
  };

  await page.route("**/api/**", async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const path = url.pathname;

    if (path === "/api/stream") {
      const events = [
        { subject: "stream.connected", kind: "stream", payload: { status: "open" } },
        ...streamEvents,
      ];
      await route.fulfill({
        status: 200,
        contentType: "text/event-stream",
        body: events.map((event) => `data: ${JSON.stringify(event)}\n\n`).join(""),
      });
      return;
    }
    if (path === "/api/alerts") {
      await json(route, [
        {
          id: 1001,
          thesis_id: null,
          symbol: null,
          kind: "risk",
          payload: { reasons: ["global portfolio drawdown warning"] },
          acknowledged: false,
          created_at: "2026-06-01T00:00:00Z",
        },
        {
          id: 1002,
          thesis_id: null,
          symbol: "OKTA",
          kind: "state_transition",
          payload: { reasons: ["OKTA thesis moved to forming"] },
          acknowledged: false,
          created_at: "2026-06-01T00:01:00Z",
        },
      ]);
      return;
    }
    if (path === "/api/price-alerts" && request.method() === "GET") {
      const symbol = url.searchParams.get("symbol");
      const status = url.searchParams.get("status");
      await json(route, priceAlertRules.filter((rule) =>
        (!symbol || rule.symbol === symbol.toUpperCase())
        && (!status || rule.status === status)
      ));
      return;
    }
    if (path === "/api/price-alerts" && request.method() === "POST") {
      const body = await request.postDataJSON();
      calls.priceAlertBodies.push(body);
      const rule = {
        id: nextPriceAlertId++,
        symbol: String(body.symbol ?? "").toUpperCase(),
        thesis_id: body.thesis_id ?? null,
        origin: body.origin ?? "manual",
        intent: body.intent ?? "watch",
        direction: body.direction,
        target_price: body.target_price,
        label: body.label,
        rationale: body.rationale ?? null,
        semantic_key: body.semantic_key ?? null,
        status: "active",
        source_ref: body.source_ref ?? {},
        expires_at: body.expires_at ?? null,
        created_at: "2026-06-01T00:30:00Z",
        updated_at: "2026-06-01T00:30:00Z",
        triggered_at: null,
        disabled_at: null,
      };
      priceAlertRules = [rule, ...priceAlertRules];
      await json(route, rule, 201);
      return;
    }
    if (/^\/api\/price-alerts\/\d+\/disable$/.test(path) && request.method() === "POST") {
      const id = Number(path.split("/")[3]);
      const rule = priceAlertRules.find((row) => row.id === id);
      if (!rule) {
        await route.fulfill({ status: 404, body: "price alert not found" });
        return;
      }
      rule.status = "disabled";
      rule.disabled_at = "2026-06-01T00:35:00Z";
      rule.updated_at = "2026-06-01T00:35:00Z";
      await json(route, rule);
      return;
    }
    if (path === "/api/price-alert-events") {
      const symbol = url.searchParams.get("symbol");
      await json(route, priceAlertEvents.filter((event) => !symbol || event.symbol === symbol.toUpperCase()));
      return;
    }
    if (path === "/api/automation/status") {
      const symbol = url.searchParams.get("symbol");
      await json(route, symbol
        ? { ...automationStatus, permissions: automationStatus.permissions.filter((p) => p.symbol === symbol.toUpperCase()) }
        : automationStatus);
      return;
    }
    if (path === "/api/regime") {
      await json(route, { regime: "neutral", capitulation: false, indicators: {}, as_of: "2026-06-01T00:00:00Z" });
      return;
    }
    if (path === "/api/tickers" && request.method() === "POST") {
      calls.promoteBody = await request.postDataJSON();
      await route.fulfill({ status: 204 });
      return;
    }
    if (path === "/api/tickers") {
      await json(route, [
        { symbol: "MSFT", cluster_id: "ai", cluster_name: "AI infrastructure", tier: 1, options_eligible: true, domain_fit: 91, added_at: "2026-01-01T00:00:00Z", open_theses: 0, latest_thesis_id: null, thesis_state: null, thesis_direction: null, technical_state: "constructive", entry_stance: "constructive", technical_pct_vs_200d: 4.2, freshness_status: "missing", open_attention: 0, attention_states: [], attention_owners: [], open_evidence: 2, blocking_evidence: 0, due_source_tasks: 1, parent_themes: [] },
        { symbol: "OKTA", cluster_id: "identity", cluster_name: "Identity", tier: 2, options_eligible: true, domain_fit: 77, added_at: "2026-01-01T00:00:00Z", open_theses: 1, latest_thesis_id: "12ceaea3-9df3-416a-bfe5-107d3233dd59", thesis_state: "forming", thesis_direction: "up", technical_state: "extended", entry_stance: "avoid_chase", technical_pct_vs_200d: 26.5, freshness_status: "stale", open_attention: 1, attention_states: [{ state: "ready_for_review", count: 1 }], attention_owners: [{ owner: "operator", count: 1 }], open_evidence: 1, blocking_evidence: 0, due_source_tasks: 1, parent_themes: [{ key: "ai_compute_infrastructure", name: "AI Compute Infrastructure", scope: "theme", state: "forming", direction: "mixed", role: "candidate", conviction: 50 }] },
        { symbol: "NVDA", cluster_id: "ai", cluster_name: "AI infrastructure", tier: 1, options_eligible: true, domain_fit: 96, added_at: "2026-01-01T00:00:00Z", open_theses: 0, latest_thesis_id: null, thesis_state: null, thesis_direction: null, technical_state: "base_building", entry_stance: "wait_breakout", technical_pct_vs_200d: -1.2, freshness_status: "fresh", open_attention: 0, attention_states: [], attention_owners: [], open_evidence: 0, blocking_evidence: 0, due_source_tasks: 0, parent_themes: [{ key: "ai_compute_infrastructure", name: "AI Compute Infrastructure", scope: "theme", state: "forming", direction: "mixed", role: "leader", conviction: 70 }] },
      ]);
      return;
    }
    if (path === "/api/calibration") {
      await json(route, {
        predictions_total: 3,
        outcomes_scored: 1,
        mean_brier: 0.21,
        mean_lead_time_days: 8.5,
        median_lead_time_days: 8.5,
        parent_themes: [{
          key: "ai_compute_infrastructure",
          name: "AI Compute Infrastructure",
          scope: "theme",
          role: "supplier",
          predictions_total: 2,
          outcomes_scored: 1,
          mean_brier: 0.18,
          mean_lead_time_days: 10.0,
        }],
        technical_timing: [{
          technical_state: "pullback_watch",
          setup_kind: "pullback_watch",
          entry_stance: "wait_reversal",
          benchmark_symbol: "QQQ",
          observations_total: 4,
          outcomes_scored: 3,
          mean_forward_return_pct: 6.2,
          mean_max_drawdown_pct: -3.1,
          mean_benchmark_return_pct: 2.0,
          mean_excess_return_pct: 4.2,
          positive_return_rate: 0.67,
          outperform_rate: 0.67,
        }],
      });
      return;
    }
    if (path === "/api/brain") {
      await json(route, {
        as_of: "2026-06-01T00:00:00Z",
        market_state: {
          regime: "neutral",
          capitulation: false,
          indicators: {},
          as_of: "2026-06-01T00:00:00Z",
        },
        macro: {
          id: "d29d2f1d-7467-45ca-9f1e-1243923c94aa",
          scope: "macro",
          key: "macro_regime",
          name: "Macro Regime",
          state: "active",
          direction: "neutral",
          summary: "Macro posture is neutral until breadth and rates confirm a stronger view.",
          core_claim: "Ticker conviction should respect the top-down risk regime.",
          why_now: null,
          evidence: [{
            generated_by: "brain_maintainer",
            kind: "macro_source_freshness",
            as_of: "2026-06-01T00:00:00Z",
            market_state: {
              regime: "neutral",
              indicators: {
                market_breadth_internals: {
                  symbol_count: 1147,
                  advancers: 484,
                  decliners: 650,
                  pct_above_200d: 0.5597,
                },
                earnings_breadth: {
                  signals: 6527,
                  symbol_count: 655,
                  net_revision_breadth: 0.0032,
                },
                sector_relative_strength: {
                  leaders_20d: ["Technology", "Healthcare", "Industrials"],
                },
                credit_internals_trend: {
                  latest_hy_oas_pct: 2.72,
                  trend: "stable",
                },
              },
            },
          }],
          invalidation_conditions: [],
          beneficiaries: [],
          losers: [],
          open_questions: ["Refresh FRED macro series"],
          missing_evidence: [],
          source_ref: {
            maintainer: {
              sources: {
                fred: { source: "fred", freshness: "fresh", status: "no_new_rows" },
                cboe: { source: "cboe", freshness: "fresh", status: "no_new_rows" },
              },
              market_state: {
                regime: "neutral",
                indicators: {
                  market_breadth_internals: {
                    symbol_count: 1147,
                    advancers: 484,
                    decliners: 650,
                    pct_above_200d: 0.5597,
                  },
                  earnings_breadth: {
                    signals: 6527,
                    symbol_count: 655,
                    net_revision_breadth: 0.0032,
                  },
                  sector_relative_strength: {
                    leaders_20d: ["Technology", "Healthcare", "Industrials"],
                  },
                  credit_internals_trend: {
                    latest_hy_oas_pct: 2.72,
                    trend: "stable",
                  },
                },
              },
              dislocation_map: {
                buckets: {
                  loved_mania: [{
                    name: "Technology",
                    score: 74,
                    interpretation: "Loved/mania: strong attention or momentum can make true stories poor entries.",
                    reasons: ["top-quartile 20d sector relative strength", "news attention is elevated"],
                  }],
                  ignored_indifference: [{
                    name: "Industrials",
                    score: 56,
                    interpretation: "Ignored/indifference: improving evidence is not yet receiving much attention.",
                    reasons: ["estimate revision breadth is improving", "news attention is low"],
                  }],
                  hated_avoided: [{
                    name: "Financial Services",
                    score: 49,
                    interpretation: "Hated/avoided: weak sentiment or price action may be masking an improving setup.",
                    reasons: ["news tone is negative", "evidence is less bad than price/sentiment"],
                  }],
                },
              },
            },
          },
          freshness_target_minutes: 720,
          last_evaluated_at: null,
          version: 1,
          created_at: "2026-06-01T00:00:00Z",
          updated_at: "2026-06-01T00:00:00Z",
          freshness: "fresh",
          tickers: [],
          watchlists: [],
          nominations: [],
          latest_changes: [],
        },
        sectors: [{
          id: "b5e8dffa-0af8-4247-a6f3-100c668545d8",
          scope: "theme",
          key: "ai_compute_infrastructure",
          name: "AI Compute Infrastructure",
          state: "forming",
          direction: "mixed",
          summary: "AI capex remains the parent theme, but symbol selection must separate leaders from challengers.",
          core_claim: "The edge is finding where adoption evidence diffuses slower than price consensus.",
          why_now: "Product/customer adoption evidence is still arriving.",
          evidence: [],
          invalidation_conditions: [],
          beneficiaries: ["NVDA", "AMD", "MU"],
          losers: [],
          open_questions: ["Which challengers have real customer traction?"],
          missing_evidence: ["theme_estimate_revision_breadth"],
          source_ref: {
            maintainer: {
              coverage: {
                linked: 2,
                contexts: 1,
                open_theses: 1,
                news: 2,
                estimates: 2,
                analyst_opinion: 1,
              },
            },
          },
          freshness_target_minutes: 720,
          last_evaluated_at: null,
          version: 1,
          created_at: "2026-06-01T00:00:00Z",
          updated_at: "2026-06-01T00:00:00Z",
          freshness: "missing",
          tickers: [
            { symbol: "NVDA", role: "leader", rationale: "Accelerator platform leader.", conviction: 70, thesis_state: null, thesis_direction: null, open_theses: 0 },
            { symbol: "OKTA", role: "candidate", rationale: "Mock linked row.", conviction: 50, thesis_state: "forming", thesis_direction: "up", open_theses: 1 },
          ],
          watchlists: [{ id: "wl-core", name: "Core", color: "#89b4fa", is_system: false }],
          nominations: [{ candidate_id: 44, symbol: "NVDA", signal_name: "volume_anomaly", signal_value: 2.4, reasoning: "2.4x volume", proposed_at: "2026-06-01T00:00:00Z" }],
          latest_changes: [],
        }],
        contradictions: [],
        summary: { active_theses: 2, stale_or_missing: 2, open_nominations: 1 },
      });
      return;
    }
    if (path === "/api/brain-journal") {
      await json(route, {
        as_of: "2026-06-01T12:00:00Z",
        date: "2026-06-01",
        synthesis: null,
        overview: {
          as_of: "2026-06-01T12:00:00Z",
          headline: "1 clean candidate(s), 1 setup wait(s), 1 changed item(s), 1 blocker(s).",
          market: {
            label: "market neutral · macro neutral",
            regime: "neutral",
            macro_direction: "neutral",
            state: "forming",
            freshness: "missing",
            summary: "Macro posture is neutral until the regime loop promotes a stronger rates/liquidity/growth view.",
            missing_evidence: ["fred_macro", "market_breadth"],
            market_state: { regime: "neutral", capitulation: false },
          },
          top_candidates: [{
            symbol: "CRDO",
            score: 76,
            thesis_id: "22ceaea3-9df3-416a-bfe5-107d3233dd59",
            thesis_state: "actionable",
            thesis_direction: "up",
            technical_state: "constructive",
            technical_setup_kind: "constructive_trend",
            entry_stance: "constructive",
            technical_read: constructiveRead,
            technical_panel_path: "/symbol/CRDO?p=technical",
            technical_pct_vs_200d: 3.2,
            freshness_status: "fresh",
            open_attention: 1,
            review_packet_attention_id: 9102,
            open_evidence: 3,
            blocking_evidence: 0,
            due_source_tasks: 0,
            parent_themes: [],
            reason: "actionable thesis, up direction, constructive technicals, constructive entry stance, fresh freshness",
          }],
          wait_for_setup: [{
            symbol: "OKTA",
            score: 54,
            thesis_id: "12ceaea3-9df3-416a-bfe5-107d3233dd59",
            thesis_state: "forming",
            thesis_direction: "up",
            technical_state: "extended",
            technical_setup_kind: "extended_run",
            entry_stance: "avoid_chase",
            technical_read: avoidChaseRead,
            technical_panel_path: "/symbol/OKTA?p=technical",
            technical_pct_vs_200d: 26.5,
            freshness_status: "stale",
            open_attention: 1,
            open_evidence: 1,
            blocking_evidence: 0,
            due_source_tasks: 1,
            parent_themes: [],
            reason: "forming thesis, up direction, extended technicals, avoid_chase entry stance, stale freshness",
          }],
          decision_brief: {
            consider: [{
              symbol: "CRDO",
              score: 76,
              stance: "consider",
              thesis_id: "22ceaea3-9df3-416a-bfe5-107d3233dd59",
              thesis_state: "actionable",
              thesis_direction: "up",
              technical_state: "constructive",
              technical_setup_kind: "constructive_trend",
              entry_stance: "constructive",
              technical_read: constructiveRead,
              technical_panel_path: "/symbol/CRDO?p=technical",
              technical_pct_vs_200d: 3.2,
              freshness_status: "fresh",
              open_attention: 1,
              review_packet_attention_id: 9102,
              open_evidence: 3,
              blocking_evidence: 0,
              due_source_tasks: 0,
              parent_themes: [],
              why_now: "Actionable up thesis with constructive setup.",
              why_not: "Still needs operator review before any order.",
              risk_note: "Normal position sizing only.",
              blockers: [],
            }],
            wait: [{
              symbol: "OKTA",
              score: 54,
              stance: "wait",
              thesis_id: "12ceaea3-9df3-416a-bfe5-107d3233dd59",
              thesis_state: "forming",
              thesis_direction: "up",
              technical_state: "extended",
              technical_setup_kind: "extended_run",
              entry_stance: "avoid_chase",
              technical_read: avoidChaseRead,
              technical_panel_path: "/symbol/OKTA?p=technical",
              technical_pct_vs_200d: 26.5,
              freshness_status: "stale",
              open_attention: 1,
              review_packet_attention_id: 7601,
              open_evidence: 1,
              blocking_evidence: 0,
              due_source_tasks: 1,
              parent_themes: [],
              why_now: "Bullish thesis exists, but setup is stretched.",
              why_not: "Wait for pullback or refreshed context.",
              risk_note: "Avoid chasing a stale setup.",
              blockers: ["stale context", "due source tasks"],
            }],
            avoid: [{
              symbol: "AMD",
              score: 31,
              stance: "avoid",
              thesis_id: "32ceaea3-9df3-416a-bfe5-107d3233dd59",
              thesis_state: "forming",
              thesis_direction: "up",
              technical_state: "deteriorating",
              technical_setup_kind: "breakdown",
              entry_stance: "avoid",
              technical_read: avoidBreakdownRead,
              technical_panel_path: "/symbol/AMD?p=technical",
              technical_pct_vs_200d: -8.5,
              freshness_status: "blocked",
              open_attention: 2,
              review_packet_attention_id: null,
              open_evidence: 1,
              blocking_evidence: 2,
              due_source_tasks: 3,
              parent_themes: [],
              why_now: "Interesting AI infrastructure exposure.",
              why_not: "Blocked data and breakdown technicals make it a no-trade today.",
              risk_note: "Do not convert a stale thesis into action.",
              blockers: ["blocked analyst estimates", "extended vs 200D"],
            }],
            research: [{
              symbol: "NVDA",
              score: 45,
              stance: "research",
              thesis_id: null,
              thesis_state: null,
              thesis_direction: null,
              technical_state: "constructive",
              technical_setup_kind: "constructive_trend",
              entry_stance: "constructive",
              technical_read: constructiveRead,
              technical_panel_path: "/symbol/NVDA?p=technical",
              technical_pct_vs_200d: 8.4,
              freshness_status: "missing",
              open_attention: 1,
              review_packet_attention_id: 7001,
              open_evidence: 0,
              blocking_evidence: 0,
              due_source_tasks: 1,
              parent_themes: [],
              why_now: "Discovery nominated the symbol.",
              why_not: "No open thesis yet.",
              risk_note: "Research only until a falsifiable thesis exists.",
              blockers: ["missing thesis"],
            }],
          },
          risk_flags: [],
          themes: [{
            name: "AI Compute Infrastructure",
            scope: "theme",
            state: "forming",
            direction: "mixed",
            summary: "AI capex remains the parent theme, but symbol quality differs.",
            missing_evidence: ["customer_adoption_research"],
            freshness: "missing",
            linked_tickers: 2,
          }],
          news_recap: [{
            symbol: "CRDO",
            kind: "news",
            summary: "Credo highlighted new AI-cluster design win momentum.",
            source: "fmp_news",
            url: null,
            strength: 0.8,
            polarity: 0.5,
            observed_at: "2026-06-01T09:00:00Z",
          }],
          research_focus: [{
            category: "blocked",
            source_kind: "source_task",
            source_id: "9101",
            symbol: "MSFT",
            title: "Data blocked: MSFT analyst estimates",
            summary: "fmp task rate_limited with high priority after 2 attempt(s).",
            importance: 78,
            occurred_at: "2026-06-01T08:00:00Z",
          }],
          counts: {
            active_universe: 67,
            top_candidates: 1,
            wait_for_setup: 1,
            risk_flags: 0,
            news_recap: 1,
            research_focus: 1,
            changed: 1,
            blocked: 1,
          },
        },
        summary: {
          total: 5,
          visible: 5,
          by_category: {
            changed: 1,
            research: 1,
            blocked: 1,
            crowded_or_extended: 1,
            ignored_or_hated: 1,
          },
          all_by_category: {
            changed: 1,
            research: 1,
            blocked: 1,
            crowded_or_extended: 1,
            ignored_or_hated: 1,
          },
        },
        pagination: {
          page: Number(url.searchParams.get("page") ?? "1"),
          per_page: Number(url.searchParams.get("per_page") ?? "50"),
          total: 5,
          total_pages: 1,
          has_previous: false,
          has_next: false,
        },
        entries: [
          {
            id: 1,
            date: "2026-06-01",
            category: "changed",
            source_kind: "thesis_version",
            source_id: "201",
            event_key: "thesis_version:201",
            symbol: "OKTA",
            brain_thesis_id: null,
            thesis_id: "12ceaea3-9df3-416a-bfe5-107d3233dd59",
            title: "OKTA thesis updated to v2",
            summary: "Estimate revisions and customer evidence changed the identity thesis.",
            importance: 88,
            occurred_at: "2026-06-01T10:00:00Z",
            source_ref: { table: "thesis_version_history", id: 201 },
            created_at: "2026-06-01T10:01:00Z",
          },
          {
            id: 2,
            date: "2026-06-01",
            category: "research",
            source_kind: "attention",
            source_id: "7001",
            event_key: "attention:7001",
            symbol: "NVDA",
            brain_thesis_id: null,
            thesis_id: null,
            title: "Research queued: NVDA via volume anomaly",
            summary: "2.4x volume vs 20-day average while price is above the 200-day SMA.",
            importance: 70,
            occurred_at: "2026-06-01T09:30:00Z",
            source_ref: { attention_id: 7001 },
            created_at: "2026-06-01T09:31:00Z",
          },
          {
            id: 3,
            date: "2026-06-01",
            category: "blocked",
            source_kind: "source_task",
            source_id: "9101",
            event_key: "source_task:9101",
            symbol: "MSFT",
            brain_thesis_id: null,
            thesis_id: null,
            title: "Data blocked: MSFT analyst estimates",
            summary: "fmp task rate_limited with high priority after 2 attempt(s).",
            importance: 78,
            occurred_at: "2026-06-01T08:00:00Z",
            source_ref: { source_task_id: 9101 },
            created_at: "2026-06-01T08:01:00Z",
          },
          {
            id: 4,
            date: "2026-06-01",
            category: "crowded_or_extended",
            source_kind: "brain_thesis",
            source_id: "macro:loved_mania",
            event_key: "brain_dislocation:loved_mania",
            symbol: null,
            brain_thesis_id: "d29d2f1d-7467-45ca-9f1e-1243923c94aa",
            thesis_id: null,
            title: "Loved / mania: Technology",
            summary: "Macro Regime flags this pocket: high relative strength and crowded attention.",
            importance: 78,
            occurred_at: "2026-06-01T07:00:00Z",
            source_ref: { bucket: "loved_mania" },
            created_at: "2026-06-01T07:01:00Z",
          },
          {
            id: 5,
            date: "2026-06-01",
            category: "ignored_or_hated",
            source_kind: "brain_thesis",
            source_id: "macro:hated_avoided",
            event_key: "brain_dislocation:hated_avoided",
            symbol: null,
            brain_thesis_id: "d29d2f1d-7467-45ca-9f1e-1243923c94aa",
            thesis_id: null,
            title: "Hated / avoided: Financial Services",
            summary: "Macro Regime flags this pocket: low attention despite improving internals.",
            importance: 82,
            occurred_at: "2026-06-01T07:05:00Z",
            source_ref: { bucket: "hated_avoided" },
            created_at: "2026-06-01T07:06:00Z",
          },
        ],
      });
      return;
    }
    if (path === "/api/watchlists" && request.method() === "GET") {
      await json(route, [{ id: "wl-core", name: "Core", description: null, color: "#89b4fa", is_system: false, created_at: "2026-01-01T00:00:00Z", member_count: watchlistMembers.length }]);
      return;
    }
    if (path === "/api/watchlists/wl-core/members" && request.method() === "GET") {
      await json(route, watchlistMembers);
      return;
    }
    if (path === "/api/watchlists/wl-core/members" && request.method() === "POST") {
      const body = await request.postDataJSON();
      calls.addedSymbols.push(body.symbol);
      watchlistMembers.push({
        watchlist_id: "wl-core",
        symbol: body.symbol,
        added_at: "2026-06-01T00:00:00Z",
        added_by: body.added_by ?? "user",
        latest_thesis_id: null,
        thesis_state: null,
        thesis_direction: null,
        technical_state: "unknown",
        entry_stance: "wait_data",
        technical_pct_vs_200d: null,
        open_theses: 0,
        freshness_status: "missing",
        open_attention: 0,
        attention_states: [],
        attention_owners: [],
        open_evidence: 0,
        blocking_evidence: 0,
        due_source_tasks: 0,
        parent_themes: [],
      });
      await route.fulfill({ status: 204 });
      return;
    }
    if (path === "/api/discovery/candidates") {
      await json(route, attentionOpen ? [{
        id: 44,
        symbol: "NVDA",
        signal_name: "volume_anomaly",
        signal_value: 2.4,
        reasoning: "2.4x volume vs 20-day average while price is above 200-day SMA",
        proposed_at: "2026-06-01T00:00:00Z",
        proposed_lists: [{ watchlist_id: "wl-core", watchlist_name: "Core", confidence: "high", rationale: "AI infrastructure fit" }],
        suggested_new_list: null,
        rank_score: 82,
        rank_bucket: "highest",
        rank_reasons: [
          "volume anomaly",
          "strong signal value",
          "active parent theme fit 70",
          "high-confidence watchlist fit",
        ],
        parent_theme_fit: 70,
        parent_themes: [{
          key: "ai_compute_infrastructure",
          name: "AI Compute Infrastructure",
          scope: "theme",
          role: "leader",
          conviction: 70,
          rationale: "Accelerator platform leader.",
        }],
      }] : []);
      return;
    }
    if (path === "/api/discovery-pool") {
      await json(route, [{
        symbol: "OKTA",
        company_name: "Okta, Inc.",
        sector: "Technology",
        industry: "Software - Infrastructure",
        market_cap: 23_000_000_000,
        first_seen_at: "2026-01-01T00:00:00Z",
        latest_thesis_id: "12ceaea3-9df3-416a-bfe5-107d3233dd59",
        thesis_state: "forming",
        thesis_direction: "up",
        technical_state: "extended",
        entry_stance: "avoid_chase",
        technical_pct_vs_200d: 26.5,
        open_theses: 1,
        freshness_status: "stale",
        open_attention: 1,
        attention_states: [{ state: "ready_for_review", count: 1 }],
        attention_owners: [{ owner: "operator", count: 1 }],
        open_evidence: 1,
        blocking_evidence: 0,
        due_source_tasks: 1,
        parent_themes: [{
          key: "ai_compute_infrastructure",
          name: "AI Compute Infrastructure",
          scope: "theme",
          state: "forming",
          direction: "mixed",
          role: "candidate",
          conviction: 50,
        }],
      }, {
        symbol: "SNDK",
        company_name: "Sandisk Corporation",
        sector: "Technology",
        industry: "Hardware, Equipment & Parts",
        market_cap: 260_493_572_494,
        first_seen_at: "2026-06-01T00:00:00Z",
        latest_thesis_id: null,
        thesis_state: null,
        thesis_direction: null,
        technical_state: "unknown",
        entry_stance: "wait_data",
        technical_pct_vs_200d: null,
        open_theses: 0,
        freshness_status: "missing",
        open_attention: 0,
        attention_states: [],
        attention_owners: [],
        open_evidence: 0,
        blocking_evidence: 0,
        due_source_tasks: 0,
        parent_themes: [],
      }]);
      return;
    }
    if (path === "/api/attention") {
      await json(route, attentionOpen ? attentionItems : []);
      return;
    }
    if (path === "/api/symbol-workflow") {
      await json(route, symbolWorkflow(url.searchParams.get("symbol") ?? "MSFT"));
      return;
    }
    if (/^\/api\/attention\/\d+\/review-packet$/.test(path)) {
      const id = Number(path.split("/")[3]);
      const packet = reviewPacketFor(id);
      if (!packet) {
        await route.fulfill({ status: 404, body: "attention not found" });
        return;
      }
      await json(route, packet);
      return;
    }
    if (/^\/api\/discovery\/candidates\/\d+\/confirm$/.test(path) && request.method() === "POST") {
      calls.confirmBody = await request.postDataJSON();
      attentionOpen = false;
      await route.fulfill({ status: 204 });
      return;
    }
    if (path === "/api/ticker-context") {
      const symbol = url.searchParams.get("symbol");
      if (symbol === "SNDK") {
        await route.fulfill({ status: 204 });
        return;
      }
      await json(route, {
        symbol,
        version: 2,
        structural: { company: symbol },
        structural_as_of: "2026-06-01T00:00:00Z",
        narrative: { summary: `${symbol} narrative` },
        narrative_as_of: "2026-06-01T00:00:00Z",
        market: { price_state: { close: 420.91 }, attention_reason: "Breakout with daily SMA support" },
        market_as_of: "2026-06-01T00:00:00Z",
        created_at: "2026-06-01T00:00:00Z",
      });
      return;
    }
    if (path === "/api/brain-status") {
      const symbol = url.searchParams.get("symbol") ?? "MSFT";
      if (symbol === "SNDK") {
        await json(route, {
          symbol,
          as_of: "2026-06-01T00:00:00Z",
          active_ticker: false,
          status: "not_monitored",
          next_action: "add_to_universe",
          reason: "symbol is not in the active universe, so the scheduled brain loop will not run until it is confirmed or added",
          freshness_target_minutes: 30,
          sources: [
            {
              source: "context",
              status: "missing",
              last_changed_at: null,
              last_checked_at: null,
              max_age_minutes: 720,
              version: null,
            },
            {
              source: "thesis",
              status: "missing",
              last_changed_at: null,
              last_checked_at: null,
              max_age_minutes: 30,
              state: null,
              direction: null,
            },
          ],
          evidence: { rows: 0, open: 0, blocking: 0, due: 0, items: 11, latest_item_at: "2026-06-01T00:00:00Z", delta: true },
          attention: { open: 0, by_kind: [] },
          cognition: { last_run: null, recent_runs: [] },
        });
        return;
      }
      const evidenceDriven = symbol === "CRDO";
      await json(route, {
        symbol,
        as_of: "2026-06-01T00:00:00Z",
        active_ticker: true,
        status: symbol === "OKTA" ? "fresh" : "due",
        next_action: symbol === "OKTA"
          ? "monitor"
          : evidenceDriven
            ? "reevaluate_after_evidence_update"
            : "reevaluate_thesis",
        reason: symbol === "OKTA"
          ? "brain loop is current for this symbol"
          : evidenceDriven
            ? "normalized evidence is newer than the current thesis evaluation"
          : "open thesis is past the re-evaluation window",
        freshness_target_minutes: 30,
        sources: [
          {
            source: "price",
            status: "fresh",
            last_changed_at: "2026-06-01T00:00:00Z",
            last_checked_at: "2026-06-01T00:00:00Z",
            max_age_minutes: 30,
            detail: { latest_session: "2026-05-29", expected_session: "2026-05-29" },
            source_health: { rows_seen: 260, rows_inserted: 2 },
            source_tasks: [{
              requirement_key: "price_history",
              action: "fmp_price_backfill",
              provider: "fmp",
              state: "satisfied",
              priority: "blocking",
              due_at: "2026-06-01T00:30:00Z",
              next_retry_at: null,
              attempts: 1,
              last_error: null,
              updated_at: "2026-06-01T00:00:00Z",
            }],
          },
          {
            source: "news",
            status: "fresh",
            last_changed_at: "2026-06-01T00:00:00Z",
            last_checked_at: "2026-06-01T00:00:00Z",
            max_age_minutes: 30,
            detail: { latest_published_at: "2026-06-01T00:00:00Z" },
            source_health: { rows_seen: 12, rows_inserted: 1 },
          },
          {
            source: "profile",
            status: "fresh",
            last_changed_at: "2026-06-01T00:00:00Z",
            last_checked_at: "2026-06-01T00:00:00Z",
            max_age_minutes: 30,
            detail: {
              company_profiles: 1,
              company_name: "NVIDIA Corporation",
              sector: "Technology",
              industry: "Semiconductors",
              market_cap: 5396923220000,
            },
            source_health: { rows_seen: 1, rows_inserted: 1 },
          },
          {
            source: "analyst_opinion",
            status: "fresh",
            last_changed_at: "2026-06-01T00:00:00Z",
            last_checked_at: "2026-06-01T00:00:00Z",
            max_age_minutes: 30,
            detail: { price_target_snapshots: 1, recommendation_snapshots: 1, price_target_events: 2, rating_events: 1 },
            source_health: { rows_seen: 4, rows_inserted: 3 },
            source_tasks: [{
              requirement_key: "analyst_opinion",
              action: "fmp_price_target_consensus",
              provider: "fmp",
              state: "queued",
              priority: "medium",
              due_at: "2026-06-01T00:30:00Z",
              next_retry_at: null,
              attempts: 2,
              last_error: null,
              updated_at: "2026-06-01T00:00:00Z",
            }],
          },
          {
            source: "earnings",
            status: "fresh",
            last_changed_at: "2026-06-01T00:00:00Z",
            last_checked_at: "2026-06-01T00:00:00Z",
            max_age_minutes: 30,
            detail: {
              earnings_events: 5,
              next_earnings_date: "2026-08-26",
            },
            source_health: { rows_seen: 5, rows_inserted: 5 },
          },
          {
            source: "evidence",
            status: evidenceDriven ? "fresh" : "stale",
            last_changed_at: evidenceDriven ? "2026-06-01T00:01:00Z" : "2026-05-31T23:00:00Z",
            last_checked_at: evidenceDriven ? "2026-06-01T00:01:00Z" : "2026-05-31T23:00:00Z",
            max_age_minutes: 30,
            detail: {
              normalized_items: evidenceDriven ? 8 : 4,
              evidence_delta: evidenceDriven,
              latest_item_at: evidenceDriven ? "2026-06-01T00:01:00Z" : "2026-05-31T23:00:00Z",
            },
          },
          {
            source: "thesis",
            status: symbol === "OKTA" ? "fresh" : "stale",
            last_changed_at: "2026-05-31T23:00:00Z",
            last_checked_at: "2026-05-31T23:00:00Z",
            max_age_minutes: 30,
            version: 2,
            state: "forming",
            direction: "up",
          },
        ],
        evidence: {
          rows: 4,
          open: 0,
          blocking: 0,
          due: 0,
          items: evidenceDriven ? 8 : 4,
          latest_item_at: evidenceDriven ? "2026-06-01T00:01:00Z" : "2026-05-31T23:00:00Z",
          delta: evidenceDriven,
        },
        attention: { open: symbol === "OKTA" ? 0 : 1, by_kind: [] },
        cognition: {
          last_run: symbol === "OKTA" ? {
            id: 77,
            symbol: "OKTA",
            trigger: "evidence_delta",
            sweep_reason: "evidence_item_changed",
            status: "reconciled",
            reason: "thesis reconciled: strengthened_view",
            context_version: 2,
            thesis_id: "12ceaea3-9df3-416a-bfe5-107d3233dd59",
            thesis_classification: "strengthened_view",
            evidence_open_count: 0,
            evidence_blocking_count: 0,
            started_at: "2026-06-01T00:02:00Z",
            finished_at: "2026-06-01T00:03:00Z",
            next_retry_at: null,
            error: null,
            source_ref: {
              evidence_item_at: "2026-06-01T00:01:00Z",
              sweep_reason: "evidence_item_changed",
            },
          } : null,
          recent_runs: [],
        },
      });
      return;
    }
    if (path === "/api/theses") {
      const symbol = url.searchParams.get("symbol");
      await json(route, symbol === "OKTA" ? [{
        thesis_id: "12ceaea3-9df3-416a-bfe5-107d3233dd59",
        symbol: "OKTA",
        cluster_id: "identity",
        cluster_thesis: null,
        parent_themes: [{
          key: "ai_compute_infrastructure",
          name: "AI Compute Infrastructure",
          scope: "theme",
          state: "forming",
          direction: "mixed",
          role: "candidate",
          conviction: 50,
          rationale: "Identity security expression of AI infrastructure budget priority.",
          summary: "AI capex remains the parent theme, but symbol selection must separate leaders from challengers.",
        }],
        state: "forming",
        edge_rationale: "Identity platform consolidation can improve growth durability.",
        bull_case: "Growth stabilizes.",
        bear_case: "Execution slips.",
        forecast: { direction: "up", target: 130, deadline_at: "2026-12-31" },
        conviction_conditions: [],
        trigger_conditions: [],
        invalidation_conditions: [],
        fulfillment_conditions: [],
        conviction_tier: "medium",
        system_confidence: "medium",
        system_confidence_components: {
          evidence_strength: "usable",
          freshness: "fresh",
        },
        instrument: "equity",
        intended_size: null,
        version: 1,
        immutable_original: {},
        created_at: "2026-06-01T00:00:00Z",
        updated_at: "2026-06-01T00:00:00Z",
        history: [
          {
            version: 1,
            diff: {},
            rationale: "smoketest duplicate",
            weakens_invalidation: false,
            at: "2026-06-01T00:00:00Z",
          },
          {
            version: 1,
            diff: {},
            rationale: "smoketest duplicate",
            weakens_invalidation: false,
            at: "2026-06-01T00:00:00Z",
          },
        ],
        evidence_items: [
          {
            id: 501,
            symbol: "OKTA",
            kind: "news",
            observed_at: "2026-06-01T00:00:00Z",
            source: "fmp",
            source_id: "news_article:501",
            source_ref: { table: "news_article", id: 501 },
            summary: "OKTA customer deployment article supports consolidation demand",
            strength: 0.8,
            polarity: 0.6,
            url: "https://example.com/evidence",
            created_at: "2026-06-01T00:01:00Z",
            weight: 0.9,
            added_by: "system",
          },
          {
            id: 502,
            symbol: "OKTA",
            kind: "estimate_revision",
            observed_at: "2026-05-31T00:00:00Z",
            source: "fmp_estimates",
            source_id: "estimate_revision:502",
            source_ref: { table: "estimate_revision", id: 502 },
            summary: "OKTA annual estimate revision up EPS 3.2%",
            strength: 0.5,
            polarity: 0.7,
            url: null,
            created_at: "2026-05-31T00:01:00Z",
            weight: 0.5,
            added_by: "system",
          },
        ],
        substance: {
          score: 2,
          max_score: 6,
          missing: ["conviction_conditions", "trigger_conditions", "invalidation_conditions", "intended_size", "fulfillment_conditions"],
          blocked_at: "building_conviction",
          well_formed: { conviction: 0, trigger: 0, invalidation: 0, fulfillment: 0 },
          freshness_score: 0.42,
          freshness_status: "limited",
          confidence_cap: "low",
          freshness_penalties: ["context: narrative context is stale"],
          freshness_components: [
            { name: "market", status: "fresh", score: 1, last_at: "2026-06-01T00:00:00Z", reason: "market checked within freshness target" },
            { name: "context", status: "old", score: 0.4, last_at: "2026-03-01T00:00:00Z", reason: "context is too old for high-confidence promotion" },
          ],
        },
      }] : []);
      return;
    }
    if (path === "/api/thesis-declines") {
      const symbol = url.searchParams.get("symbol");
      await json(route, symbol === "MSFT" ? [{
        id: 9001,
        symbol: "MSFT",
        candidate_id: null,
        severity: "info",
        status: "dismissed",
        title: "MSFT: system declined to draft a thesis",
        reason: "Context contains no non-consensus edge yet; wait for estimate revisions or a new catalyst.",
        source_ref: { reason: "no_edge" },
        created_at: "2026-06-01T00:00:00Z",
        resolved_at: null,
        resolution_kind: null,
      }] : []);
      return;
    }
    if (path === "/api/evidence-requirements") {
      await json(route, [
        {
          id: 8101,
          symbol: url.searchParams.get("symbol") ?? "MSFT",
          requirement_key: "product_research",
          source_type: "web_research",
          reason: "Need product/theme web research before claiming public evidence does or does not exist.",
          priority: "high",
          blocking_state: "satisfied",
          attempts: 1,
          next_retry_at: null,
          last_error: null,
          source_ref: { counts: { research_evidence: 2 }, fetch_actions: ["gdelt_doc_search", "bing_news_rss_search", "firecrawl_search"] },
          source_tasks: [],
          created_at: "2026-06-01T00:00:00Z",
          updated_at: "2026-06-01T00:00:00Z",
          satisfied_at: "2026-06-01T00:00:00Z",
        },
        {
          id: 8102,
          symbol: url.searchParams.get("symbol") ?? "MSFT",
          requirement_key: "analyst_estimates",
          source_type: "estimates",
          reason: "Need analyst estimate snapshots before evaluating revision/consensus drift.",
          priority: "high",
          blocking_state: "missing",
          attempts: 2,
          next_retry_at: "2026-06-01T00:30:00Z",
          last_error: null,
          source_ref: { counts: { estimate_snapshots: 0 }, fetch_actions: ["fmp_analyst_estimates"] },
          source_tasks: [{
            id: 9101,
            action: "fmp_analyst_estimates",
            provider: "fmp",
            state: "queued",
            priority: "high",
            due_at: "2026-06-01T00:30:00Z",
            next_retry_at: "2026-06-01T00:30:00Z",
            attempts: 2,
            last_error: null,
            updated_at: "2026-06-01T00:00:00Z",
          }],
          created_at: "2026-06-01T00:00:00Z",
          updated_at: "2026-06-01T00:00:00Z",
          satisfied_at: null,
        },
      ]);
      return;
    }
    if (path === "/api/research-evidence") {
      await json(route, [{
        id: 8201,
        symbol: url.searchParams.get("symbol") ?? "MSFT",
        query: "AMD MI355X deployment benchmark adoption",
        url: "https://example.com/amd-mi355x",
        title: "AMD MI355X production deployment expands",
        publisher: "Example Research",
        published_at: "2026-05-15T00:00:00Z",
        retrieved_at: "2026-06-01T00:00:00Z",
        provider: "bing_news_rss",
        source_type: "news_search",
        credibility: "industry",
        summary: "Deployment detail",
        tags: ["AMD", "MI355X"],
      }]);
      return;
    }
    if (path === "/api/evidence-items") {
      const symbol = url.searchParams.get("symbol") ?? "MSFT";
      await json(route, [
        {
          id: 501,
          symbol,
          kind: "news",
          observed_at: "2026-06-01T00:00:00Z",
          source: "fmp",
          source_id: "news_article:501",
          source_ref: { table: "news_article", id: 501 },
          summary: `${symbol} customer deployment article`,
          strength: 0.8,
          polarity: 0.4,
          url: "https://example.com/evidence",
          created_at: "2026-06-01T00:01:00Z",
        },
        {
          id: 502,
          symbol,
          kind: "estimate_revision",
          observed_at: "2026-05-31T00:00:00Z",
          source: "fmp_estimates",
          source_id: "estimate_revision:502",
          source_ref: { table: "estimate_revision", id: 502 },
          summary: `${symbol} annual estimate revision up EPS 3.2%`,
          strength: 0.5,
          polarity: 0.7,
          url: null,
          created_at: "2026-05-31T00:01:00Z",
        },
      ]);
      return;
    }
    if (path === "/api/positions") {
      const symbol = url.searchParams.get("symbol");
      await json(route, symbol === "OKTA" ? [{
        position_id: "9b496f4d-cbb8-4bb5-bd41-9766f8f962f2",
        thesis_id: "12ceaea3-9df3-416a-bfe5-107d3233dd59",
        symbol: "OKTA",
        side: "long",
        instrument: "equity",
        qty: 12,
        avg_price: 88,
        delta_notional: 1056,
        premium_at_risk: 0,
        opened_at: "2026-06-01T00:00:00Z",
        closed_at: null,
        realized_pnl: null,
        unrealized_pnl: 96,
        latest_price: 96,
        latest_price_at: "2026-06-01T00:00:00Z",
        fill_count: 1,
        thesis_state: "position_open",
        thesis_direction: "up",
      }] : []);
      return;
    }
    if (path === "/api/decisions" && request.method() === "POST") {
      calls.decisionBody = await request.postDataJSON();
      await json(route, {
        decision_id: "8b4c3f5b-8288-49ff-9282-b4398abe85ba",
        ticket_id: "02543ae2-2270-4791-a8b3-e49c5fbafec4",
        position_id: "9b496f4d-cbb8-4bb5-bd41-9766f8f962f2",
        fill_id: "ecae97a9-8719-48f0-b6a7-b74c85324173",
        risk_result: { status: "pass", veto: false, reasons: [], warnings: [] },
        transitioned_to: null,
      });
      return;
    }
    if (path === "/api/decisions/8b4c3f5b-8288-49ff-9282-b4398abe85ba/replay") {
      await json(route, {
        decision_id: "8b4c3f5b-8288-49ff-9282-b4398abe85ba",
        symbol: "OKTA",
        thesis_id: "12ceaea3-9df3-416a-bfe5-107d3233dd59",
        context_version: 2,
        thesis_snapshot: {
          thesis_id: "12ceaea3-9df3-416a-bfe5-107d3233dd59",
          symbol: "OKTA",
          state: "forming",
          version: 1,
          forecast: { direction: "up", system_confidence: "medium" },
          conviction_tier: "medium",
          system_confidence: "medium",
          system_confidence_components: { evidence_strength: "usable" },
        },
        consensus_score: 64,
        risk_verdict: { status: "pass", veto: false, reasons: [], warnings: [] },
        evidence_ids: [501],
        evidence_snapshot: [{
          id: 501,
          symbol: "OKTA",
          kind: "news",
          observed_at: "2026-06-01T00:00:00Z",
          source: "fmp",
          source_id: "news_article:501",
          source_ref: { table: "news_article", id: 501 },
          summary: "OKTA customer deployment article supports consolidation demand",
          strength: 0.8,
          polarity: 0.6,
          url: "https://example.com/evidence",
          created_at: "2026-06-01T00:01:00Z",
          weight: 0.9,
          added_by: "system",
        }],
        system_confidence: "medium",
        chart_range_seen: "ALL 1D",
        decision_snapshot: {
          decision_id: "8b4c3f5b-8288-49ff-9282-b4398abe85ba",
          action: "skip",
          user_choice: "deferred",
          disagreement_reason: "valuation_priced",
          disagreement_detail: "Story is true, but the chart already reflects it.",
          human_conviction: "low",
          reason: "Technically extended despite useful narrative.",
        },
        captured_at: "2026-06-01T00:02:00Z",
      });
      return;
    }
    if (path === "/api/decisions") {
      const symbol = url.searchParams.get("symbol");
      await json(route, symbol === "OKTA" ? [{
        decision_id: "8b4c3f5b-8288-49ff-9282-b4398abe85ba",
        thesis_id: "12ceaea3-9df3-416a-bfe5-107d3233dd59",
        action: "skip",
        user_choice: "deferred",
        disagreement_reason: "valuation_priced",
        disagreement_detail: "Story is true, but the chart already reflects it.",
        human_conviction: "low",
        reason: "Technically extended despite useful narrative.",
        sizing: { thesis_direction: "up" },
        thesis_state: "forming",
        thesis_direction: "up",
        side: "",
        instrument: "equity",
        has_replay: true,
        at: "2026-06-01T00:02:00Z",
      }] : []);
      return;
    }
    if (path === "/api/candles") {
      calls.candleUrls.push(url);
      const interval = url.searchParams.get("interval");
      await json(route, interval === "1D" ? dailyCandles() : hourlyCandles());
      return;
    }
    if (path === "/api/symbol-events") {
      await json(route, []);
      return;
    }
    if (/^\/api\/symbols\/[^/]+\/refresh-context$/.test(path) && request.method() === "POST") {
      calls.refreshContextSymbols.push(decodeURIComponent(path.split("/")[3]));
      await route.fulfill({ status: 204 });
      return;
    }
    if (/^\/api\/symbols\/[^/]+\/start-research$/.test(path) && request.method() === "POST") {
      const symbol = decodeURIComponent(path.split("/")[3]);
      calls.researchSymbols.push(symbol);
      await json(route, {
        symbol,
        requirement: { key: "product_research", state: "missing" },
        queued: 3,
        blocked: 0,
        tasks: [
          { id: 1, action: "gdelt_doc_search", provider: "gdelt", state: "queued", attempts: 0 },
          { id: 2, action: "bing_news_rss_search", provider: "bing", state: "queued", attempts: 0 },
          { id: 3, action: "firecrawl_search", provider: "firecrawl", state: "queued", attempts: 0 },
        ],
        cognition_event_published: true,
      });
      return;
    }
    if (path === "/api/system-status") {
      await json(route, {
        database: {
          status: "ok",
          reachable: true,
          database: "stocks",
          checked_at: "2026-06-01T12:00:00Z",
          latency_ms: 3,
          reason: null,
        },
        ingest: {},
        price_freshness: {
          expected_latest_session: "2026-06-01",
          actual_latest_session: "2026-06-01",
          symbols_total: 3,
          symbols_fresh: 3,
          status: "ok",
        },
        discovery: { last_pass_at: null, open_candidates: 1, by_signal: [], pool_size: 0 },
        cognition: { contexts_24h: 1, contexts_total_symbols: 3, thesis_by_state: [] },
        evidence: {
          open_requirements: 3,
          source_tasks_due: 2,
          source_tasks_stale_fetching: 1,
          by_state: [{ state: "missing", count: 2 }],
          by_reason: [{ reason: "fetching_required_sources", count: 1 }],
          source_tasks_by_state: [{ state: "fetching", count: 1 }],
          source_tasks_by_action: [{
            provider: "fmp",
            action: "fmp_analyst_estimates",
            state: "fetching",
            count: 17,
            due_count: 0,
            stale_fetching_count: 1,
            next_due_at: null,
            last_updated_at: "2026-06-01T12:00:00Z",
            sample_targets: ["HPE", "JKHY", "GIS"],
          }],
        },
        attention: { open_items: attentionOpen ? 1 : 0, by_kind: [] },
        source_health: [{
          source: "xbrl",
          last_status: "running",
          effective_status: "stale_running",
          stale_running: true,
          running_age_minutes: 30,
          last_started_at: "2026-06-01T12:00:00Z",
          last_success_at: null,
          last_failure_at: null,
          last_failure_kind: null,
          last_error: null,
          retry_after_at: null,
          rows_seen: 0,
          rows_inserted: 0,
          symbols_attempted: 1,
          symbols_failed: 0,
        }],
        llm: { calls_24h: 0, avg_latency_ms: null, by_prompt: [] },
      });
      return;
    }

    await json(route, { error: `unmocked ${path}` }, 500);
  });

  return calls;
}

test("chart defaults to ALL range and interval controls change bar size only", async ({ page }) => {
  const calls = await mockApi(page);
  await page.goto("/");

  await expect(page.locator(".symbol-box input")).toHaveValue("MSFT");
  await expect(page.getByTestId("chart-topbar")).toBeVisible();
  await expect(page.getByTestId("chart-left-tools")).toBeVisible();
  await expect(page.getByTestId("chart-interval-status")).toContainText("1D");
  await expect(page.getByTestId("chart-interval-status")).toContainText("ALL");
  await expect(page.getByText("SMA 200D")).toBeVisible();
  await expect(page.getByText("VWAP 20D").first()).toBeVisible();
  await expect(page.getByTestId("rsi-legend")).toHaveText("RSI 14");
  await expect(page.getByTestId("pso-legend")).toHaveText("PSO 8/25");
  await expect(page.getByTestId("pso32-legend")).toHaveText("PSO 32");
  await page.getByTestId("chart-indicators-button").click();
  await expect(page.getByTestId("chart-indicator-menu")).toBeVisible();
  await expect(page.getByTestId("chart-indicator-menu")).toContainText("VWAP 50D");
  await expect(page.getByTestId("chart-alert-button")).toContainText("1");
  await page.getByTestId("chart-alert-button").click();
  await expect(page.getByTestId("chart-alert-menu")).toBeVisible();
  await expect(page.getByTestId("chart-alert-menu")).toContainText("AI above 208.50");
  await page.getByLabel("Alert price").fill("222.22");
  await page.getByLabel("Alert label").fill("MSFT breakout check");
  await page.getByTestId("chart-alert-menu").getByRole("button", { name: "Create" }).click();
  await expect.poll(() => calls.priceAlertBodies).toContainEqual({
    symbol: "MSFT",
    origin: "manual",
    intent: "watch",
    direction: "above",
    target_price: 222.22,
    label: "MSFT breakout check",
    rationale: "Manual chart alert",
    source_ref: { surface: "chart" },
  });
  await expect(page.getByTestId("chart-alert-button")).toContainText("2");
  await expect.poll(() => calls.candleUrls.some((url) =>
    url.searchParams.get("symbol") === "MSFT"
    && url.searchParams.get("range") === "ALL"
    && url.searchParams.get("interval") === "1D",
  )).toBe(true);

  await page.getByTestId("interval-1h").click();

  await expect(page.getByTestId("chart-interval-status")).toContainText("1h");
  await expect(page.getByTestId("chart-interval-status")).toContainText("ALL");
  await expect.poll(() => calls.candleUrls.some((url) =>
    url.searchParams.get("range") === "ALL"
    && url.searchParams.get("interval") === "1h",
  )).toBe(true);
  await expect.poll(() => calls.candleUrls.filter((url) =>
    url.searchParams.get("range") === "ALL"
    && url.searchParams.get("interval") === "1D",
  ).length).toBeGreaterThanOrEqual(2);
});

test("chart patches latest candle from market bar stream events", async ({ page }) => {
  await mockApi(page, {
    streamEvents: [{
      subject: "market.bar.1D.MSFT",
      kind: "market_bar",
      payload: {
        symbol: "MSFT",
        interval: "1D",
        time: "2026-06-01",
        open: 330,
        high: 335,
        low: 329,
        close: 333.33,
        volume: 1_234_567,
        status: "live",
      },
    }],
  });
  await page.goto("/");

  await expect(page.locator(".symbol-box input")).toHaveValue("MSFT");
  await expect(page.getByTestId("chart-live-status")).toContainText("live");
  await expect(page.getByTestId("chart-last-close")).toHaveText("333.33");
});

test("alerts tab shows active price alert rules", async ({ page }) => {
  await mockApi(page);
  await page.goto("/symbol/MSFT?p=alerts");

  const panel = page.getByTestId("price-alerts-panel");
  await expect(panel).toContainText("Price Alerts");
  await expect(panel).toContainText("MSFT reclaim entry");
  await expect(panel).toContainText("$208.50");

  await panel.getByRole("button", { name: "disable" }).click();
  await expect(panel).toContainText("No active price levels");
  await expect(panel).toContainText("Disabled");
});

test("theses tab lists declined thesis attempts with reasons", async ({ page }) => {
  await mockApi(page);
  await page.goto("/");

  await page.getByRole("button", { name: "theses" }).click();

  const detail = page.getByRole("complementary");
  await expect(page.getByText("Declined thesis attempts")).toBeVisible();
  await expect(detail.getByText("Context contains no non-consensus edge yet")).toBeVisible();
  await expect(page.getByText("No thesis attempts")).toHaveCount(0);
});

test("pool-only symbol context does not imply active synthesis", async ({ page }) => {
  const calls = await mockApi(page);
  await page.goto("/symbol/SNDK");

  await expect(page).toHaveURL(/\/symbol\/SNDK\?p=overview$/);
  await expect(page.getByTestId("workflow-strip")).toContainText("Pool candidate");
  await expect(page.getByTestId("workflow-primary")).toHaveText("Promote to Universe");

  const review = page.getByTestId("pool-candidate-review");
  await expect(review).toContainText("Review SNDK");
  await expect(review).toContainText("Sandisk Corporation");
  await expect(review).toContainText("Universe always included");
  await expect(review).toContainText("not the active Universe");

  await review.getByRole("button", { name: "Start research" }).click();
  await expect.poll(() => calls.promoteBody).toEqual({ symbol: "SNDK", tier: 2, watchlist_ids: [] });

  await page.getByRole("button", { name: "context", exact: true }).click();

  const context = page.locator(".empty").filter({ hasText: "SNDK" });
  await expect(context).toContainText("Context");
  await expect(context).toContainText("not running");
  await expect(context).toContainText("not in the active Universe");
  await expect(context).toContainText("Promote the symbol first");
  await expect(page.getByText("synthesizing…")).toHaveCount(0);
  await expect.poll(() => calls.refreshContextSymbols).toEqual([]);
  await expect(page).toHaveURL(/\/symbol\/SNDK\?p=context$/);
});

test("overview explains selected symbol brain status and stale source", async ({ page }) => {
  await mockApi(page);
  await page.goto("/");

  const brain = page.locator(".brain-card.brain-due");
  await expect(brain).toBeVisible();
  await expect(brain).toContainText("Brain");
  await expect(brain).toContainText("due");
  await expect(brain).toContainText("reevaluate thesis");
  await expect(brain).toContainText("open thesis is past the re-evaluation window");
  await expect(brain).toContainText("4 rows, 0 open");
  await expect(brain).toContainText("price");
  await expect(brain).toContainText("thesis");
  await expect(brain).toContainText("stale");
  await expect(brain).toContainText("checked");
  await expect(brain).toContainText("changed");
  await expect(brain).toContainText("evaluated");
  await expect(brain).toContainText("session 2026-05-29/2026-05-29");
  await expect(brain).toContainText("analyst opinion");
  await expect(brain).toContainText("1 targets");
  await expect(brain).toContainText("tasks fmp price target consensus queued");
  await expect(brain).toContainText("SLA 30m");
  await expect(brain).toContainText("v2");
});

test("overview labels evidence-delta cognition runs", async ({ page }) => {
  await mockApi(page);
  await page.goto("/symbol/OKTA");

  const brain = page.locator(".brain-card.brain-fresh");
  await expect(brain).toContainText("Last cognition run");
  await expect(brain).toContainText("reconciled");
  await expect(brain).toContainText("evidence newer than thesis");
  await expect(brain).toContainText("evidence");
  await expect(brain).toContainText("classification strengthened_view");
});

test("overview explains evidence-updated brain action", async ({ page }) => {
  await mockApi(page);
  await page.goto("/symbol/CRDO");

  const brain = page.locator(".brain-card.brain-due");
  await expect(brain).toContainText("re-evaluate after evidence update");
  await expect(brain).toContainText("normalized evidence is newer than the current thesis evaluation");

  const evidence = brain.locator(".brain-sources li").filter({ hasText: "evidence" });
  await expect(evidence).toContainText("fresh");
  await expect(evidence).toContainText("8 items");
  await expect(evidence).toContainText("newer than thesis");
});

test("overview shows selected symbol placement and watchlists", async ({ page }) => {
  await mockApi(page);
  await page.goto("/symbol/OKTA?p=overview");

  const placement = page.getByTestId("symbol-status-card");
  await expect(placement).toContainText("System Placement");
  await expect(placement).toContainText("Active Universe");
  await expect(placement).toContainText("T2");
  await expect(placement).toContainText("Core");
  await expect(placement).toContainText("forming · bull");
  await expect(placement).toContainText("The scheduled brain loop may refresh context");
});

test("placement card directly promotes pool-only symbols", async ({ page }) => {
  const calls = await mockApi(page);
  await page.goto("/symbol/SNDK?p=overview");

  const placement = page.getByTestId("symbol-status-card");
  await expect(placement).toContainText("Discovery Pool");
  await expect(placement).toContainText("known to discovery");
  await expect(placement).toContainText("until it is promoted");

  const promote = page.getByTestId("placement-promote");
  await expect(promote).toContainText("Universe always included");
  await promote.getByLabel("Core").check();
  await promote.getByRole("button", { name: "Start research" }).click();

  await expect.poll(() => calls.promoteBody).toEqual({
    symbol: "SNDK",
    tier: 2,
    watchlist_ids: ["wl-core"],
  });
});

test("workflow rail shows selected symbol state and routes to thesis review", async ({ page }) => {
  await mockApi(page);
  await page.goto("/");

  const strip = page.getByTestId("workflow-strip");
  await expect(strip).toContainText("MSFT");
  await expect(strip).toContainText("Declined thesis");
  await expect(strip).toContainText("Universe T1");
  await expect(strip).toContainText("1 open evidence");
  await expect(strip).toContainText("declined attempt");
  await expect(page.getByTestId("workflow-primary")).toHaveText("Review decline");
  await expect(page.locator('[aria-label="Selected symbol workflow"]')).toBeVisible();

  await page.getByTestId("workflow-primary").click();

  await expect(page.locator(".tabs button.active")).toHaveText("theses");
});

test("theses tab shows nominated state for unpromoted symbols", async ({ page }) => {
  const calls = await mockApi(page, {
    attentionItems: [{
      id: 8801,
      kind: "candidate_review",
      symbol: "ORCL",
      thesis_id: null,
      candidate_id: 880,
      severity: "review",
      status: "open",
      fsm_state: "ready_for_review",
      owner: "operator",
      title: "ORCL: research nomination",
      reason: "Research nomination: Oracle fits software infrastructure for AI/cloud operations.",
      source: "discovery",
      source_ref: {
        interpretation_kind: "research_nomination",
        available_data: { price: true, news: true, estimates: true, fundamentals: true },
        nomination_reasons: {
          acceptance_effect: "add to monitored universe/watchlists and run context/thesis",
          business_fit: "AI infrastructure needs secure, observable, automated cloud/software operations",
          theme: "software infrastructure for AI/cloud operations",
          suggested_watchlists: ["Software Infrastructure"],
        },
      },
      created_at: "2026-06-01T00:00:00Z",
      resolved_at: null,
      resolution_kind: null,
      next_retry_at: null,
      resurface_at: null,
      state_reason: "candidate_review",
    }],
  });
  await page.goto("/symbol/ORCL");

  const strip = page.getByTestId("workflow-strip");
  await expect(strip).toContainText("ORCL");
  await expect(strip).toContainText("Nominated, not active");
  await expect(strip).toContainText("nominated");
  await expect(page.getByTestId("workflow-primary")).toHaveText("Promote to Universe");

  const promotion = page.getByTestId("promotion-review");
  await expect(promotion).toContainText("Start research for ORCL");
  await expect(promotion).toContainText("Discovery nominated ORCL for operator review.");
  await expect(promotion).toContainText("software infrastructure for AI/cloud operations");
  await expect(promotion).toContainText("What starting does");
  await expect(promotion).toContainText("publishes discovery.confirmed");
  await expect(promotion).toContainText("Universe always included");
  await expect(promotion).toContainText("start as Universe-only");

  await page.getByRole("button", { name: "theses" }).click();

  const promotionPanel = page.getByTestId("thesis-promotion-panel");
  await expect(promotionPanel).toContainText("Not active yet");
  await expect(promotionPanel).toContainText("Start research");

  const nomination = page.locator(".nomination-state").filter({ hasText: "Nominated, not active" });
  await expect(nomination).toContainText("Nominated, not active");
  await expect(nomination).toContainText("software infrastructure for AI/cloud operations");
  await expect(nomination).toContainText("secure, observable, automated cloud/software operations");
  await expect(nomination).toContainText("price");
  await expect(nomination).toContainText("news");
  await expect(nomination).toContainText("estimates");
  await expect(nomination).toContainText("fundamentals");
  await expect(nomination).toContainText("Starting research will add to monitored universe/watchlists and run context/thesis.");
  await expect(page.getByText("No thesis attempts")).toHaveCount(0);

  const placement = page.getByTestId("thesis-placement-strip");
  await expect(placement).toContainText("Nominated");
  await expect(placement).toContainText("nomination");
  await placement.getByRole("button", { name: "Start research" }).click();

  await expect.poll(() => calls.confirmBody).toEqual({ watchlist_ids: [] });
});

test("workflow primary opens nominated review packet", async ({ page }) => {
  await mockApi(page, {
    attentionItems: [{
      id: 8801,
      kind: "candidate_review",
      symbol: "ORCL",
      thesis_id: null,
      candidate_id: 880,
      severity: "review",
      status: "open",
      fsm_state: "ready_for_review",
      owner: "operator",
      title: "ORCL: research nomination",
      reason: "Research nomination: Oracle fits software infrastructure for AI/cloud operations.",
      source: "discovery",
      source_ref: {
        interpretation_kind: "research_nomination",
        available_data: { price: true, news: true, estimates: true, fundamentals: true },
        nomination_reasons: {
          acceptance_effect: "add to monitored universe/watchlists and run context/thesis",
          business_fit: "AI infrastructure needs secure, observable, automated cloud/software operations",
          theme: "software infrastructure for AI/cloud operations",
          suggested_watchlists: ["Software Infrastructure"],
        },
      },
      created_at: "2026-06-01T00:00:00Z",
      resolved_at: null,
      resolution_kind: null,
      next_retry_at: null,
      resurface_at: null,
      state_reason: "candidate_review",
    }],
  });
  await page.goto("/symbol/ORCL");

  await expect(page.getByTestId("workflow-primary")).toHaveText("Promote to Universe");
  await page.getByTestId("workflow-primary").click();

  const packet = page.getByTestId("review-packet");
  await expect(packet).toContainText("Start research for ORCL?");
  const sections = page.getByTestId("review-packet-sections");
  await expect(sections.getByText("What happened")).toBeVisible();
  await expect(sections.getByText("Why it matters")).toBeVisible();
  await expect(sections.getByText("Evidence")).toBeVisible();
  await expect(sections.getByText("Recommendation")).toBeVisible();
  await expect(sections.getByText("What will be recorded")).toBeVisible();
  await expect(packet.getByText("Receipts")).toHaveCount(0);
});

test("selected promotion review posts checked watchlist destinations", async ({ page }) => {
  const calls = await mockApi(page);
  await page.goto("/symbol/NVDA");

  const promotion = page.getByTestId("promotion-review");
  await expect(promotion).toContainText("Start research for NVDA");
  await expect(promotion).toContainText("Core");
  await expect(promotion).toContainText("AI infrastructure fit");
  const corePick = promotion.locator("label", { hasText: "Core" }).getByRole("checkbox");
  await expect(corePick).toBeChecked();

  await promotion.getByRole("button", { name: "Start research" }).click();

  await expect.poll(() => calls.confirmBody).toEqual({ watchlist_ids: ["wl-core"] });
});

test("workflow rail surfaces open position tracking and routes to decisions", async ({ page }) => {
  await mockApi(page);
  await page.goto("/symbol/OKTA");

  const strip = page.getByTestId("workflow-strip");
  await expect(strip).toContainText("OKTA");
  await expect(strip).toContainText("Position tracking");
  await expect(strip).toContainText("1 attention");
  await expect(strip).toContainText("forming · bull");
  await expect(page.getByTestId("workflow-primary")).toHaveText("Track position");
  await expect(page.getByTestId("workflow-attention")).toContainText("OKTA thesis changed");

  await page.getByTestId("workflow-primary").click();

  await expect(page.locator(".tabs button.active")).toHaveText("decisions");
});

test("thesis card opens a prefilled decision form", async ({ page }) => {
  await mockApi(page);
  await page.goto("/symbol/OKTA?p=theses");

  await page.getByTestId("thesis-record-decision").click();

  await expect(page.locator(".bottom-tabs button.active")).toHaveText("decisions");
  await expect(page.getByLabel("Thesis ID")).toHaveValue("12ceaea3-9df3-416a-bfe5-107d3233dd59");
  await expect(page.getByLabel("Action")).toHaveValue("skip");
});

test("thesis review packet opens an inline prefilled decision form", async ({ page }) => {
  await mockApi(page);
  await page.goto("/symbol/OKTA");

  await page.getByTestId("workflow-attention").getByRole("button", { name: /OKTA thesis changed/ }).click();
  const packet = page.getByTestId("review-packet");
  await expect(packet).toContainText("OKTA review");

  await packet.getByRole("button", { name: /Record decision/ }).click();

  const inline = page.getByTestId("review-packet-decision-form");
  await expect(inline).toBeVisible();
  await expect(inline.getByLabel("Thesis ID")).toHaveValue("12ceaea3-9df3-416a-bfe5-107d3233dd59");
  await expect(inline.getByLabel("Action")).toHaveValue("skip");
  await expect(page.locator(".bottom-tabs button.active")).not.toHaveText("decisions");
});

test("thesis review packet submits the decision inline", async ({ page }) => {
  const calls = await mockApi(page);
  await page.goto("/symbol/OKTA");

  await page.getByTestId("workflow-attention").getByRole("button", { name: /OKTA thesis changed/ }).click();
  const packet = page.getByTestId("review-packet");
  await packet.getByRole("button", { name: /Record decision/ }).click();

  const inline = page.getByTestId("review-packet-decision-form");
  await expect(inline).toBeVisible();
  await expect(inline.getByLabel("Thesis ID")).toHaveValue("12ceaea3-9df3-416a-bfe5-107d3233dd59");
  await expect(inline.getByLabel("Action")).toHaveValue("skip");
  await expect(page.locator(".bottom-tabs button.active")).not.toHaveText("decisions");

  await inline.getByLabel("Why").selectOption("valuation_priced");
  await inline.getByLabel("Human conviction").selectOption("low");
  await inline.getByLabel("Decision reason").fill("Inline review packet decision.");
  await inline.getByRole("button", { name: "Submit" }).click();

  await expect.poll(() => calls.decisionBody).toMatchObject({
    thesis_id: "12ceaea3-9df3-416a-bfe5-107d3233dd59",
    action: "skip",
    user_choice: "deferred",
    disagreement_reason: "valuation_priced",
    human_conviction: "low",
    reason: "Inline review packet decision.",
  });
});

test("brain tab shows macro and theme theses with linked symbols", async ({ page }) => {
  await mockApi(page);
  await page.goto("/");

  await page.getByRole("button", { name: "brain" }).click();

  await expect(page.locator(".brain-topline")).toContainText("2 active");
  await expect(page.locator(".macro-theme")).toContainText("Macro Regime");
  await expect(page.locator(".macro-theme")).toContainText("fred fresh");
  await expect(page.locator(".macro-theme")).toContainText("56% >200D");
  await expect(page.locator(".macro-theme")).toContainText("655 symbols");
  await expect(page.locator(".macro-theme")).toContainText("Technology / Healthcare / Industrials");
  await expect(page.locator(".macro-theme")).toContainText("HY OAS 2.72%");
  await expect(page.locator(".macro-theme")).toContainText("Dislocation Map");
  await expect(page.locator(".macro-theme")).toContainText("Loved / mania");
  await expect(page.locator(".macro-theme")).toContainText("Technology");
  await expect(page.locator(".macro-theme")).toContainText("Ignored");
  await expect(page.locator(".macro-theme")).toContainText("Industrials");
  await expect(page.locator(".macro-theme")).toContainText("Hated / avoided");
  await expect(page.locator(".macro-theme")).toContainText("Financial Services");

  const theme = page.locator(".brain-theme").filter({ hasText: "AI Compute Infrastructure" });
  await expect(theme).toContainText("AI capex remains the parent theme");
  await expect(theme).toContainText("1/2 context");
  await expect(theme).toContainText("Core");
  await expect(theme.getByRole("button", { name: /NVDA leader/ })).toBeVisible();
  await expect(theme.getByRole("button", { name: /OKTA/ })).toContainText("forming");
});

test("journal page shows daily history and routes symbol entries", async ({ page }) => {
  const calls = await mockApi(page);
  await page.goto("/journal/2026-06-01");

  await expect(page.getByRole("button", { name: "Journal" })).toHaveClass(/active/);

  const journal = page.locator("[data-testid='brain-journal-page']");
  await expect(journal).toContainText("Brain Journal");
  const tradeDesk = journal.locator("[data-testid='daily-trade-desk']");
  await expect(tradeDesk).toContainText(/daily trade desk/i);
  await expect(tradeDesk).toContainText(/would consider/i);
  await expect(tradeDesk).toContainText("CRDO");
  await expect(tradeDesk).toContainText("Actionable up thesis with constructive setup.");
  await expect(tradeDesk).toContainText("constructive");
  await expect(tradeDesk).toContainText("trend uptrend");
  await expect(tradeDesk).toContainText("relative strength outperforming");
  await expect(tradeDesk).toContainText(/would wait/i);
  await expect(tradeDesk).toContainText("OKTA");
  await expect(tradeDesk).toContainText("Wait for pullback or refreshed context.");
  await expect(tradeDesk).toContainText("avoid chase");
  await expect(tradeDesk).toContainText(/would avoid/i);
  await expect(tradeDesk).toContainText("AMD");
  await expect(tradeDesk).toContainText("avoid breakdown");
  await expect(tradeDesk).toContainText("Blocked data and breakdown technicals make it a no-trade today.");
  await expect(tradeDesk).toContainText(/research first/i);
  await expect(tradeDesk).toContainText("NVDA");
  await expect(tradeDesk).toContainText("No open thesis yet.");
  await tradeDesk.locator(".trade-section.research").getByRole("button", { name: "Start research" }).click();
  await expect.poll(() => calls.researchSymbols).toContainEqual("NVDA");
  await expect(journal).toContainText("NVDA: 3 research tasks queued");
  await expect(journal.locator("[data-testid='brain-journal-memo']")).toContainText("Daily Brain Memo");
  await expect(journal.locator("[data-testid='brain-journal-memo']")).toContainText("market neutral · macro neutral");
  await expect(journal.locator("[data-testid='brain-journal-memo']")).toContainText("Top Candidates");
  await expect(journal.locator("[data-testid='brain-journal-memo']")).toContainText("CRDO");
  await expect(journal.locator("[data-testid='brain-journal-memo']")).toContainText("Wait For Setup");
  await expect(journal.locator("[data-testid='brain-journal-memo']")).toContainText("OKTA");
  await expect(journal.locator("[data-testid='brain-journal-memo']")).toContainText("bullish thesis, but avoid chase");
  await expect(journal.locator("[data-testid='brain-journal-memo']")).toContainText("News And Evidence");
  await expect(journal.locator("[data-testid='brain-journal-memo']")).toContainText("Credo highlighted new AI-cluster design win momentum.");
  await expect(journal).toContainText("Receipts");
  await expect(journal).toContainText("5 total entries");
  await expect(journal).toContainText("we think this changed");
  await expect(journal).toContainText("OKTA thesis updated to v2");
  await expect(journal).toContainText("needs research");
  await expect(journal).toContainText("Research queued: NVDA via volume anomaly");
  await expect(journal).toContainText("crowded or extended");
  await expect(journal).toContainText("Loved / mania: Technology");
  await expect(journal).toContainText("ignored or hated");
  await expect(journal).toContainText("Hated / avoided: Financial Services");
  await expect(journal).toContainText("blocked");
  await expect(journal).toContainText("Data blocked: MSFT analyst estimates");

  await journal.getByRole("button", { name: /OKTA thesis updated to v2/ }).click();
  await expect(page).toHaveURL(/\/symbol\/OKTA/);
  await expect(page.locator(".symbol-box input")).toHaveValue("OKTA");
  await expect(page.locator(".right .tabs button.active")).toHaveText("theses");

  await page.goto("/journal/2026-06-01");
  const tradeDeskAgain = page.locator("[data-testid='daily-trade-desk']");
  await tradeDeskAgain.locator(".trade-section.wait").getByRole("button", { name: "Technical read" }).click();
  await expect(page).toHaveURL(/\/symbol\/OKTA\?p=technical/);
  await expect(page.getByRole("button", { name: "technical" })).toHaveClass(/active/);
});

test("journal daily trade desk opens the matching review packet", async ({ page }) => {
  await mockApi(page);
  await page.goto("/journal/2026-06-01");

  const tradeDesk = page.locator("[data-testid='daily-trade-desk']");
  await tradeDesk.locator(".trade-section.consider").getByRole("button", { name: "Open review packet" }).click();

  await expect(page).toHaveURL(/\/symbol\/CRDO/);
  await expect(page.getByTestId("review-packet")).toContainText("Record a decision on CRDO");
  await expect(page.getByTestId("review-packet")).toContainText("Actionable up thesis with constructive setup.");
});

test("brain tab links to journal without embedding it", async ({ page }) => {
  await mockApi(page);
  await page.goto("/");

  await page.getByRole("button", { name: "brain" }).click();

  await expect(page.locator("[data-testid='brain-journal-page']")).toHaveCount(0);
  await expect(page.locator(".brain-board")).not.toContainText("Brain Journal");

  await page.getByRole("button", { name: "Journal" }).click();
  await expect(page).toHaveURL(/\/journal\/\d{4}-\d{2}-\d{2}/);
  await expect(page.locator("[data-testid='brain-journal-page']")).toContainText("Brain Journal");
});

test("calibration tab shows parent theme expression results", async ({ page }) => {
  await mockApi(page);
  await page.goto("/");

  await page.getByRole("button", { name: "calibration" }).click();

  const parentCalibration = page.locator(".calibration-themes").filter({ hasText: "Parent Theme Calibration" });
  await expect(parentCalibration).toContainText("AI Compute Infrastructure");
  await expect(parentCalibration).toContainText("supplier");
  await expect(parentCalibration).toContainText("1/2");
  await expect(parentCalibration).toContainText("brier 0.180");

  const technicalCalibration = page.locator(".calibration-themes").filter({ hasText: "Technical Timing Calibration" });
  await expect(technicalCalibration).toContainText("pullback watch");
  await expect(technicalCalibration).toContainText("3/4");
  await expect(technicalCalibration).toContainText("outperform 67%");
});

test("overview shows selected symbol parent brain context", async ({ page }) => {
  await mockApi(page);
  await page.goto("/symbol/OKTA");

  const parentBrain = page.locator(".parent-brain-card");
  await expect(parentBrain).toContainText("AI Compute Infrastructure");
  await expect(parentBrain).toContainText("candidate");
  await expect(parentBrain).toContainText("Mock linked row.");
  await expect(parentBrain).toContainText("Which challengers have real customer traction?");

  await parentBrain.getByRole("button", { name: "open brain" }).click();
  await expect(page.getByRole("button", { name: "brain", exact: true })).toHaveClass(/active/);
});

test("symbol routes deep-link selected symbol and keep navigation state", async ({ page }) => {
  await mockApi(page);
  await page.goto("/symbol/2454.TW?p=context");

  await expect(page.locator(".symbol-box input")).toHaveValue("2454.TW");
  await expect(page.locator(".tabs button.active")).toHaveText("context");
  await expect(page).toHaveURL(/\/symbol\/2454\.TW\?p=context$/);

  await page.getByRole("button", { name: "theses" }).click();
  await expect(page).toHaveURL(/\/symbol\/2454\.TW\?p=theses$/);

  await page.locator(".wl-row").filter({ hasText: "Core" }).click();
  await page.locator(".wl-mem").filter({ hasText: "OKTA" }).getByRole("button", { name: "OKTA" }).click();

  await expect(page.locator(".symbol-box input")).toHaveValue("OKTA");
  await expect(page).toHaveURL(/\/symbol\/OKTA\?p=theses$/);

  await page.goBack();

  await expect(page.locator(".symbol-box input")).toHaveValue("2454.TW");
  await expect(page).toHaveURL(/\/symbol\/2454\.TW\?p=theses$/);
});

test("event stream surfaces connection events in the drawer", async ({ page }) => {
  await mockApi(page);
  await page.goto("/");

  await page.getByRole("button", { name: /events/ }).click();

  await expect(page.getByText("stream.connected")).toBeVisible();
});

test("symbol alerts tab excludes global alerts", async ({ page }) => {
  await mockApi(page);
  await page.goto("/symbol/OKTA");

  await page.getByRole("button", { name: "alerts" }).click();

  await expect(page.getByText("OKTA thesis moved to forming")).toBeVisible();
  await expect(page.getByText("global portfolio drawdown warning")).not.toBeVisible();
});

test("evidence tab shows retrieved research sources", async ({ page }) => {
  await mockApi(page);
  await page.goto("/");

  await page.locator(".tabs").getByRole("button", { name: "evidence", exact: true }).click();

  const requirement = page.locator(".evidence-card").filter({ hasText: "product/theme web research" }).first();
  await expect(requirement.locator("strong")).toHaveText("web research");
  await expect(page.locator(".evidence-items")).toContainText("Evidence facts");
  await expect(page.locator(".evidence-items")).toContainText("customer deployment article");
  await expect(page.locator(".evidence-items")).toContainText("estimate revision up");
  await expect(page.locator(".evidence-items")).toContainText("polarity +0.40");
  await expect(page.getByText("Research sources")).toBeVisible();
  await expect(page.getByText("AMD MI355X production deployment expands")).toBeVisible();
  await expect(page.getByText("AMD MI355X deployment benchmark adoption")).toBeVisible();
});

test("evidence tab shows source task acquisition state", async ({ page }) => {
  await mockApi(page);
  await page.goto("/");

  await page.locator(".tabs").getByRole("button", { name: "evidence", exact: true }).click();

  const requirement = page.locator(".evidence-card").filter({ hasText: "analyst estimate snapshots" }).first();
  await expect(requirement).toContainText("high priority");
  await expect(requirement).toContainText("missing");
  await expect(requirement).toContainText("source tasks: fmp analyst estimates: queued");
});

test("diagnostics tab shows source task backlog state", async ({ page }) => {
  await mockApi(page);
  await page.goto("/");

  await page.getByRole("button", { name: "diagnostics" }).click();

  const database = page.locator(".diag").filter({ hasText: "Database" });
  await expect(database).toContainText("reachable");
  await expect(database).toContainText("stocks");
  const evidence = page.locator(".diag").filter({ hasText: "Evidence" });
  await expect(evidence).toContainText("open requirements");
  await expect(evidence).toContainText("source tasks due");
  await expect(evidence).toContainText("stale fetching");
  await expect(evidence).toContainText("source fetching");
  await expect(evidence).toContainText("fmp analyst estimates");
  await expect(evidence).toContainText("HPE, JKHY");
  const sourceHealth = page.locator(".diag").filter({ hasText: "Source health" });
  await expect(sourceHealth).toContainText("started");
  await expect(sourceHealth).toContainText("stale running");
});

test("automation tab shows permissioned strategies read-only", async ({ page }) => {
  await mockApi(page);
  await page.goto("/symbol/OKTA");

  await page.getByRole("button", { name: "automation" }).click();

  const panel = page.getByTestId("automation-cockpit");
  await expect(panel).toContainText("Technical Breakout");
  await expect(panel).toContainText("OKTA");
  await expect(panel).toContainText("frozen");
  await expect(panel).toContainText("Permission Frozen");
  await expect(panel).toContainText("Market Data Stale");
  await expect(panel).toContainText("readiness blocked");
  await expect(panel).toContainText("Approval Missing");
  await expect(panel).toContainText("Insufficient Paper Orders");
  await expect(panel).toContainText("paper");
  await expect(panel.getByRole("button", { name: "freeze", exact: true })).toBeDisabled();
  await expect(panel.getByRole("button", { name: "kill switch" })).toBeDisabled();
});

test("discovery tab shows candidate ranking reasons", async ({ page }) => {
  await mockApi(page);
  await page.goto("/");

  await page.getByRole("button", { name: /discovery/ }).click();

  const card = page.locator(".disc-card").filter({ hasText: "NVDA" });
  await expect(card).toContainText("highest 82");
  await expect(card).toContainText("volume anomaly");
  await expect(card).toContainText("active parent theme fit 70");
  await expect(card).toContainText("AI Compute Infrastructure (leader)");
  await expect(card).toContainText("high-confidence watchlist fit");
});

test("attention Promote posts selected watchlist memberships", async ({ page }) => {
  const calls = await mockApi(page);
  await page.goto("/");

  const card = page.locator(".att-card").filter({ hasText: "NVDA" }).first();
  await expect(card).toBeVisible();
  await expect(card).toContainText("2.4x volume vs 200-day SMA");
  await expect(page.locator(".att-section-head").filter({ hasText: "ready for review" })).toContainText("operator owns next step");

  await card.getByRole("button", { name: "Start research" }).click();

  await expect.poll(() => calls.confirmBody).toEqual({ watchlist_ids: ["wl-core"] });
  await expect(page.getByText("No open attention. The system is quiet.")).toBeVisible();
});

test("attention thesis review opens selected symbol thesis panel", async ({ page }) => {
  await mockApi(page, {
    attentionItems: [{
      id: 7002,
      kind: "thesis_review",
      symbol: "OKTA",
      thesis_id: "12ceaea3-9df3-416a-bfe5-107d3233dd59",
      candidate_id: null,
      severity: "review",
      status: "open",
      fsm_state: "ready_for_review",
      owner: "operator",
      title: "OKTA thesis needs review: material change",
      reason: "Fresh evidence changed the standing thesis direction to down. Review before recording a decision.",
      source: "thesis",
      source_ref: {
        event: "thesis_reconciliation",
        classification: "material_change",
        operator_action_required: true,
      },
      created_at: "2026-06-01T00:00:00Z",
      resolved_at: null,
      resolution_kind: null,
      next_retry_at: null,
      resurface_at: null,
      state_reason: "thesis_material_change",
    }],
  });
  await page.goto("/");

  await page.locator(".att-filters").getByRole("button", { name: "thesis review" }).click();
  const card = page.locator(".att-card").filter({ hasText: "OKTA" });
  await expect(card).toContainText("thesis changed");
  await expect(card).toContainText("Fresh evidence changed");

  await card.getByRole("button", { name: "Review", exact: true }).click();

  await expect(page.locator(".tabs button.active")).toHaveText("theses");
  await expect(page.getByTestId("workflow-strip")).toContainText("OKTA");
  const placement = page.getByTestId("thesis-placement-strip");
  await expect(placement).toContainText("Active Universe");
  await expect(placement).toContainText("Universe T2");
  await expect(placement).toContainText("Core");
});

test("watchlist add form posts symbol and refreshes members", async ({ page }) => {
  const calls = await mockApi(page);
  await page.goto("/");

  await page.locator(".wl-row").filter({ hasText: "Core" }).click();
  await page.locator(".wl-add-sym input").fill("2454.tw");
  await page.locator(".wl-add-sym input").press("Enter");

  await expect.poll(() => calls.addedSymbols).toContainEqual("2454.TW");
  await expect(page.locator(".wl-mem").filter({ hasText: "2454.TW" }).first()).toBeVisible();
});

test("watchlist rows show thesis state and direction", async ({ page }) => {
  await mockApi(page);
  await page.goto("/");

  await page.locator(".wl-row").filter({ hasText: "Core" }).click();
  const row = page.locator(".wl-mem").filter({ hasText: "OKTA" }).first();

  await expect(row).toContainText("forming");
  await expect(row).toContainText("bull");
  await expect(row).toContainText("extended");
  await expect(row).toContainText("avoid chase");
  await expect(row.locator(".badge.fresh-stale")).toHaveText("stale");
  await expect(row.locator(".badge.att-open")).toHaveText("1");
  await expect(row.locator(".badge.theme")).toContainText("AI Compute Infrastructure");
  await expect(row).toContainText("+27% 200D");
});

test("watchlist filters combine thesis, technical, freshness, attention, and theme", async ({ page }) => {
  await mockApi(page);
  await page.goto("/");

  const universe = page.locator(".wl-row").filter({ hasText: "Universe" });
  await universe.click();
  await page.getByLabel("Thesis direction filter").selectOption("up");
  await page.getByLabel("Technical filter").selectOption("extended");
  if (await page.locator(".wl-mem").filter({ hasText: "OKTA" }).count() === 0) {
    await universe.click();
  }

  await expect(universe).toContainText("1/3");
  await expect(page.locator(".wl-mem").filter({ hasText: "OKTA" })).toBeVisible();
  await expect(page.locator(".wl-mem").filter({ hasText: "MSFT" })).toHaveCount(0);
  await expect(page.locator(".wl-mem").filter({ hasText: "NVDA" })).toHaveCount(0);

  await page.getByRole("button", { name: "reset" }).click();
  await page.getByLabel("Freshness filter").selectOption("stale_missing");
  await expect(universe).toContainText("2/3");
  await expect(page.locator(".wl-mem").filter({ hasText: "MSFT" })).toBeVisible();
  await expect(page.locator(".wl-mem").filter({ hasText: "OKTA" })).toBeVisible();
  await expect(page.locator(".wl-mem").filter({ hasText: "NVDA" })).toHaveCount(0);

  await page.getByRole("button", { name: "reset" }).click();
  await page.getByLabel("Attention filter").selectOption("open");
  await expect(universe).toContainText("1/3");
  await expect(page.locator(".wl-mem").filter({ hasText: "OKTA" })).toBeVisible();
  await expect(page.locator(".wl-mem").filter({ hasText: "MSFT" })).toHaveCount(0);
  await expect(page.locator(".wl-mem").filter({ hasText: "NVDA" })).toHaveCount(0);

  await page.getByRole("button", { name: "reset" }).click();
  await expect(page.getByLabel("Parent brain theme filter")).toContainText("AI Compute Infrastructure");
  await page.getByLabel("Parent brain theme filter").selectOption("ai_compute_infrastructure");
  await expect(universe).toContainText("2/3");
  await expect(page.locator(".wl-mem").filter({ hasText: "OKTA" })).toBeVisible();
  await expect(page.locator(".wl-mem").filter({ hasText: "NVDA" })).toBeVisible();
  await expect(page.locator(".wl-mem").filter({ hasText: "MSFT" })).toHaveCount(0);
});

test("decisions tab shows positions and posts manual exit fills", async ({ page }) => {
  const calls = await mockApi(page);
  await page.goto("/");

  await page.locator(".symbol-box input").fill("OKTA");
  await page.locator(".symbol-box input").press("Enter");
  await page.locator(".right").getByRole("button", { name: "decisions" }).click();

  const position = page.locator(".positions li").filter({ hasText: "12" });
  await expect(position).toContainText("long");
  await expect(position).toContainText("@ $88.00");
  await expect(position).toContainText("$96");

  await position.getByRole("button", { name: "exit ↓" }).click();
  await expect(page.getByLabel("Action")).toHaveValue("exit");
  await expect(page.getByLabel("Qty")).toHaveValue("12");
  await page.getByLabel("Fill price").fill("97");
  await page.getByLabel("Human conviction").selectOption("medium");
  await page.getByLabel("Decision reason").fill("Taking profit after thesis review.");
  await page.locator(".decform").getByRole("button", { name: "Submit" }).click();

  await expect.poll(() => calls.decisionBody).toMatchObject({
    thesis_id: "12ceaea3-9df3-416a-bfe5-107d3233dd59",
    action: "exit",
    user_choice: "confirmed",
    human_conviction: "medium",
    reason: "Taking profit after thesis review.",
    chart_range_seen: "ALL 1D",
    sizing: { side: "long", instrument: "equity", thesis_direction: "up" },
    manual_fill: {
      position_id: "9b496f4d-cbb8-4bb5-bd41-9766f8f962f2",
      side: "long",
      instrument: "equity",
      qty: 12,
      price: 97,
      fees: 0,
    },
  });
});

test("decision form requires disagreement reason for skip decisions", async ({ page }) => {
  const calls = await mockApi(page);
  await page.goto("/symbol/OKTA");

  await page.locator(".bottom-tabs").getByRole("button", { name: "decisions" }).click();
  await page.getByRole("button", { name: "Submit" }).click();

  await expect(page.getByText("choose why you disagree")).toBeVisible();

  await page.getByLabel("Why").selectOption("valuation_priced");
  await page.getByLabel("Detail").fill("Story is true, but the chart already reflects it.");
  await page.getByRole("button", { name: "Submit" }).click();

  await expect(page.getByText("choose human conviction")).toBeVisible();

  await page.getByLabel("Human conviction").selectOption("low");
  await page.getByLabel("Decision reason").fill("Technically extended despite useful narrative.");
  await page.getByRole("button", { name: "Submit" }).click();

  await expect.poll(() => calls.decisionBody).toMatchObject({
    thesis_id: "12ceaea3-9df3-416a-bfe5-107d3233dd59",
    action: "skip",
    user_choice: "deferred",
    disagreement_reason: "valuation_priced",
    disagreement_detail: "Story is true, but the chart already reflects it.",
    human_conviction: "low",
    reason: "Technically extended despite useful narrative.",
  });
});

test("decisions tab opens decision replay snapshot", async ({ page }) => {
  await mockApi(page);
  await page.goto("/symbol/OKTA");

  await page.locator(".right").getByRole("button", { name: "decisions" }).click();
  await page.locator(".decisions li").filter({ hasText: "skip" }).getByRole("button", { name: "replay" }).click();

  const replay = page.locator(".decision-replay");
  await expect(replay).toContainText("Decision replay");
  await expect(replay).toContainText("OKTA");
  await expect(replay).toContainText("v1 · forming · up");
  await expect(replay).toContainText("v2");
  await expect(replay).toContainText("64");
  await expect(replay).toContainText("ALL 1D");
  await expect(replay).toContainText("pass");
  await expect(replay).toContainText("disagreement: valuation priced");
  await expect(replay).toContainText("human conviction: low");
  await expect(replay).toContainText("Technically extended despite useful narrative.");
  await expect(replay).toContainText("system confidence medium");
  await expect(replay).toContainText("OKTA customer deployment article supports consolidation demand");
});

test("discovery pool rows show thesis state and direction", async ({ page }) => {
  await mockApi(page);
  await page.goto("/");

  await page.locator(".wl-row").filter({ hasText: "Discovery pool" }).click();
  const row = page.locator(".wl-mem").filter({ hasText: "OKTA" }).first();

  await expect(row).toContainText("forming");
  await expect(row).toContainText("bull");
});

test("theses tab renders current thesis despite duplicate history rows", async ({ page }) => {
  await mockApi(page);
  await page.goto("/");

  await page.locator(".symbol-box input").fill("OKTA");
  await page.locator(".symbol-box input").press("Enter");
  await page.getByRole("button", { name: "theses" }).click();

  await expect(page.getByText("Identity platform consolidation can improve growth durability.")).toBeVisible();
  await expect(page.getByText("freshness 42%")).toBeVisible();
  await expect(page.getByText("confidence capped at low")).toBeVisible();
  await expect(page.getByText("context: narrative context is stale")).toBeVisible();
  await expect(page.getByText("Linked evidence")).toBeVisible();
  const parentThemes = page.locator(".parent-theme-strip");
  await expect(parentThemes).toContainText("AI Compute Infrastructure");
  await expect(parentThemes).toContainText("theme · candidate · mixed · 50% fit");
  await expect(parentThemes).toContainText("Identity security expression of AI infrastructure budget priority.");
  await expect(page.getByText("OKTA customer deployment article supports consolidation demand")).toBeVisible();
  await expect(page.getByText(/weight 90/)).toBeVisible();
  await expect(page.getByText(/polarity \+0\.60/)).toBeVisible();
  await expect(page.getByText("Version history")).toBeVisible();
  await expect(page.getByText("smoketest duplicate")).toHaveCount(2);
});
