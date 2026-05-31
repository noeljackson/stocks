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

  // ---------- workspace state ----------
  type RightTab = "overview" | "context" | "theses" | "alerts" | "decisions";
  type BottomMode = "events" | "discovery" | "decisions" | "calibration";

  let selectedSymbol = $state<string | null>(null);
  let selectedWatchlistId = $state<string | null>(null);
  let rightTab = $state<RightTab>("overview");
  let bottomMode = $state<BottomMode>("events");
  let bottomOpen = $state(true);

  // ---------- global data ----------
  let regime = $state<MarketState | null>(null);
  let calibration = $state<Calibration | null>(null);
  let tickers = $state<Ticker[]>([]);
  let alerts = $state<Alert[]>([]);
  let live = $state<StreamEvent[]>([]);
  let connected = $state(false);
  let error = $state<string | null>(null);
  let pending = $state<PendingCandidate[]>([]);
  let watchlists = $state<Watchlist[]>([]);
  let watchlistMembers = $state<Record<string, WatchlistMember[]>>({});

  // ---------- selected-symbol-scoped data ----------
  let symbolContext = $state<TickerContext | null | undefined>(undefined);
  let symbolTheses = $state<ThesisDetail[] | null | undefined>(undefined);
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
  let decChoice = $state("deferred");
  let decStatus = $state<string | null>(null);

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
  let allWatchlists = $derived<Watchlist[]>([...watchlists, universeList]);
  let universeMembers = $derived<WatchlistMember[]>(
    tickers.map((t) => ({
      watchlist_id: UNIVERSE_ID,
      symbol: t.symbol,
      added_at: t.added_at,
      added_by: "system",
    })),
  );

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

  function tickerFor(symbol: string | null): Ticker | undefined {
    if (!symbol) return undefined;
    return tickers.find((t) => t.symbol === symbol);
  }

  function membersFor(listId: string): WatchlistMember[] {
    if (listId === UNIVERSE_ID) return universeMembers;
    return watchlistMembers[listId] ?? [];
  }

  // ---------- selection logic ----------
  async function selectSymbol(symbol: string) {
    if (selectedSymbol === symbol) return;
    selectedSymbol = symbol;
    symbolContext = undefined;
    symbolTheses = undefined;
    // Fetch detail in parallel.
    const [ctx, theses] = await Promise.all([
      fetchTickerContext(symbol).catch(() => null),
      fetchTheses(symbol).catch(() => []),
    ]);
    symbolContext = ctx;
    symbolTheses = theses;
  }

  function pickFirstSymbol() {
    if (selectedSymbol) return;
    // Try first non-empty user watchlist, then Universe.
    for (const w of allWatchlists) {
      const m = membersFor(w.id);
      if (m.length > 0) {
        selectedWatchlistId = w.id;
        expandedListIds = { ...expandedListIds, [w.id]: true };
        selectSymbol(m[0].symbol);
        return;
      }
    }
  }

  async function toggleListExpanded(id: string) {
    const open = !expandedListIds[id];
    expandedListIds = { ...expandedListIds, [id]: open };
    if (open && id !== UNIVERSE_ID && !watchlistMembers[id]) {
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
    const sym = (addSymbolFor[id] ?? "").trim().toUpperCase();
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
    } catch (err) {
      decStatus = `error: ${err}`;
    }
  }

  // ---------- bootstrap ----------
  function refreshAll() {
    fetchAlerts({ unacked: !showAcked }).then((a) => (alerts = a)).catch((e) => (error = String(e)));
    fetchRegime().then((r) => (regime = r)).catch((e) => (error = String(e)));
    fetchTickers().then((t) => (tickers = t)).catch((e) => (error = String(e)));
    fetchCalibration().then((c) => (calibration = c)).catch(() => {});
    refreshWatchlists();
    refreshPending();
  }

  $effect(() => {
    fetchAlerts({ unacked: !showAcked }).then((a) => (alerts = a)).catch(() => {});
  });

  $effect(() => {
    // Once tickers and watchlists arrive, auto-pick the first symbol.
    if (!selectedSymbol && (tickers.length > 0 || watchlists.length > 0)) {
      pickFirstSymbol();
    }
  });

  onMount(() => {
    refreshAll();
    const stop = subscribe(
      (e) => {
        live = [e, ...live].slice(0, 200);
        if (e.subject?.startsWith("regime.")) {
          fetchRegime().then((r) => (regime = r)).catch(() => {});
        }
        if (e.kind === "state_transition" || e.kind === "risk") {
          fetchAlerts({ unacked: !showAcked }).then((a) => (alerts = a)).catch(() => {});
        }
      },
      (open) => (connected = open),
    );
    return stop;
  });

  let selectedTicker = $derived(tickerFor(selectedSymbol));

  // ---------- panel sizing + resize ----------
  // Pixels for the right panel width and the bottom drawer height.
  // Persisted to localStorage so reloads don't reset.
  function loadSize(k: string, def: number, min: number, max: number): number {
    const raw = typeof localStorage !== "undefined" ? localStorage.getItem(k) : null;
    const n = raw ? parseInt(raw, 10) : NaN;
    return Number.isFinite(n) ? Math.max(min, Math.min(max, n)) : def;
  }
  // v2 keys — bumped on default change to drop any stuck values from earlier runs.
  let rightWidth = $state(loadSize("ws.v2.rightWidth", 360, 240, 800));
  let bottomHeight = $state(loadSize("ws.v2.bottomHeight", 200, 80, 600));
  $effect(() => {
    try { localStorage.setItem("ws.v2.rightWidth", String(rightWidth)); } catch {}
  });
  $effect(() => {
    try { localStorage.setItem("ws.v2.bottomHeight", String(bottomHeight)); } catch {}
  });

  function startResizeRight(e: PointerEvent) {
    const startX = e.clientX;
    const startW = rightWidth;
    (e.target as HTMLElement).setPointerCapture?.(e.pointerId);
    const move = (m: PointerEvent) => {
      const dx = startX - m.clientX;
      rightWidth = Math.max(240, Math.min(800, startW + dx));
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  }
  function startResizeBottom(e: PointerEvent) {
    if (!bottomOpen) {
      // First drag from collapsed state opens the drawer.
      bottomOpen = true;
    }
    const startY = e.clientY;
    const startH = bottomHeight;
    (e.target as HTMLElement).setPointerCapture?.(e.pointerId);
    const move = (m: PointerEvent) => {
      const dy = startY - m.clientY;
      // Clamp to viewport so we never push the drawer past the chart area.
      const maxH = Math.max(120, window.innerHeight - 200);
      bottomHeight = Math.max(80, Math.min(maxH, startH + dy));
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  }
  function resetBottom() {
    bottomHeight = 200;
    bottomOpen = true;
  }
</script>

<div
  class="workspace"
  class:bottom-open={bottomOpen}
  style="--right-w: {rightWidth}px; --bottom-h: {bottomOpen ? bottomHeight : 36}px;"
>
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
  <div class="body">
    <div class="main-col">
      <section class="chart-panel">
      <div class="chart-stub">
        <h3>
          {#if selectedSymbol}
            {selectedSymbol} chart
          {:else}
            no symbol selected
          {/if}
        </h3>
        <p class="muted">
          Chart panel comes in #57 PR2. For now the workspace shell + selection model is in place.
        </p>
        {#if selectedTicker}
          <dl class="ticker-meta">
            <dt>Cluster</dt><dd>{selectedTicker.cluster_name ?? selectedTicker.cluster_id}</dd>
            <dt>Tier</dt><dd>T{selectedTicker.tier}</dd>
            <dt>Domain fit</dt><dd>{selectedTicker.domain_fit !== null && selectedTicker.domain_fit !== undefined ? Math.round(selectedTicker.domain_fit) : "—"}</dd>
            <dt>Options</dt><dd>{selectedTicker.options_eligible ? "eligible" : "—"}</dd>
            <dt>Open theses</dt><dd>{selectedTicker.open_theses}</dd>
          </dl>
        {/if}
      </div>
      </section>

      <!-- splitter is a sibling between chart and drawer (grid row 2) -->
      <div
        class="split-h"
        role="separator"
        aria-orientation="horizontal"
        title="drag to resize"
        onpointerdown={startResizeBottom}
      ></div>

      <!-- bottom drawer is inside main-col so it only spans the chart width -->
      <footer class="bottom">
    <nav class="bottom-tabs">
      {#each ["events", "discovery", "decisions", "calibration"] as BottomMode[] as m}
        <button class:active={bottomMode === m} onclick={() => (bottomMode = m, bottomOpen = true)}>
          {m}
          {#if m === "discovery" && pending.length > 0}<span class="badge tiny">{pending.length}</span>{/if}
          {#if m === "events"}<span class="badge tiny">{live.length}</span>{/if}
        </button>
      {/each}
      <button
        class="bottom-toggle"
        onclick={() => (bottomOpen = !bottomOpen)}
        title={bottomOpen ? "collapse drawer" : "expand drawer"}
      >
        {bottomOpen ? "▾ hide" : "▴ show"}
      </button>
      {#if bottomOpen}
        <button class="bottom-reset" onclick={resetBottom} title="reset drawer height">⟲</button>
      {/if}
    </nav>

    {#if bottomOpen}
      <div class="bottom-body">
        {#if bottomMode === "events"}
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
                    <span class="badge tiny">{c.signal_name}</span>
                    {#if c.signal_value !== null}<span class="muted">value {c.signal_value.toFixed(3)}</span>{/if}
                    <span class="muted">{shortTs(c.proposed_at)}</span>
                  </div>
                  {#if c.reasoning}<p class="muted disc-reasoning">{c.reasoning}</p>{/if}
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
            <label>
              Action
              <select bind:value={decAction}>
                <option>enter</option><option>exit</option><option>skip</option><option>resize</option>
              </select>
            </label>
            <label>
              User choice
              <select bind:value={decChoice}>
                <option>confirmed</option><option>rejected</option><option>deferred</option>
              </select>
            </label>
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
        {/if}
      </div>
    {/if}
      </footer>
    </div>

    <div
      class="split-v"
      role="separator"
      aria-orientation="vertical"
      title="drag to resize"
      onpointerdown={startResizeRight}
    ></div>

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
              <div class="wl-row" onclick={() => toggleListExpanded(w.id)}>
                <span class="caret">{open ? "▾" : "▸"}</span>
                <span class="wl-name" style={w.color ? `border-left: 3px solid ${w.color}; padding-left: .35rem` : ""}>{w.name}</span>
                <span class="muted">{w.member_count}</span>
                {#if w.is_system}<span class="badge tiny">sys</span>{/if}
              </div>
              {#if open}
                {#if w.id !== UNIVERSE_ID}
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
                      onclick={() => selectSymbol(m.symbol)}
                    >
                      <strong>{m.symbol}</strong>
                      {#if w.id !== UNIVERSE_ID}
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
            {#each ["overview", "context", "theses", "alerts", "decisions"] as RightTab[] as t}
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
            {:else if rightTab === "context"}
              {#if symbolContext === undefined}
                <p class="muted">Loading…</p>
              {:else}
                <ContextPanel ctx={symbolContext ?? null} symbol={selectedSymbol} />
              {/if}
            {:else if rightTab === "theses"}
              {#if symbolTheses === undefined}
                <p class="muted">Loading…</p>
              {:else if symbolTheses && symbolTheses.length > 0}
                {#each symbolTheses as t (t.thesis_id)}
                  <ThesisDetails thesis={t} />
                {/each}
              {:else}
                <p class="muted">
                  No theses for <strong>{selectedSymbol}</strong>. Run
                  <code>make draft-thesis SYMBOL={selectedSymbol}</code>.
                </p>
              {/if}
            {:else if rightTab === "alerts"}
              <div class="alert-toolbar">
                <label class="toggle"><input type="checkbox" bind:checked={showAcked} /> show acked</label>
              </div>
              {@const syms = alerts.filter((a) => !a.symbol || a.symbol === selectedSymbol)}
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
              <p class="muted">Per-symbol decisions view comes in #57 PR4. For now the global decision form lives in the bottom drawer.</p>
            {/if}
          </div>
        {:else}
          <p class="muted center-msg">Pick a symbol on the left.</p>
        {/if}
      </section>
    </aside>
  </div>

</div>

<style>
  .workspace {
    /* Locked to viewport edges — no dependency on any parent chain. */
    position: fixed;
    inset: 0;
    display: grid;
    /* Top bar (44) / error bar (auto when present) / body (fills) */
    grid-template-rows: 44px auto minmax(0, 1fr);
    grid-template-columns: 1fr;
    background: #0b0e14;
    overflow: hidden;
  }

  /* Body splits horizontally: main-col | splitter | right panel.
     Right panel is full body height; bottom drawer is nested in main-col. */
  .body {
    display: grid;
    grid-template-columns: minmax(0, 1fr) 6px var(--right-w, 360px);
    min-height: 0;
    overflow: hidden;
  }
  .main-col {
    display: grid;
    /* chart fills, splitter (8px), bottom drawer (var) */
    grid-template-rows: minmax(0, 1fr) 8px var(--bottom-h, 36px);
    min-height: 0;
    min-width: 0;
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

  .error-bar {
    margin: 0 1rem; display: flex; align-items: center; gap: .5rem;
  }
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
  }
  .split-v {
    background: #1f2733;
    cursor: col-resize;
    transition: background .15s;
    /* Wider hit area than the visual; pseudo center bar inside. */
    position: relative;
    width: 6px;
  }
  .split-v::before {
    content: ""; position: absolute; top: 0; bottom: 0;
    left: 50%; width: 2px; transform: translateX(-50%);
    background: #2a3548;
  }
  .split-v:hover, .split-v:active { background: #45567a; }
  .split-v:hover::before, .split-v:active::before { background: #89b4fa; }

  .split-h {
    background: #1f2733;
    cursor: row-resize;
    height: 8px;
    flex-shrink: 0;
    transition: background .15s;
    position: relative;
  }
  .split-h::before {
    content: ""; position: absolute; left: 50%; top: 50%;
    transform: translate(-50%, -50%);
    width: 40px; height: 3px; border-radius: 2px;
    background: #45567a;
  }
  .split-h:hover, .split-h:active { background: #45567a; }
  .split-h:hover::before, .split-h:active::before { background: #89b4fa; }
  .chart-stub {
    height: 100%;
    display: flex; flex-direction: column;
    border: 1px dashed #2a3548; border-radius: 8px;
    padding: 1.5rem; align-items: center; justify-content: center;
    background: rgba(180, 190, 254, .02);
    text-align: center;
  }
  .ticker-meta {
    display: grid; grid-template-columns: auto auto; gap: .2rem 1rem;
    margin-top: 1rem; font-size: .85rem;
  }
  .ticker-meta dt { color: #6c7693; }
  .ticker-meta dd { margin: 0; font-weight: 500; }

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
    display: flex; gap: .35rem; align-items: baseline; cursor: pointer;
    padding: .2rem .25rem; border-radius: 3px;
  }
  .wl-row:hover { background: rgba(137, 180, 250, .06); }
  .caret { color: #6c7693; font-size: .7rem; width: .9rem; }
  .wl-name { font-size: .85rem; font-weight: 500; flex: 1; }
  .wl-members { list-style: none; padding: 0 0 0 1.5rem; margin: .1rem 0 .25rem; display: flex; flex-direction: column; gap: .1rem; }
  .wl-mem {
    display: flex; gap: .35rem; align-items: baseline; padding: .15rem .3rem;
    cursor: pointer; border-radius: 3px; font-size: .8rem;
  }
  .wl-mem:hover { background: rgba(137, 180, 250, .08); }
  .wl-mem.active { background: rgba(137, 180, 250, .18); }
  .wl-mem strong { flex: 1; }
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
  .bottom-reset { background: transparent; border: 1px solid #2a3548; color: #6c7693; }
  .bottom-reset:hover { color: #cdd6f4; border-color: #45567a; }
  .bottom-body {
    flex: 1; overflow: auto; padding: .5rem .75rem;
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
  .alerts .x, .alert-toolbar .x {
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
    display: grid; grid-template-columns: 1fr 1fr; gap: .5rem; max-width: 600px;
    font-size: .85rem;
  }
  .decform label { display: flex; flex-direction: column; gap: .15rem; }
  .decform input, .decform select {
    background: #0a0d14; color: #cdd6f4; border: 1px solid #2a3548; border-radius: 4px;
    padding: .25rem .4rem; font: inherit;
  }
  .decform button {
    grid-column: 1;
    background: #1b2230; color: #cdd6f4; border: 1px solid #45567a;
    border-radius: 4px; padding: .35rem .8rem; font: inherit; cursor: pointer;
  }
  .decform .muted { grid-column: 2; align-self: end; }

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
</style>
