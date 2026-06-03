<script lang="ts">
  import type { TickerContext } from "./api";

  let {
    ctx,
    symbol,
    autoSynthesize = true,
    blockedReason = "",
  }: {
    ctx: TickerContext | null;
    symbol: string;
    autoSynthesize?: boolean;
    blockedReason?: string;
  } = $props();

  let openBand = $state<"structural" | "narrative" | "market" | null>("structural");
  let synthError = $state<string | null>(null);
  // Track per-symbol so we only fire once per ticker per session, even if
  // ctx stays null across multiple re-renders.
  const fired = new Set<string>();

  // Auto-trigger synthesis the moment we render with no context. No button —
  // the parent's polling picks up v1 within ~30s of the LLM call returning.
  $effect(() => {
    if (!autoSynthesize) return;
    if (ctx !== null || !symbol) return;
    if (fired.has(symbol)) return;
    fired.add(symbol);
    void synthesize();
  });

  function shortTs(s: string | null | undefined): string {
    if (!s) return "—";
    return new Date(s).toLocaleString();
  }

  function isEmpty(o: Record<string, unknown>): boolean {
    return Object.keys(o ?? {}).length === 0;
  }

  function titleize(key: string): string {
    return key
      .replace(/_/g, " ")
      .replace(/\b\w/g, (c) => c.toUpperCase());
  }

  function entries(o: Record<string, unknown>): [string, unknown][] {
    return Object.entries(o ?? {}).filter(([, v]) => v !== null && v !== undefined && v !== "");
  }

  function isRecord(v: unknown): v is Record<string, unknown> {
    return !!v && typeof v === "object" && !Array.isArray(v);
  }

  function isPrimitive(v: unknown): boolean {
    return ["string", "number", "boolean"].includes(typeof v);
  }

  function valueText(v: unknown): string {
    if (typeof v === "number") {
      return Math.abs(v) >= 1_000_000 ? v.toLocaleString() : String(v);
    }
    if (typeof v === "boolean") return v ? "yes" : "no";
    if (typeof v === "string") return v;
    return JSON.stringify(v);
  }

  async function synthesize() {
    synthError = null;
    try {
      const res = await fetch(
        `/api/symbols/${encodeURIComponent(symbol)}/refresh-context`,
        { method: "POST" },
      );
      if (!res.ok) synthError = `HTTP ${res.status}`;
    } catch (e) {
      synthError = e instanceof Error ? e.message : String(e);
    }
  }
</script>

