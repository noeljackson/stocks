<script lang="ts">
  import type { TickerContext } from "./api";

  let { ctx, symbol }: { ctx: TickerContext | null; symbol: string } = $props();

  let openBand = $state<"structural" | "narrative" | "market" | null>("structural");

  function shortTs(s: string | null | undefined): string {
    if (!s) return "—";
    return new Date(s).toLocaleString();
  }

  function isEmpty(o: Record<string, unknown>): boolean {
    return Object.keys(o ?? {}).length === 0;
  }

  function pretty(o: unknown): string {
    return JSON.stringify(o, null, 2);
  }
</script>

{#if ctx === null}
  <div class="empty">
    <h4>Context</h4>
    <p class="muted">
      No <code>ticker_context</code> yet for <strong>{symbol}</strong>.
      Run <code>make refresh-context SYMBOL={symbol}</code> to synthesize one from the
      ingested corpus. The thesis engine refuses to draft against empty context.
    </p>
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
            <pre>{pretty(ctx.structural)}</pre>
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
            <pre>{pretty(ctx.narrative)}</pre>
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
            <pre>{pretty(ctx.market)}</pre>
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
  pre {
    margin: 0; font-size: 0.75rem; line-height: 1.45; color: #cdd6f4;
    white-space: pre-wrap; word-break: break-word;
  }
  .muted { color: #6c7086; font-size: 0.8rem; }
  code { background: #0a0d14; padding: 0.05rem 0.3rem; border-radius: 3px; font-size: 0.85rem; }
</style>
