<script lang="ts">
  import { onMount } from "svelte";
  import {
    fetchAlerts,
    fetchRegime,
    fetchTickers,
    postDecision,
    subscribe,
    type Alert,
    type MarketState,
    type StreamEvent,
    type Ticker,
  } from "./lib/api";

  type View = "feed" | "tickers" | "decisions";
  let view = $state<View>("feed");

  let regime = $state<MarketState | null>(null);
  let tickers = $state<Ticker[]>([]);
  let alerts = $state<Alert[]>([]);
  let live = $state<StreamEvent[]>([]);
  let connected = $state(false);
  let error = $state<string | null>(null);

  // Decision form
  let decThesisId = $state("");
  let decAction = $state("skip");
  let decChoice = $state("deferred");
  let decStatus = $state<string | null>(null);

  function refreshAll() {
    fetchAlerts().then((a) => (alerts = a)).catch((e) => (error = String(e)));
    fetchRegime().then((r) => (regime = r)).catch((e) => (error = String(e)));
    fetchTickers().then((t) => (tickers = t)).catch((e) => (error = String(e)));
  }

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
          fetchAlerts().then((a) => (alerts = a)).catch(() => {});
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
  <span class="status" class:on={connected}>{connected ? "● live" : "○ offline"}</span>
</header>

<nav>
  <button class:active={view === "feed"} onclick={() => (view = "feed")}>Feed</button>
  <button class:active={view === "tickers"} onclick={() => (view = "tickers")}>
    Tickers <span class="badge">{tickers.length}</span>
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
          <li>
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
          </li>
        {/each}
      </ul>
    </section>

    <section>
      <h2>Recent alerts <span class="muted">({alerts.length})</span></h2>
      {#if alerts.length === 0}
        <p class="muted">No alerts yet.</p>
      {/if}
      <ul class="feed">
        {#each alerts as a (a.id)}
          {@const p = (a.payload ?? {}) as Record<string, unknown>}
          <li>
            <span class="kind" style="color:{kindColor(a.kind, p)}">{a.kind}</span>
            {#if a.symbol}<strong>{a.symbol}</strong>{/if}
            {#if p.veto}<span class="badge danger">VETO</span>{/if}
            {#if p.kind === "goalpost_moved"}<span class="badge warning">GOALPOST</span>{/if}
            <span class="muted">{shortTs(a.created_at)}</span>
          </li>
        {/each}
      </ul>
    </section>
  {:else if view === "tickers"}
    <h2>Tracked tickers</h2>
    {#if tickers.length === 0}
      <p class="muted">No active tickers seeded. Run <code>make seed-demo</code> to populate sample data.</p>
    {/if}
    <table>
      <thead>
        <tr>
          <th>Symbol</th><th>Cluster</th><th>Tier</th>
          <th>Domain-fit</th><th>Options</th><th>Open theses</th>
        </tr>
      </thead>
      <tbody>
        {#each tickers as t (t.symbol)}
          <tr>
            <td><strong>{t.symbol}</strong></td>
            <td><span class="muted">{t.cluster_name ?? t.cluster_id}</span></td>
            <td>T{t.tier}</td>
            <td>{t.domain_fit !== null && t.domain_fit !== undefined ? Math.round(t.domain_fit) : "—"}</td>
            <td>{t.options_eligible ? "✓" : ""}</td>
            <td>{t.open_theses}</td>
          </tr>
        {/each}
      </tbody>
    </table>
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
</style>
