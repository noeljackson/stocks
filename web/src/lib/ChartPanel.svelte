<script lang="ts">
  // ChartPanel — TradingView-style interval controls over lightweight-charts.
  // Renders OHLC + volume in the workspace chart pane.
  import { onMount } from "svelte";
  import type { StreamEvent } from "./api";
  import {
    createChart,
    CandlestickSeries,
    HistogramSeries,
    LineSeries,
    LineStyle,
    createSeriesMarkers,
    type IChartApi,
    type ISeriesApi,
    type ISeriesMarkersPluginApi,
    type CandlestickData,
    type HistogramData,
    type LineData,
    type UTCTimestamp,
    type Time,
    type SeriesMarker,
    type SeriesMarkerBarPosition,
    type SeriesMarkerShape,
  } from "lightweight-charts";

  type Candle = { time: string; open: number; high: number; low: number; close: number; volume: number };
  type SymbolEvent = { kind: string; time: string; thesis_id: string; label: string; detail: string };
  type ChartCoverage = {
    start: string | null;
    end: string | null;
    bars: number;
    fetchAttempts: number;
    fetchError: string | null;
  };
  type Interval = "1m" | "3m" | "5m" | "15m" | "30m" | "1h" | "2h" | "4h" | "1D" | "1W" | "3W" | "1M";
  type Range = "1D" | "5D" | "1M" | "3M" | "6M" | "200D" | "1Y" | "2Y" | "ALL";
  type ChartState = { interval: Interval; range: Range };
  type LiveStatus = "disconnected" | "listening" | "live" | "delayed" | "market_closed" | "entitlement_blocked" | "rate_limited";
  let {
    symbol = null as string | null,
    liveEvents = [] as StreamEvent[],
    streamConnected = false,
    onStateChange = (_state: ChartState) => {},
  } = $props();

  const INTERVALS: Interval[] = ["1m", "3m", "5m", "15m", "30m", "1h", "2h", "4h", "1D", "1W", "3W", "1M"];
  const RANGES: Range[] = ["1D", "5D", "1M", "3M", "6M", "200D", "1Y", "2Y", "ALL"];
  const INTRADAY_INTERVALS = new Set<Interval>(["1m", "3m", "5m", "15m", "30m", "1h", "2h", "4h"]);
  const SMA_WINDOWS = [20, 50, 100, 200] as const;
  const SMA_COLORS: Record<(typeof SMA_WINDOWS)[number], string> = {
    20: "#f9e2af",
    50: "#89b4fa",
    100: "#cba6f7",
    200: "#94e2d5",
  };
  let interval = $state<Interval>("1D");
  let range = $state<Range>("ALL");

  let container: HTMLDivElement | null = null;
  let chart: IChartApi | null = null;
  let priceSeries: ISeriesApi<"Candlestick"> | null = null;
  let volSeries: ISeriesApi<"Histogram"> | null = null;
  let rsiSeries: ISeriesApi<"Line"> | null = null;
  let psoSeries: ISeriesApi<"Line"> | null = null;
  const smaSeries = new Map<number, ISeriesApi<"Line">>();
  let markersApi: ISeriesMarkersPluginApi<Time> | null = null;
  let candles = $state<Candle[] | null>(null);
  let smaCandles = $state<Candle[] | null>(null);
  let events = $state<SymbolEvent[]>([]);
  let coverage = $state<ChartCoverage | null>(null);
  let error = $state<string | null>(null);
  let loading = $state(false);
  let liveStatus = $state<LiveStatus>("disconnected");
  let loadSeq = 0;
  let liveScope = "";
  let seenLiveEventKeys = new Set<string>();

  function toUtc(time: string): UTCTimestamp {
    // lightweight-charts wants seconds since epoch for time-based charts.
    const stamp = time.includes("T") ? time : `${time}T00:00:00Z`;
    return Math.floor(new Date(stamp).getTime() / 1000) as UTCTimestamp;
  }

  function normalizeInterval(value: string | null | undefined): Interval | null {
    if (!value) return null;
    const raw = value.trim();
    const normalized = raw.match(/^[0-9]+[dwm]$/i) ? `${raw.slice(0, -1)}${raw.slice(-1).toUpperCase()}` : raw;
    return INTERVALS.includes(normalized as Interval) ? normalized as Interval : null;
  }

  function valueText(value: unknown): string | null {
    return typeof value === "string" && value.trim() ? value.trim() : null;
  }

  function valueNumber(value: unknown): number | null {
    if (typeof value === "number" && Number.isFinite(value)) return value;
    if (typeof value === "string" && value.trim()) {
      const parsed = Number(value);
      if (Number.isFinite(parsed)) return parsed;
    }
    return null;
  }

  function liveStatusLabel(): string {
    if (!streamConnected) return "disconnected";
    switch (liveStatus) {
      case "live": return "live";
      case "delayed": return "delayed";
      case "market_closed": return "market closed";
      case "entitlement_blocked": return "entitlement blocked";
      case "rate_limited": return "rate limited";
      case "disconnected": return "disconnected";
      default: return "listening";
    }
  }

  function marketEventScope(event: StreamEvent): { symbol: string | null; interval: Interval | null } {
    const subjectParts = event.subject.split(".");
    const subjectInterval = subjectParts[0] === "market" && subjectParts[1] === "bar"
      ? normalizeInterval(subjectParts[2])
      : null;
    const subjectSymbol = subjectParts[0] === "market" && subjectParts[1] === "bar" && subjectParts.length >= 4
      ? subjectParts.slice(3).join(".").toUpperCase()
      : null;
    const payloadSymbol = valueText(event.payload.symbol)?.toUpperCase() ?? null;
    const payloadInterval = normalizeInterval(valueText(event.payload.interval));
    return {
      symbol: payloadSymbol ?? subjectSymbol,
      interval: payloadInterval ?? subjectInterval,
    };
  }

  function candleFromMarketEvent(event: StreamEvent): Candle | null {
    const time = valueText(event.payload.time)
      ?? valueText(event.payload.ts)
      ?? valueText(event.payload.start)
      ?? valueText(event.payload.at);
    const close = valueNumber(event.payload.close ?? event.payload.c);
    if (!time || close === null) return null;
    const open = valueNumber(event.payload.open ?? event.payload.o) ?? close;
    const high = valueNumber(event.payload.high ?? event.payload.h) ?? Math.max(open, close);
    const low = valueNumber(event.payload.low ?? event.payload.l) ?? Math.min(open, close);
    const volume = valueNumber(event.payload.volume ?? event.payload.v) ?? 0;
    return { time, open, high, low, close, volume };
  }

  function updateLiveStatus(event: StreamEvent) {
    const status = valueText(event.payload.status)?.toLowerCase();
    if (status && ["live", "delayed", "market_closed", "entitlement_blocked", "rate_limited", "disconnected"].includes(status)) {
      liveStatus = status as LiveStatus;
      return;
    }
    liveStatus = streamConnected ? "live" : "disconnected";
  }

  function mergeLiveCandle(next: Candle) {
    if (loading || !candles) return false;
    const merged = [...candles];
    const nextTs = toUtc(next.time);
    const existingIndex = merged.findIndex((c) => toUtc(c.time) === nextTs);
    if (existingIndex >= 0) {
      const current = merged[existingIndex];
      merged[existingIndex] = {
        ...current,
        ...next,
        high: Math.max(current.high, next.high),
        low: Math.min(current.low, next.low),
        volume: next.volume || current.volume,
      };
    } else {
      merged.push(next);
      merged.sort((a, b) => (toUtc(a.time) as number) - (toUtc(b.time) as number));
    }
    candles = merged;
    if (interval === "1D" && smaCandles) {
      const daily = [...smaCandles];
      const dailyIndex = daily.findIndex((c) => toUtc(c.time) === nextTs);
      if (dailyIndex >= 0) daily[dailyIndex] = { ...daily[dailyIndex], ...next };
      else daily.push(next);
      daily.sort((a, b) => (toUtc(a.time) as number) - (toUtc(b.time) as number));
      smaCandles = daily;
    }
    render();
    return true;
  }

  function liveEventKey(event: StreamEvent): string {
    const time = valueText(event.payload.time) ?? valueText(event.payload.ts) ?? valueText(event.payload.start) ?? "";
    const close = String(event.payload.close ?? event.payload.c ?? "");
    return `${event.subject}|${time}|${close}`;
  }

  function applyLiveMarketEvent(event: StreamEvent) {
    if (event.kind !== "market_bar" && !event.subject.startsWith("market.bar.")) return;
    const scope = marketEventScope(event);
    if (!symbol || scope.symbol !== symbol.toUpperCase() || scope.interval !== interval) return;
    updateLiveStatus(event);
    const next = candleFromMarketEvent(event);
    if (!next) return;
    const key = liveEventKey(event);
    if (seenLiveEventKeys.has(key)) return;
    if (mergeLiveCandle(next)) seenLiveEventKeys.add(key);
  }

  function isIntraday(i: Interval) {
    return INTRADAY_INTERVALS.has(i);
  }

  function chooseInterval(next: Interval) {
    interval = next;
  }

  function chooseRange(next: Range) {
    range = next;
  }

  function smaLabel(window: number) {
    return `SMA ${window}D`;
  }

  async function load(sym: string, rng: Range, intv: Interval) {
    const seq = ++loadSeq;
    loading = true;
    error = null;
    candles = [];
    smaCandles = [];
    events = [];
    coverage = null;
    clearSeries();
    try {
      const [cRes, eRes, sRes] = await Promise.all([
        fetch(`/api/candles?symbol=${encodeURIComponent(sym)}&range=${rng}&interval=${encodeURIComponent(intv)}`),
        fetch(`/api/symbol-events?symbol=${encodeURIComponent(sym)}&range=${rng}&interval=${encodeURIComponent(intv)}`),
        intv === "1D" && rng === "ALL"
          ? Promise.resolve(null)
          : fetch(`/api/candles?symbol=${encodeURIComponent(sym)}&range=ALL&interval=1D`),
      ]);
      if (!cRes.ok) throw new Error(await cRes.text() || `candles ${cRes.status}`);
      const nextCandles = (await cRes.json()) as Candle[];
      const nextCoverage: ChartCoverage = {
        start: cRes.headers.get("x-chart-coverage-start"),
        end: cRes.headers.get("x-chart-coverage-end"),
        bars: Number(cRes.headers.get("x-chart-bars") ?? nextCandles.length),
        fetchAttempts: Number(cRes.headers.get("x-chart-fetch-attempts") ?? 0),
        fetchError: cRes.headers.get("x-chart-fetch-error"),
      };
      let nextSmaCandles = nextCandles;
      if (sRes?.ok) nextSmaCandles = (await sRes.json()) as Candle[];
      if (seq !== loadSeq) return;
      candles = nextCandles;
      smaCandles = nextSmaCandles;
      events = eRes.ok ? ((await eRes.json()) as SymbolEvent[]) : [];
      coverage = nextCoverage;
      render();
    } catch (e) {
      if (seq !== loadSeq) return;
      error = String(e);
      candles = [];
      smaCandles = [];
      events = [];
      coverage = null;
      render();
    } finally {
      if (seq === loadSeq) loading = false;
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

  function clearSeries() {
    ensureChart();
    priceSeries?.setData([]);
    volSeries?.setData([]);
    rsiSeries?.setData([]);
    psoSeries?.setData([]);
    for (const series of smaSeries.values()) series.setData([]);
    markersApi?.setMarkers([]);
  }

  function smaData(window: number): LineData[] {
    if (!smaCandles || smaCandles.length < window || !candles) return [];
    const dailyPoints: { date: string; value: number }[] = [];
    let rolling = 0;
    for (let i = 0; i < smaCandles.length; i += 1) {
      rolling += smaCandles[i].close;
      if (i >= window) rolling -= smaCandles[i - window].close;
      if (i >= window - 1) dailyPoints.push({ date: smaCandles[i].time.slice(0, 10), value: rolling / window });
    }

    if (interval === "1D") {
      const visibleTimes = new Set(candles.map((c) => c.time));
      return dailyPoints
        .filter((point) => visibleTimes.has(point.date))
        .map((point) => ({ time: toUtc(point.date), value: point.value }));
    }

    const out: LineData[] = [];
    let dailyIndex = 0;
    for (const candle of candles) {
      const date = candle.time.slice(0, 10);
      while (dailyIndex + 1 < dailyPoints.length && dailyPoints[dailyIndex + 1].date <= date) dailyIndex += 1;
      if (dailyPoints[dailyIndex]?.date <= date) out.push({ time: toUtc(candle.time), value: dailyPoints[dailyIndex].value });
    }
    return out;
  }

  function hasSma(window: number) {
    return smaData(window).length > 0;
  }

  function hasAnySma() {
    return SMA_WINDOWS.some((window) => hasSma(window));
  }

  function rsiData(window = 14): LineData[] {
    if (!candles || candles.length <= window) return [];
    const out: LineData[] = [];
    let gains = 0;
    let losses = 0;
    for (let i = 1; i <= window; i += 1) {
      const change = candles[i].close - candles[i - 1].close;
      if (change >= 0) gains += change;
      else losses -= change;
    }
    let avgGain = gains / window;
    let avgLoss = losses / window;
    const toRsi = () => (avgLoss === 0 ? 100 : 100 - (100 / (1 + avgGain / avgLoss)));
    out.push({ time: toUtc(candles[window].time), value: toRsi() });

    for (let i = window + 1; i < candles.length; i += 1) {
      const change = candles[i].close - candles[i - 1].close;
      const gain = change > 0 ? change : 0;
      const loss = change < 0 ? -change : 0;
      avgGain = ((avgGain * (window - 1)) + gain) / window;
      avgLoss = ((avgLoss * (window - 1)) + loss) / window;
      out.push({ time: toUtc(candles[i].time), value: toRsi() });
    }
    return out;
  }

  function emaOptional(values: (number | null)[], window: number): (number | null)[] {
    const out = values.map(() => null as number | null);
    if (window <= 0) return out;
    const alpha = 2 / (window + 1);
    let ema: number | null = null;
    values.forEach((value, idx) => {
      if (value === null) return;
      ema = ema === null ? value : (alpha * value) + ((1 - alpha) * ema);
      out[idx] = ema;
    });
    return out;
  }

  function psoData(stochasticWindow = 8, smoothingWindow = 5): LineData[] {
    if (!candles || candles.length < stochasticWindow) return [];
    const stochastic = candles.map((c, idx) => {
      if (idx + 1 < stochasticWindow) return null;
      const slice = candles!.slice(idx + 1 - stochasticWindow, idx + 1);
      const high = Math.max(...slice.map((bar) => bar.high));
      const low = Math.min(...slice.map((bar) => bar.low));
      const range = high - low;
      if (Math.abs(range) < Number.EPSILON) return 50;
      return Math.max(0, Math.min(100, ((c.close - low) / range) * 100));
    });
    const normalized = stochastic.map((value) => value === null ? null : 0.1 * (value - 50));
    const first = emaOptional(normalized, smoothingWindow);
    const second = emaOptional(first, smoothingWindow);
    return second.flatMap((value, idx) => {
      if (value === null) return [];
      const exp = Math.exp(value);
      return [{ time: toUtc(candles![idx].time), value: (exp - 1) / (exp + 1) }];
    });
  }

  function ensureChart() {
    if (!container || chart) return;
    chart = createChart(container, {
      autoSize: true,
      layout: { background: { color: "#0b0e14" }, textColor: "#bac2de" },
      localization: { locale: "en-US" },
      grid: { vertLines: { color: "#1f2733" }, horzLines: { color: "#1f2733" } },
      timeScale: { borderColor: "#2a3548", timeVisible: isIntraday(interval), rightOffset: 4 },
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
    for (const window of SMA_WINDOWS) {
      const series = chart.addSeries(LineSeries, {
        color: SMA_COLORS[window],
        lineWidth: window === 200 ? 2 : 1,
        priceLineVisible: false,
        lastValueVisible: true,
        crosshairMarkerVisible: false,
        title: smaLabel(window),
      });
      smaSeries.set(window, series);
    }
    rsiSeries = chart.addSeries(LineSeries, {
      color: "#fab387",
      lineWidth: 2,
      priceLineVisible: false,
      lastValueVisible: true,
      crosshairMarkerVisible: true,
      priceScaleId: "rsi",
      title: "RSI 14",
      priceFormat: { type: "price", precision: 1, minMove: 0.1 },
    }, 1);
    chart.priceScale("rsi", 1).applyOptions({
      scaleMargins: { top: 0.12, bottom: 0.12 },
      borderColor: "#2a3548",
    });
    rsiSeries.createPriceLine({
      price: 70,
      color: "rgba(243, 139, 168, 0.7)",
      lineWidth: 1,
      lineStyle: LineStyle.Dotted,
      axisLabelVisible: true,
      title: "70",
    });
    rsiSeries.createPriceLine({
      price: 50,
      color: "rgba(186, 194, 222, 0.35)",
      lineWidth: 1,
      lineStyle: LineStyle.Dotted,
      axisLabelVisible: false,
      title: "50",
    });
    rsiSeries.createPriceLine({
      price: 30,
      color: "rgba(166, 227, 161, 0.7)",
      lineWidth: 1,
      lineStyle: LineStyle.Dotted,
      axisLabelVisible: true,
      title: "30",
    });
    psoSeries = chart.addSeries(LineSeries, {
      color: "#94e2d5",
      lineWidth: 2,
      priceLineVisible: false,
      lastValueVisible: true,
      crosshairMarkerVisible: true,
      priceScaleId: "pso",
      title: "PSO 8/25",
      priceFormat: { type: "price", precision: 2, minMove: 0.01 },
    }, 2);
    chart.priceScale("pso", 2).applyOptions({
      scaleMargins: { top: 0.12, bottom: 0.12 },
      borderColor: "#2a3548",
    });
    psoSeries.createPriceLine({
      price: 0.9,
      color: "rgba(243, 139, 168, 0.7)",
      lineWidth: 1,
      lineStyle: LineStyle.Dotted,
      axisLabelVisible: true,
      title: "+0.9",
    });
    psoSeries.createPriceLine({
      price: 0.2,
      color: "rgba(186, 194, 222, 0.35)",
      lineWidth: 1,
      lineStyle: LineStyle.Dotted,
      axisLabelVisible: false,
      title: "+0.2",
    });
    psoSeries.createPriceLine({
      price: -0.2,
      color: "rgba(186, 194, 222, 0.35)",
      lineWidth: 1,
      lineStyle: LineStyle.Dotted,
      axisLabelVisible: false,
      title: "-0.2",
    });
    psoSeries.createPriceLine({
      price: -0.9,
      color: "rgba(166, 227, 161, 0.7)",
      lineWidth: 1,
      lineStyle: LineStyle.Dotted,
      axisLabelVisible: true,
      title: "-0.9",
    });
    chart.panes()[0]?.setStretchFactor(4);
    chart.panes()[1]?.setStretchFactor(1);
    chart.panes()[2]?.setStretchFactor(1);
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
    rsiSeries?.setData(rsiData(14));
    psoSeries?.setData(psoData());
    chart.timeScale().applyOptions({ timeVisible: isIntraday(interval), secondsVisible: false });
    for (const window of SMA_WINDOWS) {
      smaSeries.get(window)?.applyOptions({ title: smaLabel(window) });
      smaSeries.get(window)?.setData(smaData(window));
    }
    applyMarkers();
    if (cs.length > 0) chart.timeScale().fitContent();
  }

  $effect(() => {
    onStateChange({ interval, range });
    if (symbol) load(symbol, range, interval);
  });

  $effect(() => {
    const scope = `${symbol ?? ""}|${interval}`;
    if (scope !== liveScope) {
      liveScope = scope;
      seenLiveEventKeys = new Set<string>();
      liveStatus = streamConnected ? "listening" : "disconnected";
    } else if (!streamConnected) {
      liveStatus = "disconnected";
    } else if (liveStatus === "disconnected") {
      liveStatus = "listening";
    }
    for (const event of [...liveEvents].reverse()) applyLiveMarketEvent(event);
  });

  onMount(() => {
    return () => {
      chart?.remove();
      chart = null;
      priceSeries = null;
      volSeries = null;
      rsiSeries = null;
      psoSeries = null;
      smaSeries.clear();
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
        <strong data-testid="chart-last-close">{last.close.toFixed(2)}</strong>
      </span>
      <span class="meta">
        <span class="muted">{candles.length} bars</span>
      </span>
    {/if}
    {#if coverage?.start && coverage?.end}
      <span class="meta coverage" title={coverage.fetchError ?? ""}>
        <span class="muted">coverage</span>
        <strong>{coverage.start.slice(0, 10)} to {coverage.end.slice(0, 10)}</strong>
        {#if coverage.fetchAttempts > 0}
          <span class="muted">{coverage.fetchAttempts} fetches</span>
        {/if}
        {#if coverage.fetchError}
          <span class="err">partial</span>
        {/if}
      </span>
    {/if}
    <span class="meta" data-testid="chart-interval-status">
      <span class="muted">interval</span>
      <strong>{interval}</strong>
      <span class="muted">{range}</span>
    </span>
    <span class="meta" data-testid="chart-live-status">
      <span class="muted">market data</span>
      <strong>{liveStatusLabel()}</strong>
    </span>
    {#if events.length > 0}
      <span class="meta"><span class="muted">{events.length} events</span></span>
    {/if}
    {#if hasAnySma()}
      <span class="sma-legend" aria-label="SMA ribbon">
        {#each SMA_WINDOWS as window}
          {#if hasSma(window)}
            <span class="sma-key" style={`--sma-color: ${SMA_COLORS[window]}`}>{smaLabel(window)}</span>
          {/if}
        {/each}
      </span>
    {/if}
    {#if candles && candles.length > 14}
      <span class="rsi-key" data-testid="rsi-legend">RSI 14</span>
    {/if}
    {#if candles && candles.length > 25}
      <span class="pso-key" data-testid="pso-legend">PSO 8/25</span>
    {/if}
    <span class="interval-picker" aria-label="Chart interval">
      {#each INTERVALS as intv}
        <button
          class:active={interval === intv}
          data-testid={`interval-${intv}`}
          onclick={() => chooseInterval(intv)}
        >{intv}</button>
      {/each}
    </span>
    <span class="range-picker" aria-label="Chart range">
      {#each RANGES as r}
        <button
          class:active={range === r}
          data-testid={`range-${r}`}
          onclick={() => chooseRange(r)}
        >{r}</button>
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
        No price bars for {symbol} at {interval} in this range.
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
  .sma-legend {
    display: flex; gap: .45rem; align-items: baseline; flex-wrap: wrap;
    font-size: .72rem;
  }
  .sma-key {
    color: #bac2de;
    display: inline-flex; gap: .25rem; align-items: center;
  }
  .sma-key::before {
    content: ""; width: .9rem; height: 2px; background: var(--sma-color);
    display: inline-block; border-radius: 2px;
  }
  .rsi-key {
    color: #fab387;
    font-size: .72rem;
    white-space: nowrap;
  }
  .pso-key {
    color: #94e2d5;
    font-size: .72rem;
    white-space: nowrap;
  }
  .interval-picker { margin-left: auto; }
  .interval-picker,
  .range-picker { display: flex; gap: .15rem; flex-wrap: wrap; justify-content: flex-end; }
  .interval-picker button,
  .range-picker button {
    background: #11161f; color: #6c7693; border: 1px solid #1f2733;
    border-radius: 3px; padding: .15rem .5rem; cursor: pointer; font: inherit;
    font-size: .75rem;
  }
  .interval-picker button:hover,
  .range-picker button:hover { color: #cdd6f4; border-color: #2a3548; }
  .interval-picker button.active,
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
