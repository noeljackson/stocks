<script lang="ts">
  import {
    disablePriceAlert,
    fetchPriceAlertEvents,
    fetchPriceAlerts,
    type PriceAlertEvent,
    type PriceAlertRule,
    type StreamEvent,
  } from "./api";

  let {
    symbol,
    liveEvents = [] as StreamEvent[],
  }: { symbol: string | null; liveEvents?: StreamEvent[] } = $props();

  let rules = $state<PriceAlertRule[] | null>(null);
  let events = $state<PriceAlertEvent[] | null>(null);
  let error = $state<string | null>(null);
  let busyRule = $state<number | null>(null);
  let lastSymbol = "";
  let lastLiveKey = "";

  let activeRules = $derived(
    (rules ?? [])
      .filter((rule) => rule.status === "active")
      .sort((a, b) => a.target_price - b.target_price),
  );
  let inactiveRules = $derived(
    (rules ?? [])
      .filter((rule) => rule.status !== "active")
      .slice(0, 6),
  );

  async function refresh(nextSymbol: string) {
    error = null;
    try {
      const [nextRules, nextEvents] = await Promise.all([
        fetchPriceAlerts({ symbol: nextSymbol }),
        fetchPriceAlertEvents({ symbol: nextSymbol }),
      ]);
      rules = nextRules;
      events = nextEvents;
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
      rules = [];
      events = [];
    }
  }

  async function disableRule(rule: PriceAlertRule) {
    busyRule = rule.id;
    error = null;
    try {
      const updated = await disablePriceAlert(rule.id);
      rules = (rules ?? []).map((row) => row.id === updated.id ? updated : row);
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      busyRule = null;
    }
  }

  function money(value: number | null | undefined): string {
    if (value === null || value === undefined || Number.isNaN(value)) return "-";
    return value.toLocaleString(undefined, { style: "currency", currency: "USD", maximumFractionDigits: 2 });
  }

  function shortTs(value: string | null | undefined): string {
    if (!value) return "";
    return new Date(value).toLocaleString();
  }

  function titleize(value: string | null | undefined): string {
    return (value ?? "")
      .replace(/_/g, " ")
      .replace(/\b\w/g, (char) => char.toUpperCase());
  }

  function liveEventKey(event: StreamEvent): string {
    return `${event.subject}|${String(event.payload.symbol ?? "")}|${String(event.payload.rule_id ?? "")}|${String(event.payload.triggered_at ?? "")}`;
  }

  $effect(() => {
    if (!symbol) {
      lastSymbol = "";
      rules = null;
      events = null;
      error = null;
      return;
    }
    if (symbol === lastSymbol) return;
    lastSymbol = symbol;
    rules = null;
    events = null;
    void refresh(symbol);
  });

  $effect(() => {
    if (!symbol) return;
    const hit = liveEvents.find((event) =>
      event.kind === "price_alert" && String(event.payload.symbol ?? "").toUpperCase() === symbol.toUpperCase()
    );
    if (!hit) return;
    const key = liveEventKey(hit);
    if (key === lastLiveKey) return;
    lastLiveKey = key;
    void refresh(symbol);
  });
</script>