{#if ctx === null && !autoSynthesize}
  <div class="empty">
    <h4>Context <span class="muted-chip">not running</span></h4>
    <p class="muted">
      <strong>{symbol}</strong> is not in the active Universe, so the scheduled
      brain loop will not synthesize context yet.
    </p>
    {#if blockedReason}
      <p class="muted">{blockedReason}</p>
    {/if}
    <p class="muted">
      Promote the ticker first; promotion publishes <code>discovery.confirmed</code>
      and starts context plus thesis work.
    </p>
  </div>
{:else if ctx === null}
  <div class="empty">
    <h4>Context <span class="muted-chip">synthesizing…</span></h4>
    <p class="muted">
      Cognition pipeline is composing the first context version for
      <strong>{symbol}</strong> from the ingested news + estimates + price
      evidence. Usually appears within ~30s.
    </p>
    {#if synthError}
      <p class="err">Refresh failed: {synthError}</p>
    {/if}
  </div>
{:else}
  <div class="context">
    <div class="hdr">
      <h4>Context <span class="version-chip">v{ctx.version}</span></h4>
      <span class="muted">created {shortTs(ctx.created_at)}</span>
    </div>

    <div class="bands">
      <!-- Structural band: fundamentals, competitive position, lagged positioning -->
      <button
        class="band-hdr"
        class:active={openBand === "structural"}
        onclick={() => (openBand = openBand === "structural" ? null : "structural")}
      >
        <span class="caret">{openBand === "structural" ? "▾" : "▸"}</span>
        <strong>Structural</strong>
        <span class="muted">as of {shortTs(ctx.structural_as_of)}</span>
      </button>
      {#if openBand === "structural"}
        <div class="band-body">
          {#if isEmpty(ctx.structural)}
            <p class="muted">empty</p>
          {:else}
            <div class="human-band">
              {#if typeof ctx.structural.summary === "string"}
                <p class="summary">{ctx.structural.summary}</p>
              {/if}
              {#each entries(ctx.structural).filter(([k]) => k !== "summary") as [key, value] (key)}
                <section class="ctx-section">
                  <h5>{titleize(key)}</h5>
                  {#if Array.isArray(value)}
                    <ul>
                      {#each value as item}
                        <li>{isPrimitive(item) ? valueText(item) : JSON.stringify(item)}</li>
                      {/each}
                    </ul>
                  {:else if isRecord(value)}
                    <dl>
                      {#each entries(value) as [k, v] (k)}
                        <dt>{titleize(k)}</dt>
                        <dd>{isPrimitive(v) ? valueText(v) : JSON.stringify(v)}</dd>
                      {/each}
                    </dl>
                  {:else}
                    <p>{valueText(value)}</p>
                  {/if}
                </section>
              {/each}
            </div>
          {/if}
        </div>
      {/if}

      <!-- Narrative band: themes, analyst trajectory, catalysts, risks -->
      <button
        class="band-hdr"
        class:active={openBand === "narrative"}
        onclick={() => (openBand = openBand === "narrative" ? null : "narrative")}
      >
        <span class="caret">{openBand === "narrative" ? "▾" : "▸"}</span>
        <strong>Narrative</strong>
        <span class="muted">as of {shortTs(ctx.narrative_as_of)}</span>
      </button>
      {#if openBand === "narrative"}
        <div class="band-body">
          {#if isEmpty(ctx.narrative)}
            <p class="muted">empty</p>
          {:else}
            <div class="human-band">
              {#if typeof ctx.narrative.summary === "string"}
                <p class="summary">{ctx.narrative.summary}</p>
              {/if}
              {#each entries(ctx.narrative).filter(([k]) => k !== "summary") as [key, value] (key)}
                <section class="ctx-section">
                  <h5>{titleize(key)}</h5>
                  {#if Array.isArray(value)}
                    <ul>
                      {#each value as item}
                        <li>
                          {#if isRecord(item)}
                            {#if item.what || item.date}
                              <strong>{valueText(item.what ?? "Catalyst")}</strong>
                              {#if item.date}<span class="muted"> · {valueText(item.date)}</span>{/if}
                              {#if item.matters_because}<p>{valueText(item.matters_because)}</p>{/if}
                            {:else}
                              {JSON.stringify(item)}
                            {/if}
                          {:else}
                            {valueText(item)}
                          {/if}
                        </li>
                      {/each}
                    </ul>
                  {:else if isRecord(value)}
                    <dl>
                      {#each entries(value) as [k, v] (k)}
                        <dt>{titleize(k)}</dt>
                        <dd>{isPrimitive(v) ? valueText(v) : JSON.stringify(v)}</dd>
                      {/each}
                    </dl>
                  {:else}
                    <p>{valueText(value)}</p>
                  {/if}
                </section>
              {/each}
            </div>
          {/if}
        </div>
      {/if}

      <!-- Market band: raw (not LLM-synthesized per SPEC §5.2) -->
      <button
        class="band-hdr"
        class:active={openBand === "market"}
        onclick={() => (openBand = openBand === "market" ? null : "market")}
      >
        <span class="caret">{openBand === "market" ? "▾" : "▸"}</span>
        <strong>Market</strong>
        <span class="muted">
          {isEmpty(ctx.market) ? "raw (no ingest yet)" : `as of ${shortTs(ctx.market_as_of)}`}
        </span>
      </button>
      {#if openBand === "market"}
        <div class="band-body">
          {#if isEmpty(ctx.market)}
            <p class="muted">
              Market band is intentionally raw (SPEC §5.2). Populated by the indicator
              pipeline once price ingestion lands (#17).
            </p>
          {:else}
            <div class="human-band">
              {#each entries(ctx.market) as [key, value] (key)}
                <section class="ctx-section">
                  <h5>{titleize(key)}</h5>
                  <p>{isPrimitive(value) ? valueText(value) : JSON.stringify(value)}</p>
                </section>
              {/each}
            </div>
          {/if}
        </div>
      {/if}
    </div>
  </div>
{/if}

<style>
  .empty, .context {
    background: #0c1019; border: 1px solid #1f2733; border-radius: 6px;
    padding: 0.75rem 1rem; margin: 0.5rem 0;
  }
  .hdr { display: flex; align-items: baseline; gap: 0.6rem; }
  h4 { font-size: 0.85rem; color: #bac2de; margin: 0 0 0.5rem 0; }
  .version-chip {
    display: inline-block; padding: 0.05rem 0.4rem; border-radius: 4px;
    background: rgba(137, 180, 250, 0.12); color: #89b4fa; font-size: 0.7rem;
  }
  .bands { display: flex; flex-direction: column; gap: 0.25rem; }
  .band-hdr {
    display: flex; align-items: baseline; gap: 0.5rem; width: 100%;
    background: #11161f; color: inherit; border: 1px solid #1f2733;
    border-radius: 4px; padding: 0.4rem 0.6rem; text-align: left;
    cursor: pointer; font: inherit;
  }
  .band-hdr:hover { background: #131927; }
  .band-hdr.active { background: rgba(137, 180, 250, 0.06); border-color: #2a3548; }
  .caret { color: #6c7086; font-size: 0.8rem; width: 1rem; }
  .band-body {
    background: #0a0d14; padding: 0.5rem 0.75rem; border-radius: 4px;
    border: 1px solid #1f2733; border-top: 0;
    margin-top: -0.25rem; margin-bottom: 0.25rem;
  }
  .human-band { display: flex; flex-direction: column; gap: 0.65rem; }
  .summary {
    margin: 0; color: #d7def7; line-height: 1.45;
    padding-bottom: 0.55rem; border-bottom: 1px solid #1f2733;
  }
  .ctx-section { display: flex; flex-direction: column; gap: 0.3rem; }
  .ctx-section h5 {
    margin: 0; color: #89b4fa; font-size: 0.76rem;
  }
  .ctx-section p { margin: 0; line-height: 1.45; }
  .ctx-section ul { margin: 0; padding-left: 1.05rem; display: flex; flex-direction: column; gap: 0.35rem; }
  .ctx-section li { line-height: 1.4; }
  .ctx-section li p { margin-top: 0.15rem; color: #a6adc8; }
  .ctx-section dl {
    display: grid; grid-template-columns: minmax(8rem, 35%) 1fr;
    gap: 0.25rem 0.75rem; margin: 0;
  }
  .ctx-section dt { color: #6c7086; }
  .ctx-section dd { margin: 0; line-height: 1.4; overflow-wrap: anywhere; }
  .muted { color: #6c7086; font-size: 0.8rem; }
  .muted-chip {
    display: inline-block; padding: 0.05rem 0.4rem; border-radius: 4px;
    background: rgba(108, 112, 134, 0.15); color: #6c7086;
    font-size: 0.7rem; font-weight: 400;
  }
  .err { color: #f38ba8; font-size: 0.8rem; }
</style>
