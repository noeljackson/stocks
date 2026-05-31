<script lang="ts">
  // ChartPanel — daily candles via lightweight-charts (#57 PR2).
  // Renders OHLC + volume in the workspace's chart pane. Range buttons
  // hit /api/candles?symbol=&range= to swap data without re-mounting.
  import { onMount } from "svelte";
  import {
    createChart,
    CandlestickSeries,
    HistogramSeries,
    createSeriesMarkers,
    type IChartApi,
    type ISeriesApi,
    type ISeriesMarkersPluginApi,
    type CandlestickData,
    type HistogramData,
    type UTCTimestamp,
    type Time,
    type SeriesMarker,
    type SeriesMarkerBarPosition,
    type SeriesMarkerShape,
  } from "lightweight-charts";

  let { symbol = null as string | null } = $props();

  type Candle = { time: string; open: number; high: number; low: number; close: number; volume: number };
  type SymbolEvent = { kind: string; time: string; thesis_id: string; label: string; detail: string };
  type Range = "1M" | "3M" | "6M" | "YTD" | "1Y" | "ALL";
  const RANGES: Range[] = ["1M", "3M", "6M", "YTD", "1Y", "ALL"];
  let range = $state<Range>("6M");

  let container: HTMLDivElement | null = null;
  let chart: IChartApi | null = null;
  let priceSeries: ISeriesApi<"Candlestick"> | null = null;
  let volSeries: ISeriesApi<"Histogram"> | null = null;
  let markersApi: ISeriesMarkersPluginApi<Time> | null = null;
  let candles = $state<Candle[] | null>(null);
  let events = $state<SymbolEvent[]>([]);
  let error = $state<string | null>(null);
  let loading = $state(false);

  function toUtc(time: string): UTCTimestamp {
    // lightweight-charts wants seconds since epoch for time-based charts.
    return Math.floor(new Date(time + "T00:00:00Z").getTime() / 1000) as UTCTimestamp;
  }

  async function load(sym: string, rng: Range) {
    loading = true;
    error = null;
    try {
      const [cRes, eRes] = await Promise.all([
        fetch(`/api/candles?symbol=${encodeURIComponent(sym)}&range=${rng}`),
        fetch(`/api/symbol-events?symbol=${encodeURIComponent(sym)}&range=${rng}`),
      ]);
      if (!cRes.ok) throw new Error(`candles ${cRes.status}`);
      candles = (await cRes.json()) as Candle[];
      events = eRes.ok ? ((await eRes.json()) as SymbolEvent[]) : [];
      render();
    } catch (e) {
      error = String(e);
      candles = null;
    } finally {
      loading = false;
    }
  }

  function eventStyle(ev: SymbolEvent): { color: string; shape: SeriesMarkerShape; position: SeriesMarkerBarPosition; text: string } {
    switch (ev.kind) {
      case "state_transition": {
        const promoted = ["actionable", "position_open", "armed"].includes(ev.label);
        const killed = ["disqualified", "closed"].includes(ev.label);
        return {
          color: promoted ? "#a6e3a1" : killed ? "#f38ba8" : "#89b4fa",
          shape: promoted ? "arrowUp" : "circle",
          position: promoted ? "belowBar" : "aboveBar",
          text: ev.label.replace(/_/g, " "),
        };
      }
      case "risk":
        return { color: "#f9e2af", shape: "square", position: "aboveBar", text: ev.label };
      case "decision":
        return {
          color: ev.detail === "confirmed" ? "#94e2d5" : "#cba6f7",
          shape: "square",
          position: "belowBar",
          text: ev.label,
        };
      default:
        return { color: "#bac2de", shape: "circle", position: "aboveBar", text: ev.kind };
    }
  }

  function applyMarkers() {
    if (!priceSeries || !markersApi) return;
    const dedup = new Map<string, SeriesMarker<Time>>();
    for (const e of events) {
      const t = toUtc(e.time);
      const sty = eventStyle(e);
      // Dedup multiple same-day same-kind events to one marker.
      const k = `${t}-${e.kind}-${sty.text}`;
      if (!dedup.has(k)) {
        dedup.set(k, { time: t, position: sty.position, color: sty.color, shape: sty.shape, text: sty.text });
      }
    }
    markersApi.setMarkers([...dedup.values()].sort((a, b) => (a.time as number) - (b.time as number)));
  }

  function ensureChart() {
    if (!container || chart) return;
    chart = createChart(container, {
      autoSize: true,
      layout: { background: { color: "#0b0e14" }, textColor: "#bac2de" },
      localization: { locale: "en-US" },
      grid: { vertLines: { color: "#1f2733" }, horzLines: { color: "#1f2733" } },
      timeScale: { borderColor: "#2a3548", timeVisible: false, rightOffset: 4 },
      rightPriceScale: { borderColor: "#2a3548" },
      crosshair: { mode: 0 }, // Normal
    });
    priceSeries = chart.addSeries(CandlestickSeries, {
      upColor: "#a6e3a1", downColor: "#f38ba8",
      borderUpColor: "#a6e3a1", borderDownColor: "#f38ba8",
      wickUpColor: "#a6e3a1", wickDownColor: "#f38ba8",
    });
    volSeries = chart.addSeries(HistogramSeries, {
      priceFormat: { type: "volume" },
      priceScaleId: "vol",
      color: "#6c7693",
    });
    chart.priceScale("vol").applyOptions({
      scaleMargins: { top: 0.85, bottom: 0 },
      borderColor: "#2a3548",
    });
    markersApi = createSeriesMarkers(priceSeries, []);
  }

  function render() {
    ensureChart();
    if (!chart || !priceSeries || !volSeries || !candles) return;
    const cs: CandlestickData[] = candles.map((c) => ({
      time: toUtc(c.time),
      open: c.open, high: c.high, low: c.low, close: c.close,
    }));
    const vs: HistogramData[] = candles.map((c) => ({
      time: toUtc(c.time),
      value: c.volume,
      color: c.close >= c.open ? "rgba(166, 227, 161, 0.4)" : "rgba(243, 139, 168, 0.4)",
    }));
    priceSeries.setData(cs);
    volSeries.setData(vs);
    applyMarkers();
    chart.timeScale().fitContent();
  }

  $effect(() => {
    if (symbol) load(symbol, range);
  });

  onMount(() => {
    return () => {
      chart?.remove();
      chart = null;
      priceSeries = null;
      volSeries = null;
      markersApi = null;
    };
  });
