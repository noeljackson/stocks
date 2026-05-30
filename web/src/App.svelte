<script lang="ts">
  import { onMount } from "svelte";
  import { fetchAlerts, subscribe, type Alert, type StreamEvent } from "./lib/api";

  type View = "alerts" | "positions" | "context";
  let view = $state<View>("alerts");
  let alerts = $state<Alert[]>([]);
  let live = $state<StreamEvent[]>([]);
  let connected = $state(false);
  let error = $state<string | null>(null);

  onMount(() => {
    fetchAlerts()
      .then((a) => (alerts = a))
      .catch((e) => (error = String(e)));
    const stop = subscribe(
      (e) => (live = [e, ...live].slice(0, 100)),
      (open) => (connected = open),
    );
    return stop;
  });
</script>

<header>
  <h1>stocks <span class="muted">intelligence</span></h1>
  <nav>
    <button class:active={view === "alerts"} onclick={() => (view = "alerts")}>Alerts</button>
    <button class:active={view === "positions"} onclick={() => (view = "positions")}>Positions</button>
    <button class:active={view === "context"} onclick={() => (view = "context")}>Context</button>
  </nav>
  <span class="status" class:on={connected}>{connected ? "live" : "offline"}</span>
</header>

<main>
  {#if error}<p class="error">{error}</p>{/if}

  {#if view === "alerts"}
    <h2>Live feed</h2>
    {#if live.length === 0}<p class="muted">Waiting for events…</p>{/if}
    <ul class="feed">
      {#each live as e, i (i)}
        <li><code>{e.subject}</code> <span class="kind">{e.kind}</span></li>
      {/each}
    </ul>

    <h2>Recent alerts</h2>
    {#if alerts.length === 0}<p class="muted">No alerts yet.</p>{/if}
    <ul class="feed">
      {#each alerts as a (a.id)}
        <li>
          <span class="kind">{a.kind}</span>
          {#if a.symbol}<strong>{a.symbol}</strong>{/if}
          <span class="muted">{new Date(a.created_at).toLocaleString()}</span>
        </li>
      {/each}
    </ul>
  {:else if view === "positions"}
    <h2>Positions</h2>
    <p class="muted">Position tracking — Phase 2.</p>
  {:else}
    <h2>Ticker context</h2>
    <p class="muted">3-band context view — Phase 1/2.</p>
  {/if}
</main>

<style>
  header { display: flex; align-items: center; gap: 1rem; flex-wrap: wrap; }
  nav { display: flex; gap: 0.5rem; }
  button {
    background: #1b2230; color: #cdd6f4; border: 1px solid #2a3548;
    border-radius: 6px; padding: 0.35rem 0.7rem; cursor: pointer;
  }
  button.active { background: #2a3548; }
  .status { margin-left: auto; font-size: 0.8rem; color: #f38ba8; }
  .status.on { color: #a6e3a1; }
  .feed { list-style: none; padding: 0; display: flex; flex-direction: column; gap: 0.25rem; }
  .feed li {
    background: #11161f; border: 1px solid #1f2733; border-radius: 6px; padding: 0.4rem 0.6rem;
  }
  .kind { font-size: 0.75rem; color: #89b4fa; text-transform: uppercase; }
</style>
