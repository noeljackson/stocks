import { expect, type Page, type Route, test } from "@playwright/test";

type Calls = {
  candleUrls: URL[];
  confirmBody: unknown | null;
  addedSymbols: string[];
};

type MockWatchlistMember = {
  watchlist_id: string;
  symbol: string;
  added_at: string;
  added_by: string;
  latest_thesis_id?: string | null;
  thesis_state?: string | null;
  thesis_direction?: string | null;
  open_theses?: number;
};

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

async function mockApi(page: Page): Promise<Calls> {
  const calls: Calls = { candleUrls: [], confirmBody: null, addedSymbols: [] };
  let attentionOpen = true;
  const watchlistMembers: MockWatchlistMember[] = [{
    watchlist_id: "wl-core",
    symbol: "OKTA",
    added_at: "2026-01-01T00:00:00Z",
    added_by: "seed",
    latest_thesis_id: "12ceaea3-9df3-416a-bfe5-107d3233dd59",
    thesis_state: "forming",
    thesis_direction: "up",
    open_theses: 1,
  }];

  await page.route("**/api/**", async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const path = url.pathname;

    if (path === "/api/stream") {
      await route.fulfill({ status: 200, contentType: "text/event-stream", body: ":\n\n" });
      return;
    }
    if (path === "/api/alerts") {
      await json(route, []);
      return;
    }
    if (path === "/api/regime") {
      await json(route, { regime: "neutral", capitulation: false, indicators: {}, as_of: "2026-06-01T00:00:00Z" });
      return;
    }
    if (path === "/api/tickers") {
      await json(route, [
        { symbol: "MSFT", cluster_id: "ai", cluster_name: "AI infrastructure", tier: 1, options_eligible: true, domain_fit: 91, added_at: "2026-01-01T00:00:00Z", open_theses: 0, latest_thesis_id: null, thesis_state: null, thesis_direction: null },
        { symbol: "OKTA", cluster_id: "identity", cluster_name: "Identity", tier: 2, options_eligible: true, domain_fit: 77, added_at: "2026-01-01T00:00:00Z", open_theses: 1, latest_thesis_id: "12ceaea3-9df3-416a-bfe5-107d3233dd59", thesis_state: "forming", thesis_direction: "up" },
        { symbol: "NVDA", cluster_id: "ai", cluster_name: "AI infrastructure", tier: 1, options_eligible: true, domain_fit: 96, added_at: "2026-01-01T00:00:00Z", open_theses: 0, latest_thesis_id: null, thesis_state: null, thesis_direction: null },
      ]);
      return;
    }
    if (path === "/api/calibration") {
      await json(route, { predictions_total: 3, outcomes_scored: 0, mean_brier: null, mean_lead_time_days: null, median_lead_time_days: null });
      return;
    }
    if (path === "/api/brain") {
      await json(route, {
        as_of: "2026-06-01T00:00:00Z",
        market_state: { regime: "neutral", capitulation: false, indicators: {}, as_of: "2026-06-01T00:00:00Z" },
        macro: {
          id: "d29d2f1d-7467-45ca-9f1e-1243923c94aa",
          scope: "macro",
          key: "macro_regime",
          name: "Macro Regime",
          state: "forming",
          direction: "neutral",
          summary: "Macro posture is neutral until breadth and rates confirm a stronger view.",
          core_claim: "Ticker conviction should respect the top-down risk regime.",
          why_now: null,
          evidence: [],
          invalidation_conditions: [],
          beneficiaries: [],
          losers: [],
          open_questions: ["Refresh FRED macro series"],
          missing_evidence: ["fred_macro", "market_breadth"],
          source_ref: {},
          freshness_target_minutes: 720,
          last_evaluated_at: null,
          version: 1,
          created_at: "2026-06-01T00:00:00Z",
          updated_at: "2026-06-01T00:00:00Z",
          freshness: "missing",
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
          summary: "AI capex remains the parent theme, but ticker selection must separate leaders from challengers.",
          core_claim: "The edge is finding where adoption evidence diffuses slower than price consensus.",
          why_now: "Product/customer adoption evidence is still arriving.",
          evidence: [],
          invalidation_conditions: [],
          beneficiaries: ["NVDA", "AMD", "MU"],
          losers: [],
          open_questions: ["Which challengers have real customer traction?"],
          missing_evidence: ["theme_estimate_revision_breadth"],
          source_ref: {},
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
        open_theses: 0,
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
        rank_reasons: ["volume anomaly", "strong signal value", "high-confidence watchlist fit"],
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
        open_theses: 1,
      }]);
      return;
    }
    if (path === "/api/attention") {
      await json(route, attentionOpen ? [{
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
      }] : []);
      return;
    }
    if (path === "/api/discovery/candidates/44/confirm" && request.method() === "POST") {
      calls.confirmBody = await request.postDataJSON();
      attentionOpen = false;
      await route.fulfill({ status: 204 });
      return;
    }
    if (path === "/api/ticker-context") {
      const symbol = url.searchParams.get("symbol");
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
      await json(route, {
        symbol,
        as_of: "2026-06-01T00:00:00Z",
        active_ticker: true,
        status: symbol === "OKTA" ? "fresh" : "due",
        next_action: symbol === "OKTA" ? "monitor" : "reevaluate_thesis",
        reason: symbol === "OKTA"
          ? "brain loop is current for this symbol"
          : "open thesis is past the re-evaluation window",
        freshness_target_minutes: 30,
        sources: [
          {
            source: "price",
            status: "fresh",
            last_changed_at: "2026-06-01T00:00:00Z",
            last_checked_at: "2026-06-01T00:00:00Z",
            max_age_minutes: 30,
          },
          {
            source: "news",
            status: "fresh",
            last_changed_at: "2026-06-01T00:00:00Z",
            last_checked_at: "2026-06-01T00:00:00Z",
            max_age_minutes: 30,
          },
          {
            source: "thesis",
            status: symbol === "OKTA" ? "fresh" : "stale",
            last_changed_at: "2026-05-31T23:00:00Z",
            last_checked_at: "2026-05-31T23:00:00Z",
            max_age_minutes: 30,
          },
        ],
        evidence: { rows: 4, open: 0, blocking: 0, due: 0 },
        attention: { open: symbol === "OKTA" ? 0 : 1, by_kind: [] },
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
        state: "forming",
        edge_rationale: "Identity platform consolidation can improve growth durability.",
        bull_case: "Growth stabilizes.",
        bear_case: "Execution slips.",
        forecast: { direction: "up", target: 130, deadline_at: "2026-12-31" },
        conviction_conditions: [],
        trigger_conditions: [],
        invalidation_conditions: [],
        fulfillment_conditions: [],
        conviction_tier: "monitoring",
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
        substance: null,
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
      await json(route, [{
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
        source_ref: { counts: { research_evidence: 2 }, fetch_actions: ["gdelt_doc_search", "bing_news_rss_search"] },
        created_at: "2026-06-01T00:00:00Z",
        updated_at: "2026-06-01T00:00:00Z",
        satisfied_at: "2026-06-01T00:00:00Z",
      }]);
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
    if (path === "/api/decisions") {
      await json(route, []);
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
    if (path === "/api/system-status") {
      await json(route, {
        ingest: {},
        discovery: { last_pass_at: null, open_candidates: 1, by_signal: [], pool_size: 0 },
        cognition: { contexts_24h: 1, contexts_total_symbols: 3, thesis_by_state: [] },
        attention: { open_items: attentionOpen ? 1 : 0, by_kind: [] },
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
  await expect(page.getByTestId("chart-interval-status")).toContainText("1D");
  await expect(page.getByTestId("chart-interval-status")).toContainText("ALL");
  await expect(page.getByText("SMA 200D")).toBeVisible();
  await expect(page.getByTestId("rsi-legend")).toHaveText("RSI 14");
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

test("theses tab lists declined thesis attempts with reasons", async ({ page }) => {
  await mockApi(page);
  await page.goto("/");

  await page.getByRole("button", { name: "theses" }).click();

  await expect(page.getByText("Declined thesis attempts")).toBeVisible();
  await expect(page.getByText("Context contains no non-consensus edge yet")).toBeVisible();
  await expect(page.getByText("No thesis attempts")).toHaveCount(0);
});

test("overview explains selected symbol brain status and stale source", async ({ page }) => {
  await mockApi(page);
  await page.goto("/");

  const brain = page.locator(".brain-card");
  await expect(brain).toBeVisible();
  await expect(brain).toContainText("Brain");
  await expect(brain).toContainText("due");
  await expect(brain).toContainText("reevaluate thesis");
  await expect(brain).toContainText("open thesis is past the re-evaluation window");
  await expect(brain).toContainText("4 rows, 0 open");
  await expect(brain).toContainText("price");
  await expect(brain).toContainText("thesis");
  await expect(brain).toContainText("stale");
});

test("brain tab shows macro and theme theses with linked tickers", async ({ page }) => {
  await mockApi(page);
  await page.goto("/");

  await page.getByRole("button", { name: "brain" }).click();

  await expect(page.locator(".brain-topline")).toContainText("2 active");
  await expect(page.locator(".macro-theme")).toContainText("Macro Regime");
  await expect(page.locator(".macro-theme")).toContainText("fred_macro");

  const theme = page.locator(".brain-theme").filter({ hasText: "AI Compute Infrastructure" });
  await expect(theme).toContainText("AI capex remains the parent theme");
  await expect(theme).toContainText("Core");
  await expect(theme.getByRole("button", { name: /NVDA leader/ })).toBeVisible();
  await expect(theme.getByRole("button", { name: /OKTA/ })).toContainText("forming");
});

test("symbol routes deep-link selected ticker and keep navigation state", async ({ page }) => {
  await mockApi(page);
  await page.goto("/symbol/NVDA");

  await expect(page.locator(".symbol-box input")).toHaveValue("NVDA");
  await expect(page).toHaveURL(/\/symbol\/NVDA$/);

  await page.locator(".wl-row").filter({ hasText: "Core" }).click();
  await page.locator(".wl-mem").filter({ hasText: "OKTA" }).getByRole("button", { name: "OKTA" }).click();

  await expect(page.locator(".symbol-box input")).toHaveValue("OKTA");
  await expect(page).toHaveURL(/\/symbol\/OKTA$/);

  await page.goBack();

  await expect(page.locator(".symbol-box input")).toHaveValue("NVDA");
  await expect(page).toHaveURL(/\/symbol\/NVDA$/);
});

test("evidence tab shows retrieved research sources", async ({ page }) => {
  await mockApi(page);
  await page.goto("/");

  await page.getByRole("button", { name: "evidence" }).click();

  const requirement = page.locator(".evidence-card").filter({ hasText: "product/theme web research" }).first();
  await expect(requirement.locator("strong")).toHaveText("web research");
  await expect(page.getByText("Research sources")).toBeVisible();
  await expect(page.getByText("AMD MI355X production deployment expands")).toBeVisible();
  await expect(page.getByText("AMD MI355X deployment benchmark adoption")).toBeVisible();
});

test("discovery tab shows candidate ranking reasons", async ({ page }) => {
  await mockApi(page);
  await page.goto("/");

  await page.getByRole("button", { name: /discovery/ }).click();

  const card = page.locator(".disc-card").filter({ hasText: "NVDA" });
  await expect(card).toContainText("highest 82");
  await expect(card).toContainText("volume anomaly");
  await expect(card).toContainText("high-confidence watchlist fit");
});

test("attention Confirm posts selected watchlist memberships", async ({ page }) => {
  const calls = await mockApi(page);
  await page.goto("/");

  const card = page.locator(".att-card").filter({ hasText: "NVDA" }).first();
  await expect(card).toBeVisible();
  await expect(card).toContainText("2.4x volume vs 200-day SMA");
  await expect(page.locator(".att-section-head").filter({ hasText: "ready for review" })).toContainText("operator owns next step");

  await card.getByRole("button", { name: "Confirm" }).click();

  await expect.poll(() => calls.confirmBody).toEqual({ watchlist_ids: ["wl-core"] });
  await expect(page.getByText("No open attention. The system is quiet.")).toBeVisible();
});

test("watchlist add form posts ticker and refreshes members", async ({ page }) => {
  const calls = await mockApi(page);
  await page.goto("/");

  await page.locator(".wl-row").filter({ hasText: "Core" }).click();
  await page.locator(".wl-add-sym input").fill("NVDA");
  await page.locator(".wl-add-sym input").press("Enter");

  await expect.poll(() => calls.addedSymbols).toContainEqual("NVDA");
  await expect(page.locator(".wl-mem").filter({ hasText: "NVDA" }).first()).toBeVisible();
});

test("watchlist rows show thesis state and direction", async ({ page }) => {
  await mockApi(page);
  await page.goto("/");

  await page.locator(".wl-row").filter({ hasText: "Core" }).click();
  const row = page.locator(".wl-mem").filter({ hasText: "OKTA" }).first();

  await expect(row).toContainText("forming");
  await expect(row).toContainText("bull");
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
  await expect(page.getByText("Version history")).toBeVisible();
  await expect(page.getByText("smoketest duplicate")).toHaveCount(2);
});