</script>

<div class="chart-wrap">
  <div class="toolbar">
    <strong class="symbol-label">{symbol ?? "—"}</strong>
    {#if candles && candles.length > 0}
      {@const last = candles[candles.length - 1]}
      <span class="meta">
        <span class="muted">close</span>
        <strong>{last.close.toFixed(2)}</strong>
      </span>
      <span class="meta">
        <span class="muted">{candles.length} bars</span>
      </span>
    {/if}
    {#if events.length > 0}
      <span class="meta"><span class="muted">{events.length} events</span></span>
    {/if}
    <span class="range-picker">
      {#each RANGES as r}
        <button class:active={range === r} onclick={() => (range = r)}>{r}</button>
      {/each}
    </span>
    {#if loading}<span class="muted">loading…</span>{/if}
    {#if error}<span class="err">{error}</span>{/if}
  </div>

  <div class="chart" bind:this={container}>
    {#if !symbol}
      <div class="empty muted">Select a symbol on the right.</div>
    {:else if candles && candles.length === 0 && !loading}
      <div class="empty muted">
        No price bars for {symbol} in this range.
        Run <code>make run-ingest</code> to backfill.
      </div>
    {/if}
  </div>
</div>

<style>
  .chart-wrap {
    display: flex; flex-direction: column;
    height: 100%; min-height: 0;
    background: #0b0e14;
  }
  .toolbar {
    display: flex; gap: .75rem; align-items: baseline;
    padding: .35rem .75rem;
    border-bottom: 1px solid #1f2733;
    font-size: .85rem;
    flex-shrink: 0;
  }
  .symbol-label { font-size: .95rem; }
  .meta { display: flex; gap: .3rem; align-items: baseline; }
  .range-picker { margin-left: auto; display: flex; gap: .15rem; }
  .range-picker button {
    background: #11161f; color: #6c7693; border: 1px solid #1f2733;
    border-radius: 3px; padding: .15rem .5rem; cursor: pointer; font: inherit;
    font-size: .75rem;
  }
  .range-picker button:hover { color: #cdd6f4; border-color: #2a3548; }
  .range-picker button.active {
    background: #2a3548; color: #cdd6f4; border-color: #45567a;
  }
  .err { color: #f38ba8; font-size: .8rem; }
  .chart {
    flex: 1 1 auto; min-height: 0;
    position: relative;
  }
  .empty {
    position: absolute; inset: 0;
    display: flex; align-items: center; justify-content: center;
    padding: 1rem; text-align: center;
  }
</style>