<section class="price-alerts" data-testid="price-alerts-panel">
  <header class="panel-hdr">
    <div>
      <h4>Price Alerts</h4>
      <span>{symbol ?? "No symbol selected"}</span>
    </div>
    {#if symbol}
      <button type="button" onclick={() => refresh(symbol)} disabled={rules === null}>refresh</button>
    {/if}
  </header>

  {#if !symbol}
    <p class="muted">Select a symbol to see active and triggered levels.</p>
  {:else if error}
    <p class="error-text">{error}</p>
  {/if}

  {#if symbol && rules === null}
    <p class="muted">Loading price alerts...</p>
  {:else if symbol}
    <section class="alert-block">
      <div class="section-hdr">
        <h5>Active</h5>
        <span>{activeRules.length}</span>
      </div>
      {#if activeRules.length === 0}
        <p class="muted">No active price levels. Create one from the chart Alert button.</p>
      {:else}
        <ul class="rule-list">
          {#each activeRules as rule (rule.id)}
            <li class="rule origin-{rule.origin}">
              <div class="rule-main">
                <strong>{rule.direction} {money(rule.target_price)}</strong>
                <span class="badge intent-{rule.intent}">{titleize(rule.intent)}</span>
                <span class="badge origin">{rule.origin === "ai" ? "AI" : "manual"}</span>
              </div>
              <p>{rule.label}</p>
              {#if rule.rationale}<p class="muted">{rule.rationale}</p>{/if}
              <div class="rule-foot">
                <span>{shortTs(rule.created_at)}</span>
                {#if rule.expires_at}<span>expires {shortTs(rule.expires_at)}</span>{/if}
                <button type="button" disabled={busyRule === rule.id} onclick={() => disableRule(rule)}>
                  {busyRule === rule.id ? "disabling..." : "disable"}
                </button>
              </div>
            </li>
          {/each}
        </ul>
      {/if}
    </section>

    <section class="alert-block">
      <div class="section-hdr">
        <h5>Triggered</h5>
        <span>{events?.length ?? 0}</span>
      </div>
      {#if !events || events.length === 0}
        <p class="muted">No triggered price alerts yet.</p>
      {:else}
        <ul class="event-list">
          {#each events.slice(0, 8) as event (event.id)}
            <li>
              <strong>{money(event.trigger_price)}</strong>
              <span>{event.trigger_interval}</span>
              <span class="muted">{shortTs(event.trigger_ts)}</span>
            </li>
          {/each}
        </ul>
      {/if}
    </section>

    {#if inactiveRules.length > 0}
      <section class="alert-block">
        <div class="section-hdr">
          <h5>Recent Rules</h5>
        </div>
        <ul class="event-list">
          {#each inactiveRules as rule (rule.id)}
            <li>
              <strong>{titleize(rule.status)}</strong>
              <span>{rule.direction} {money(rule.target_price)}</span>
              <span class="muted">{rule.label}</span>
            </li>
          {/each}
        </ul>
      </section>
    {/if}
  {/if}
</section>

<style>
  .price-alerts {
    display: flex;
    flex-direction: column;
    gap: .65rem;
  }

  .panel-hdr,
  .section-hdr,
  .rule-main,
  .rule-foot,
  .event-list li {
    display: flex;
    align-items: center;
    gap: .4rem;
    flex-wrap: wrap;
  }

  .panel-hdr {
    justify-content: space-between;
  }

  h4,
  h5,
  p {
    margin: 0;
  }

  h4 {
    font-size: .95rem;
  }

  h5 {
    color: #cdd6f4;
    font-size: .78rem;
    text-transform: uppercase;
  }

  .panel-hdr span,
  .section-hdr span,
  .muted {
    color: #6c7693;
    font-size: .76rem;
  }

  button {
    background: #1b2230;
    color: #cdd6f4;
    border: 1px solid #2a3548;
    border-radius: 4px;
    padding: .18rem .5rem;
    font: inherit;
    cursor: pointer;
  }

  button:disabled {
    opacity: .55;
    cursor: default;
  }

  .alert-block {
    border: 1px solid #1f2733;
    border-radius: 4px;
    background: #0a0d14;
    padding: .55rem .6rem;
  }

  .section-hdr {
    justify-content: space-between;
    margin-bottom: .45rem;
  }

  .rule-list,
  .event-list {
    list-style: none;
    display: flex;
    flex-direction: column;
    gap: .35rem;
    padding: 0;
    margin: 0;
  }

  .rule {
    border: 1px solid #263143;
    border-radius: 4px;
    background: #11161f;
    padding: .48rem .55rem;
  }

  .rule.origin-ai {
    border-left: 3px solid #89b4fa;
  }

  .rule.origin-manual {
    border-left: 3px solid #a6e3a1;
  }

  .rule p {
    line-height: 1.35;
    margin-top: .25rem;
  }

  .rule-foot {
    margin-top: .35rem;
    color: #6c7693;
    font-size: .72rem;
  }

  .rule-foot button {
    margin-left: auto;
    padding: .08rem .42rem;
  }

  .badge {
    border: 1px solid #2a3548;
    border-radius: 4px;
    color: #bac2de;
    font-size: .68rem;
    padding: .04rem .3rem;
  }

  .badge.origin {
    color: #89b4fa;
  }

  .intent-entry {
    color: #a6e3a1;
  }

  .intent-invalidation,
  .intent-exit {
    color: #f38ba8;
  }

  .event-list li {
    border-top: 1px solid #1f2733;
    padding-top: .32rem;
    font-size: .78rem;
  }

  .event-list li:first-child {
    border-top: 0;
    padding-top: 0;
  }

  .error-text {
    color: #f38ba8;
    font-size: .8rem;
  }
</style>
