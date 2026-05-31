<script lang="ts">
  import { onMount } from "svelte";
  import {
    ackAlert,
    addToWatchlist,
    confirmCandidate,
    createWatchlist,
    fetchAlerts,
    fetchCalibration,
    fetchPendingCandidates,
    fetchRegime,
    fetchTheses,
    fetchTickerContext,
    fetchTickers,
    fetchWatchlistMembers,
    fetchWatchlists,
    postDecision,
    rejectCandidate,
    removeFromWatchlist,
    subscribe,
    type Alert,
    type Calibration,
    type MarketState,
    type PendingCandidate,
    type StreamEvent,
    type ThesisDetail,
    type Ticker,
    type TickerContext,
    type Watchlist,
    type WatchlistMember,
  } from "./lib/api";
  import ContextPanel from "./lib/ContextPanel.svelte";
  import ThesisDetails from "./lib/ThesisDetails.svelte";

  type View = "feed" | "tickers" | "watchlists" | "discovery" | "decisions";
  let view = $state<View>("feed");

  let regime = $state<MarketState | null>(null);
  let calibration = $state<Calibration | null>(null);
  let tickers = $state<Ticker[]>([]);
  let alerts = $state<Alert[]>([]);
  let live = $state<StreamEvent[]>([]);
  let connected = $state(false);
  let error = $state<string | null>(null);

  // Watchlists state (#54)
  let watchlists = $state<Watchlist[]>([]);
  let activeWatchlistId = $state<string | null>(null);
  let watchlistMembers = $state<WatchlistMember[]>([]);
  let newListName = $state("");
  let newListDescription = $state("");
  let addSymbolFor = $state<Record<string, string>>({});

  // Discovery review state (#54 phase B / #55)
  let pending = $state<PendingCandidate[]>([]);
  // candidate.id -> set of selected watchlist UUIDs to confirm to
  let chosenLists = $state<Record<number, Record<string, boolean>>>({});

  async function refreshPending() {
    try {
      pending = await fetchPendingCandidates();
      // Seed the selection map: any proposed list with a known watchlist_id
      // gets pre-checked (medium/high confidence). Low-conf left unchecked.
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
    if (ids.length === 0) {
      error = "Pick at least one watchlist before confirming.";
      return;
    }
    try {
      await confirmCandidate(candId, ids);
      await Promise.all([refreshPending(), refreshWatchlists()]);
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

  async function refreshWatchlists() {
    try {
      watchlists = await fetchWatchlists();
      if (activeWatchlistId === null && watchlists.length > 0) {
        activeWatchlistId = watchlists[0].id;
      }
      if (activeWatchlistId) {
        watchlistMembers = await fetchWatchlistMembers(activeWatchlistId);
      }
    } catch (e) {
      error = String(e);
    }
  }

  async function pickWatchlist(id: string) {
    activeWatchlistId = id;
    try {
      watchlistMembers = await fetchWatchlistMembers(id);
    } catch (e) {
      error = String(e);
    }
  }

  async function submitNewList(e: Event) {
    e.preventDefault();
    if (!newListName.trim()) return;
    try {
      await createWatchlist({ name: newListName.trim(), description: newListDescription.trim() || undefined });
      newListName = "";
      newListDescription = "";
      await refreshWatchlists();
    } catch (err) {
      error = String(err);
    }
  }

  async function addMember(id: string) {
    const sym = (addSymbolFor[id] ?? "").trim().toUpperCase();
    if (!sym) return;
    try {
      await addToWatchlist(id, sym);
      addSymbolFor = { ...addSymbolFor, [id]: "" };
      await refreshWatchlists();
    } catch (err) {
      error = String(err);
    }
  }

  async function removeMember(id: string, symbol: string) {
    try {
      await removeFromWatchlist(id, symbol);
      await refreshWatchlists();
    } catch (err) {
      error = String(err);
    }
  }

  // Per-symbol expand state for the Tickers view (symbol → loaded theses or null while loading).
  let expanded = $state<Record<string, ThesisDetail[] | null | undefined>>({});
  // Parallel context load per-symbol; `undefined` = not loaded, `null` = no context.
  let contextBySymbol = $state<Record<string, TickerContext | null | undefined>>({});
  // Per-event expand state for the Feed views (use index or alert id as key).
  let liveOpen = $state<Record<number, boolean>>({});
  let alertOpen = $state<Record<number, boolean>>({});
  // Default: hide acknowledged alerts; toggle reveals everything.
  let showAcked = $state(false);

  async function ack(id: number) {
    try {
      await ackAlert(id);
      // Optimistic: remove from list locally; the next fetch confirms.
      alerts = alerts.filter((a) => a.id !== id || showAcked);
      if (showAcked) {
        alerts = alerts.map((a) => (a.id === id ? { ...a, acknowledged: true } : a));
      }
    } catch (e) {
      error = String(e);
    }
  }

  function toggleLive(i: number) {
    liveOpen = { ...liveOpen, [i]: !liveOpen[i] };
  }
  function toggleAlert(id: number) {
    alertOpen = { ...alertOpen, [id]: !alertOpen[id] };
  }

  async function toggleTicker(symbol: string) {
    if (expanded[symbol] !== undefined) {
      // collapse
      const { [symbol]: _, ...rest } = expanded;
      expanded = rest;
      return;
    }
    expanded = { ...expanded, [symbol]: null }; // loading
    // Fire both fetches in parallel — context and theses are independent.
    const [theses, ctx] = await Promise.all([
      fetchTheses(symbol).catch((e) => { error = String(e); return [] as ThesisDetail[]; }),
      fetchTickerContext(symbol).catch((e) => { error = String(e); return null; }),
    ]);
    expanded = { ...expanded, [symbol]: theses };
    contextBySymbol = { ...contextBySymbol, [symbol]: ctx };
  }

  // Decision form
  let decThesisId = $state("");
  let decAction = $state("skip");
  let decChoice = $state("deferred");
  let decStatus = $state<string | null>(null);

  function refreshAll() {
    fetchAlerts({ unacked: !showAcked }).then((a) => (alerts = a)).catch((e) => (error = String(e)));
    fetchRegime().then((r) => (regime = r)).catch((e) => (error = String(e)));
    fetchTickers().then((t) => (tickers = t)).catch((e) => (error = String(e)));
    fetchCalibration().then((c) => (calibration = c)).catch(() => {});
    refreshWatchlists();
    refreshPending();
  }

  // React when the user toggles showAcked.
  $effect(() => {
    fetchAlerts({ unacked: !showAcked }).then((a) => (alerts = a)).catch(() => {});
  });

  onMount(() => {
    refreshAll();
    const stop = subscribe(
      (e) => {
        live = [e, ...live].slice(0, 200);
        // Regime updates arrive via regime.state; refresh the top bar lazily.
        if (e.subject?.startsWith("regime.")) {
          fetchRegime().then((r) => (regime = r)).catch(() => {});
        }
        // Refresh recent alerts when a new one would have been persisted by the gateway.
        if (e.kind === "state_transition" || e.kind === "risk") {
          fetchAlerts({ unacked: !showAcked }).then((a) => (alerts = a)).catch(() => {});
        }
      },
      (open) => (connected = open),
    );
    return stop;
  });

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
    const d = new Date(s);
    return d.toLocaleTimeString();
  }

  async function submitDecision(e: Event) {
    e.preventDefault();
    decStatus = "sending…";
    try {
      await postDecision({
        thesis_id: decThesisId || undefined,
        action: decAction,
        user_choice: decChoice,
      });
      decStatus = "recorded ✓";
      setTimeout(() => (decStatus = null), 2500);
      decThesisId = "";
      refreshAll();
    } catch (err) {
      decStatus = `error: ${err}`;
    }
  }
</script>

<header>
  <h1>stocks <span class="muted">intelligence</span></h1>
  <div class="regime" title={regime ? `as of ${regime.as_of ?? "?"}` : ""}>
    <span class="dot" style="background:{regimeColor(regime?.regime)}"></span>
    <strong>{regime?.regime ?? "loading…"}</strong>
    {#if regime?.capitulation}
      <span class="capitulation">CAPITULATION</span>
    {/if}
    {#if regime && Object.keys(regime.indicators).length > 0}
      <span class="muted">
        {Object.entries(regime.indicators)
          .map(([k, v]) => `${k}=${Number(v).toFixed(2)}`)
          .join(" · ")}
      </span>
    {/if}
  </div>
  {#if calibration}
    <div class="calibration" title="Forward-only validation (SPEC §9). Brier=0 is perfect calibration; lead-time positive means alert preceded consensus.">
      <span class="muted">calibration</span>
      <strong>{calibration.outcomes_scored}</strong>/<span class="muted">{calibration.predictions_total}</span>
      {#if calibration.mean_brier !== null}
        <span class="muted">brier</span>
        <strong>{calibration.mean_brier.toFixed(3)}</strong>
      {/if}
      {#if calibration.median_lead_time_days !== null}
        <span class="muted">median lead</span>
        <strong>{calibration.median_lead_time_days.toFixed(1)}d</strong>
      {/if}
    </div>
  {/if}
  <span class="status" class:on={connected}>{connected ? "● live" : "○ offline"}</span>
</header>

<nav>
  <button class:active={view === "feed"} onclick={() => (view = "feed")}>Feed</button>
  <button class:active={view === "tickers"} onclick={() => (view = "tickers")}>
    Tickers <span class="badge">{tickers.length}</span>
  </button>
  <button class:active={view === "watchlists"} onclick={() => (view = "watchlists")}>
    Watchlists <span class="badge">{watchlists.length}</span>
  </button>
  <button class:active={view === "discovery"} onclick={() => (view = "discovery")}>
    Discovery <span class="badge">{pending.length}</span>
  </button>
  <button class:active={view === "decisions"} onclick={() => (view = "decisions")}>Decision</button>
</nav>

<main>
  {#if error}<p class="error">{error}</p>{/if}

  {#if view === "feed"}
    <section>
      <h2>Live <span class="muted">({live.length})</span></h2>
      {#if live.length === 0}
        <p class="muted">Waiting for events…</p>
      {/if}
      <ul class="feed">
        {#each live as e, i (i)}
          {@const p = (e.payload ?? {}) as Record<string, unknown>}
          <li class="expandable" onclick={() => toggleLive(i)}>
            <span class="caret">{liveOpen[i] ? "▾" : "▸"}</span>
            <span class="kind" style="color:{kindColor(e.kind, p)}">{e.kind}</span>
            <code>{e.subject}</code>
            {#if p.symbol}<strong>{p.symbol as string}</strong>{/if}
            {#if e.kind === "risk" && p.veto}
              <span class="badge danger">VETO {(p.reasons as string[])?.join(", ")}</span>
            {:else if e.kind === "risk" && p.kind === "goalpost_moved"}
              <span class="badge warning">GOALPOST {p.weakened ? "weakened" : "needs review"}</span>
              {#if p.loosened}<span class="muted">loosened: {(p.loosened as string[]).join(",")}</span>{/if}
              {#if p.dropped}<span class="muted">dropped: {(p.dropped as string[]).join(",")}</span>{/if}
            {:else if e.kind === "state_transition" && p.delta_notional}
              <span class="muted">Δ${Number(p.delta_notional).toLocaleString()}</span>
            {/if}
            {#if liveOpen[i]}
              <div class="event-detail">
                <h5>payload</h5>
                <pre>{JSON.stringify(p, null, 2)}</pre>
              </div>
            {/if}
          </li>
        {/each}
      </ul>
    </section>

    <section>
      <div class="section-hdr">
        <h2>
          {showAcked ? "All alerts" : "Open alerts"}
          <span class="muted">({alerts.length})</span>
        </h2>
        <label class="toggle">
          <input type="checkbox" bind:checked={showAcked} />
          show acknowledged
        </label>
      </div>
      {#if alerts.length === 0}
        <p class="muted">
          {showAcked ? "No alerts." : "All caught up. Toggle 'show acknowledged' to see history."}
        </p>
      {/if}
      <ul class="feed">
        {#each alerts as a (a.id)}
          {@const p = (a.payload ?? {}) as Record<string, unknown>}
          <li class="expandable" class:acked={a.acknowledged} onclick={() => toggleAlert(a.id)}>
            <span class="caret">{alertOpen[a.id] ? "▾" : "▸"}</span>
            <span class="kind" style="color:{kindColor(a.kind, p)}">{a.kind}</span>
            {#if a.symbol}<strong>{a.symbol}</strong>{/if}
            {#if p.veto}<span class="badge danger">VETO</span>{/if}
            {#if p.kind === "goalpost_moved"}<span class="badge warning">GOALPOST</span>{/if}
            {#if p.kind === "condition_stale"}<span class="badge warning">STALE</span>{/if}
            {#if p.reasons}<span class="muted">{(p.reasons as string[]).join(" · ")}</span>{/if}
            <span class="muted">{shortTs(a.created_at)}</span>
            {#if !a.acknowledged}
              <button class="ack-btn" onclick={(e) => { e.stopPropagation(); ack(a.id); }}
                      title="Mark seen / handled">
                ack
              </button>
            {:else}
              <span class="muted ack-mark">✓ acked</span>
            {/if}
            {#if alertOpen[a.id]}
              <div class="event-detail">
                <h5>alert #{a.id}</h5>
                <pre>{JSON.stringify({
                  thesis_id: a.thesis_id,
                  acknowledged: a.acknowledged,
                  payload: p,
                }, null, 2)}</pre>
              </div>
            {/if}
          </li>
        {/each}
      </ul>
    </section>
  {:else if view === "tickers"}
    <h2>Tracked tickers</h2>
    {#if tickers.length === 0}
      <p class="muted">No active tickers seeded. Run <code>make seed-demo</code> to populate sample data.</p>
    {/if}
    <p class="muted">Click a row to expand the thesis details (why we're tracking it, invalidation conditions, goalpost history).</p>
    <table>
      <thead>
        <tr>
          <th></th>
          <th>Symbol</th><th>Cluster</th><th>Tier</th>
          <th>Domain-fit</th><th>Options</th><th>Open theses</th>
        </tr>
      </thead>
      <tbody>
        {#each tickers as t (t.symbol)}
          {@const isOpen = expanded[t.symbol] !== undefined}
          <tr class="ticker-row" class:open={isOpen} onclick={() => toggleTicker(t.symbol)}>
            <td class="caret">{isOpen ? "▾" : "▸"}</td>
            <td><strong>{t.symbol}</strong></td>
            <td><span class="muted">{t.cluster_name ?? t.cluster_id}</span></td>
            <td>T{t.tier}</td>
            <td>{t.domain_fit !== null && t.domain_fit !== undefined ? Math.round(t.domain_fit) : "—"}</td>
            <td>{t.options_eligible ? "✓" : ""}</td>
            <td>{t.open_theses}</td>
          </tr>
          {#if isOpen}
            <tr class="detail-row">
              <td colspan="7">
                <!-- Context band — what the LLM has synthesized for this ticker -->
                {#if contextBySymbol[t.symbol] !== undefined}
                  <ContextPanel ctx={contextBySymbol[t.symbol] ?? null} symbol={t.symbol} />
                {/if}

                <!-- Theses for this ticker -->
                {#if expanded[t.symbol] === null}
                  <p class="muted">Loading…</p>
                {:else if expanded[t.symbol] && (expanded[t.symbol] as ThesisDetail[]).length === 0}
                  <p class="muted">No theses for <strong>{t.symbol}</strong> yet.
                  Run <code>make draft-thesis SYMBOL={t.symbol}</code> to ask the engine
                  to draft one against the context above.</p>
                {:else}
                  {#each expanded[t.symbol] as ThesisDetail[] as thesis (thesis.thesis_id)}
                    <ThesisDetails {thesis} />
                  {/each}
                {/if}
              </td>
            </tr>
          {/if}
        {/each}
      </tbody>
    </table>
  {:else if view === "watchlists"}
    <h2>Watchlists</h2>
    <p class="muted">User-curated multi-list ticker organization. Same ticker can live on many lists.</p>

    <form onsubmit={submitNewList} class="new-list">
      <input bind:value={newListName} placeholder="new list name" />
      <input bind:value={newListDescription} placeholder="description (optional)" />
      <button type="submit">Create</button>
    </form>

    <div class="wl-grid">
      <aside class="wl-side">
        {#each watchlists as w (w.id)}
          <button
            class="wl-pick"
            class:active={activeWatchlistId === w.id}
            onclick={() => pickWatchlist(w.id)}
            style={w.color ? `border-left: 3px solid ${w.color}` : ""}
          >
            <strong>{w.name}</strong>
            <span class="muted">{w.member_count}</span>
            {#if w.is_system}<span class="badge">system</span>{/if}
            {#if w.description}<div class="muted desc">{w.description}</div>{/if}
          </button>
        {/each}
      </aside>

      <section class="wl-main">
        {#if !activeWatchlistId}
          <p class="muted">Pick a list on the left, or create a new one above.</p>
        {:else}
          {@const active = watchlists.find((w) => w.id === activeWatchlistId)}
          <h3>{active?.name ?? "—"} <span class="muted">({watchlistMembers.length} members)</span></h3>

          <form
            onsubmit={(e) => { e.preventDefault(); if (activeWatchlistId) addMember(activeWatchlistId); }}
            class="add-symbol"
          >
            <input
              placeholder="add ticker, e.g. AAPL"
              value={addSymbolFor[activeWatchlistId] ?? ""}
              oninput={(e) => {
                if (activeWatchlistId) {
                  addSymbolFor = { ...addSymbolFor, [activeWatchlistId]: (e.target as HTMLInputElement).value };
                }
              }}
            />
            <button type="submit">Add</button>
          </form>

          {#if watchlistMembers.length === 0}
            <p class="muted">No members yet.</p>
          {:else}
            <table class="member-table">
              <thead><tr><th>Symbol</th><th>Added</th><th>By</th><th></th></tr></thead>
              <tbody>
                {#each watchlistMembers as m (m.symbol)}
                  <tr>
                    <td><strong>{m.symbol}</strong></td>
                    <td><span class="muted">{shortTs(m.added_at)}</span></td>
                    <td><span class="muted">{m.added_by ?? "—"}</span></td>
                    <td>
                      <button class="ack-btn"
                              onclick={() => activeWatchlistId && removeMember(activeWatchlistId, m.symbol)}>
                        remove
                      </button>
                    </td>
                  </tr>
                {/each}
              </tbody>
            </table>
          {/if}
        {/if}
      </section>
    </div>
  {:else if view === "discovery"}
    <h2>Discovery review <span class="muted">({pending.length} pending)</span></h2>
    <p class="muted">
      Cheap-wide signals (volume anomalies, base breakouts, …) propose tickers.
      The LLM suggests which existing watchlist(s) each one fits. You confirm or reject.
    </p>
    {#if pending.length === 0}
      <p class="muted">
        Nothing pending. Run <code>make run-discovery</code> to scan, then
        <code>make classify-candidates</code> to populate suggestions.
      </p>
    {/if}
    <ul class="disc-list">
      {#each pending as c (c.id)}
        <li class="disc-card">
          <div class="disc-hdr">
            <strong>{c.symbol}</strong>
            <span class="badge">{c.signal_name}</span>
            {#if c.signal_value !== null}
              <span class="muted">value {c.signal_value.toFixed(3)}</span>
            {/if}
            <span class="muted">{shortTs(c.proposed_at)}</span>
          </div>
          {#if c.reasoning}
            <p class="muted disc-reasoning">{c.reasoning}</p>
          {/if}
          {#if c.proposed_lists.length === 0 && !c.suggested_new_list}
            <p class="muted">No LLM classification yet — run <code>make classify-candidates</code>.</p>
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
                    <span class="wl-name">{p.watchlist_name}</span>
                    <span class="badge conf-{p.confidence}">{p.confidence}</span>
                    <span class="muted disc-rat">{p.rationale}</span>
                  </label>
                {:else}
                  <div class="disc-pick disabled">
                    <span class="wl-name">{p.watchlist_name}</span>
                    <span class="badge warning">list missing</span>
                    <span class="muted disc-rat">{p.rationale}</span>
                  </div>
                {/if}
              {/each}
            </div>
          {/if}
          {#if c.suggested_new_list}
            <div class="disc-newlist">
              <span class="badge">propose new</span>
              <strong>{c.suggested_new_list.name}</strong>
              <span class="muted">— {c.suggested_new_list.description}</span>
              <div class="muted disc-rat">{c.suggested_new_list.rationale}</div>
            </div>
          {/if}
          <div class="disc-actions">
            <button onclick={() => confirmOne(c.id)}>Confirm → selected lists</button>
            <button class="reject" onclick={() => rejectOne(c.id)}>Reject</button>
          </div>
        </li>
      {/each}
    </ul>
  {:else}
    <h2>Record a decision</h2>
    <p class="muted">
      Logs your choice on a thesis to the decision table and emits
      <code>decision.recorded</code>.
    </p>
    <form onsubmit={submitDecision} class="decform">
      <label>
        Thesis ID
        <input bind:value={decThesisId} placeholder="(leave blank for ad-hoc)" />
      </label>
      <label>
        Action
        <select bind:value={decAction}>
          <option>enter</option>
          <option>exit</option>
          <option>skip</option>
          <option>resize</option>
        </select>
      </label>
      <label>
        User choice
        <select bind:value={decChoice}>
          <option>confirmed</option>
          <option>rejected</option>
          <option>deferred</option>
        </select>
      </label>
      <button type="submit">Submit</button>
      {#if decStatus}<span class="muted">{decStatus}</span>{/if}
    </form>
  {/if}
</main>

<style>
  header {
    display: flex; align-items: center; gap: 1rem; flex-wrap: wrap;
    padding-bottom: 0.75rem; border-bottom: 1px solid #1f2733; margin-bottom: 1rem;
  }
  h1 { margin: 0; font-size: 1.1rem; }
  .regime { display: flex; align-items: center; gap: 0.5rem; }
  .regime .dot {
    width: 0.6rem; height: 0.6rem; border-radius: 50%; display: inline-block;
  }
  .calibration {
    display: flex; align-items: baseline; gap: 0.35rem; font-size: 0.85rem;
    padding: 0.25rem 0.6rem; background: rgba(180, 190, 254, 0.05);
    border: 1px solid #1f2733; border-radius: 4px;
  }
  .calibration strong { color: #cdd6f4; }
  .regime .capitulation {
    background: rgba(243, 139, 168, 0.2); color: rgb(243, 139, 168);
    padding: 0.1rem 0.4rem; border-radius: 4px; font-size: 0.7rem; letter-spacing: 0.05em;
  }
  .status { margin-left: auto; font-size: 0.8rem; color: #f38ba8; }
  .status.on { color: #a6e3a1; }

  nav { display: flex; gap: 0.5rem; margin-bottom: 1rem; }
  button {
    background: #1b2230; color: #cdd6f4; border: 1px solid #2a3548;
    border-radius: 6px; padding: 0.35rem 0.8rem; cursor: pointer; font: inherit;
  }
  button.active { background: #2a3548; border-color: #45567a; }
  .badge {
    display: inline-block; padding: 0.05rem 0.4rem; border-radius: 4px;
    background: #1f2733; font-size: 0.7rem; margin-left: 0.3rem;
  }
  .badge.danger { background: rgba(243, 139, 168, 0.18); color: rgb(243, 139, 168); }
  .badge.warning { background: rgba(249, 226, 175, 0.15); color: rgb(249, 226, 175); }

  section { margin-bottom: 1.5rem; }
  h2 { font-size: 0.95rem; color: #bac2de; margin: 0 0 0.5rem 0; }

  .feed {
    list-style: none; padding: 0; display: flex; flex-direction: column; gap: 0.25rem;
  }
  .feed li {
    background: #11161f; border: 1px solid #1f2733; border-radius: 6px;
    padding: 0.4rem 0.6rem; display: flex; flex-wrap: wrap; gap: 0.4rem; align-items: baseline;
  }
  .kind { font-size: 0.7rem; text-transform: uppercase; letter-spacing: 0.05em; }
  code { background: #11161f; padding: 0.05rem 0.3rem; border-radius: 4px; font-size: 0.85rem; }
  .muted { color: #6c7086; font-size: 0.8rem; }
  .error { color: #f38ba8; background: rgba(243,139,168,0.1); padding: 0.5rem; border-radius: 6px; }

  table { width: 100%; border-collapse: collapse; }
  th, td {
    text-align: left; padding: 0.35rem 0.5rem; border-bottom: 1px solid #1f2733;
  }
  th { color: #bac2de; font-weight: 500; font-size: 0.8rem; }
  .ticker-row { cursor: pointer; transition: background 0.1s; }
  .ticker-row:hover { background: rgba(137, 180, 250, 0.05); }
  .ticker-row.open { background: rgba(137, 180, 250, 0.08); }
  .caret { color: #6c7086; font-size: 0.8rem; width: 1.2rem; }
  .detail-row td { padding: 0; border-bottom: 1px solid #1f2733; }
  .feed li.expandable { cursor: pointer; }
  .feed li.expandable:hover { background: rgba(137, 180, 250, 0.04); }
  .event-detail {
    width: 100%; margin-top: 0.4rem; padding-top: 0.4rem;
    border-top: 1px dashed #2a3548;
  }
  .event-detail pre {
    background: #0a0d14; padding: 0.5rem; border-radius: 4px;
    font-size: 0.75rem; overflow-x: auto; color: #bac2de; margin: 0.25rem 0;
  }
  .event-detail h5 { margin: 0.4rem 0 0.2rem 0; font-size: 0.75rem; color: #bac2de; text-transform: uppercase; letter-spacing: 0.05em; }
  .section-hdr { display: flex; align-items: center; justify-content: space-between; flex-wrap: wrap; gap: 0.5rem; margin-bottom: 0.5rem; }
  .toggle { display: flex; align-items: center; gap: 0.35rem; font-size: 0.8rem; color: #6c7086; cursor: pointer; }
  .ack-btn {
    margin-left: auto; padding: 0.1rem 0.45rem; font-size: 0.7rem;
    background: #1b2230; color: #cdd6f4; border: 1px solid #2a3548; border-radius: 4px; cursor: pointer;
  }
  .ack-btn:hover { background: #2a3548; }
  .ack-mark { margin-left: auto; }
  .feed li.acked { opacity: 0.5; }

  .decform {
    display: grid; grid-template-columns: 1fr 1fr; gap: 0.75rem; max-width: 600px;
    background: #11161f; border: 1px solid #1f2733; padding: 1rem; border-radius: 6px;
  }
  .decform label { display: flex; flex-direction: column; gap: 0.25rem; font-size: 0.85rem; }
  .decform input, .decform select {
    background: #0a0d14; color: #cdd6f4; border: 1px solid #2a3548; border-radius: 4px;
    padding: 0.35rem; font: inherit;
  }
  .decform button { grid-column: 1; }
  .decform .muted { grid-column: 2; align-self: end; }

  /* Watchlists view (#54) */
  .new-list { display: flex; gap: 0.5rem; margin-bottom: 1rem; flex-wrap: wrap; }
  .new-list input {
    background: #0a0d14; color: #cdd6f4; border: 1px solid #2a3548; border-radius: 4px;
    padding: 0.35rem 0.5rem; font: inherit; min-width: 140px;
  }
  .new-list input:first-child { flex: 0 0 200px; }
  .new-list input:nth-child(2) { flex: 1; min-width: 220px; }

  .wl-grid {
    display: grid; grid-template-columns: 260px 1fr; gap: 1rem;
  }
  @media (max-width: 700px) { .wl-grid { grid-template-columns: 1fr; } }

  .wl-side { display: flex; flex-direction: column; gap: 0.35rem; }
  .wl-pick {
    background: #11161f; border: 1px solid #1f2733; border-radius: 6px;
    padding: 0.55rem 0.7rem; cursor: pointer; text-align: left;
    color: inherit; font: inherit;
    display: flex; flex-direction: column; gap: 0.2rem;
  }
  .wl-pick:hover { background: rgba(137, 180, 250, 0.06); }
  .wl-pick.active { background: rgba(137, 180, 250, 0.12); border-color: #45567a; }
  .wl-pick strong { display: inline-block; margin-right: 0.4rem; }
  .wl-pick .desc { font-size: 0.75rem; line-height: 1.3; margin-top: 0.15rem; }

  .wl-main h3 { font-size: 0.95rem; margin: 0 0 0.5rem 0; }
  .add-symbol { display: flex; gap: 0.4rem; margin-bottom: 0.75rem; }
  .add-symbol input {
    background: #0a0d14; color: #cdd6f4; border: 1px solid #2a3548; border-radius: 4px;
    padding: 0.35rem 0.5rem; font: inherit; flex: 1;
  }
  .member-table { width: 100%; }
  .member-table th, .member-table td {
    text-align: left; padding: 0.3rem 0.5rem; border-bottom: 1px solid #1f2733;
  }

  /* Discovery review (#54 phase B / #55) */
  .disc-list { list-style: none; padding: 0; display: flex; flex-direction: column; gap: 0.75rem; }
  .disc-card {
    background: #11161f; border: 1px solid #1f2733; border-radius: 6px;
    padding: 0.75rem;
  }
  .disc-hdr { display: flex; gap: 0.5rem; align-items: baseline; flex-wrap: wrap; }
  .disc-reasoning { margin: 0.4rem 0 0.6rem 0; font-size: 0.85rem; }
  .disc-lists { display: flex; flex-direction: column; gap: 0.3rem; margin-bottom: 0.4rem; }
  .disc-pick {
    display: flex; align-items: baseline; gap: 0.5rem; flex-wrap: wrap;
    padding: 0.35rem 0.5rem; border: 1px solid #1f2733; border-radius: 4px;
    cursor: pointer;
  }
  .disc-pick.disabled { cursor: not-allowed; opacity: 0.6; }
  .wl-name { font-weight: 500; }
  .disc-rat { flex: 1; font-size: 0.78rem; }
  .badge.conf-high { background: rgba(166, 227, 161, 0.18); color: rgb(166, 227, 161); }
  .badge.conf-medium { background: rgba(249, 226, 175, 0.15); color: rgb(249, 226, 175); }
  .badge.conf-low { background: rgba(108, 112, 134, 0.2); color: #9aa3b8; }
  .disc-newlist {
    background: rgba(180, 190, 254, 0.05); border: 1px dashed #2a3548;
    border-radius: 4px; padding: 0.4rem 0.55rem; margin-bottom: 0.5rem;
    display: flex; gap: 0.4rem; flex-wrap: wrap; align-items: baseline;
  }
  .disc-actions { display: flex; gap: 0.4rem; margin-top: 0.4rem; }
  .disc-actions .reject {
    background: rgba(243, 139, 168, 0.1); border-color: rgba(243, 139, 168, 0.3);
    color: rgb(243, 139, 168);
  }
</style>
