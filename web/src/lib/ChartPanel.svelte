<script lang="ts">
  // ChartPanel — TradingView-style interval controls over lightweight-charts.
  // Renders OHLC + volume in the workspace chart pane.
  import { onMount } from "svelte";
  import {
    createPriceAlert,
    disablePriceAlert,
    fetchPriceAlertEvents,
    fetchPriceAlerts,
    type PriceAlertEvent,
    type PriceAlertRule,
    type StreamEvent,
  } from "./api";
  import {
    createChart,
    CandlestickSeries,
    CrosshairMode,
    HistogramSeries,
    LineSeries,
    LineStyle,
    createSeriesMarkers,
    type IChartApi,
    type IPriceLine,
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
  let crosshairMode = $state<"magnet" | "free" | "hidden">("magnet");
  let indicatorsOpen = $state(false);
  let showVolume = $state(true);
  let showRsi = $state(true);
  let showPso = $state(true);
  let showPso32 = $state(true);
  let visibleSma = $state<Record<number, boolean>>({ 20: true, 50: true, 100: true, 200: true });
  let alertMenuOpen = $state(false);
  let alertDirection = $state<"above" | "below">("above");
  let alertIntent = $state<"watch" | "entry" | "invalidation" | "exit">("watch");
  let alertTarget = $state("");
  let alertLabel = $state("");
  let alertSaving = $state(false);
  let alertError = $state<string | null>(null);

  let container: HTMLDivElement | null = null;
  let chart: IChartApi | null = null;
  let priceSeries: ISeriesApi<"Candlestick"> | null = null;
  let volSeries: ISeriesApi<"Histogram"> | null = null;
  let rsiSeries: ISeriesApi<"Line"> | null = null;
  let psoSeries: ISeriesApi<"Line"> | null = null;
  let pso32Series: ISeriesApi<"Line"> | null = null;
  const smaSeries = new Map<number, ISeriesApi<"Line">>();
  let markersApi: ISeriesMarkersPluginApi<Time> | null = null;
  let priceAlertLines: IPriceLine[] = [];
  let candles = $state<Candle[] | null>(null);
  let smaCandles = $state<Candle[] | null>(null);
  let events = $state<SymbolEvent[]>([]);
  let priceAlerts = $state<PriceAlertRule[]>([]);
  let priceAlertEvents = $state<PriceAlertEvent[]>([]);
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

  function lastClose() {
    return candles && candles.length > 0 ? candles[candles.length - 1].close : null;
  }

  function openAlertMenu() {
    const close = lastClose();
    alertTarget = close === null ? "" : close.toFixed(2);
    alertLabel = symbol ? `${symbol} price watch` : "price watch";
    alertDirection = "above";
    alertIntent = "watch";
    alertError = null;
    alertMenuOpen = !alertMenuOpen;
  }

  function setCrosshairMode(next: "magnet" | "free" | "hidden") {
    crosshairMode = next;
    chart?.applyOptions({ crosshair: { mode: crosshairModeValue() } });
  }

  function crosshairModeValue() {
    if (crosshairMode === "free") return CrosshairMode.Normal;
    if (crosshairMode === "hidden") return CrosshairMode.Hidden;
    return CrosshairMode.Magnet;
  }

  function fitVisibleRange() {
    chart?.timeScale().fitContent();
  }

  function toggleSma(window: number) {
    visibleSma = { ...visibleSma, [window]: !visibleSma[window] };
    render();
  }

  function resetIndicators() {
    visibleSma = { 20: true, 50: true, 100: true, 200: true };
    showVolume = true;
    showRsi = true;
    showPso = true;
    showPso32 = true;
    render();
  }

  async function refreshPriceAlerts(sym: string) {
    const [rules, triggered] = await Promise.all([
      fetchPriceAlerts({ symbol: sym }).catch(() => []),
      fetchPriceAlertEvents({ symbol: sym }).catch(() => []),
    ]);
    priceAlerts = rules;
    priceAlertEvents = triggered;
    render();
  }

  async function saveManualAlert() {
    if (!symbol) return;
    const target = Number(alertTarget);
    if (!Number.isFinite(target) || target <= 0) {
      alertError = "Enter a positive target price.";
      return;
    }
    alertSaving = true;
    alertError = null;
    try {
      const created = await createPriceAlert({
        symbol,
        origin: "manual",
        intent: alertIntent,
        direction: alertDirection,
        target_price: target,
        label: alertLabel.trim() || `${symbol} ${alertDirection} ${target.toFixed(2)}`,
        rationale: "Manual chart alert",
        source_ref: { surface: "chart" },
      });
      priceAlerts = [created, ...priceAlerts.filter((rule) => rule.id !== created.id)];
      alertMenuOpen = false;
      render();
    } catch (e) {
      alertError = e instanceof Error ? e.message : String(e);
    } finally {
      alertSaving = false;
    }
  }

  async function disableRule(rule: PriceAlertRule) {
    try {
      const updated = await disablePriceAlert(rule.id);
      priceAlerts = priceAlerts.map((row) => row.id === updated.id ? updated : row);
      render();
    } catch (e) {
      alertError = e instanceof Error ? e.message : String(e);
    }
  }

  function activeIndicatorCount() {
    return SMA_WINDOWS.filter((window) => visibleSma[window]).length
      + (showVolume ? 1 : 0)
      + (showRsi ? 1 : 0)
      + (showPso ? 1 : 0)
      + (showPso32 ? 1 : 0);
  }

  function lastChange() {
    if (!candles || candles.length < 2) return null;
    const last = candles[candles.length - 1];
    const prev = candles[candles.length - 2];
    if (prev.close === 0) return null;
    const diff = last.close - prev.close;
    const pct = (diff / prev.close) * 100;
    return { diff, pct, up: diff >= 0 };
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
    priceAlerts = [];
    priceAlertEvents = [];
    coverage = null;
    clearSeries();
    try {
      const [cRes, eRes, sRes, nextPriceAlerts, nextPriceAlertEvents] = await Promise.all([
        fetch(`/api/candles?symbol=${encodeURIComponent(sym)}&range=${rng}&interval=${encodeURIComponent(intv)}`),
        fetch(`/api/symbol-events?symbol=${encodeURIComponent(sym)}&range=${rng}&interval=${encodeURIComponent(intv)}`),
        intv === "1D" && rng === "ALL"
          ? Promise.resolve(null)
          : fetch(`/api/candles?symbol=${encodeURIComponent(sym)}&range=ALL&interval=1D`),
        fetchPriceAlerts({ symbol: sym }).catch(() => []),
        fetchPriceAlertEvents({ symbol: sym }).catch(() => []),
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
      priceAlerts = nextPriceAlerts;
      priceAlertEvents = nextPriceAlertEvents;
      coverage = nextCoverage;
      render();
    } catch (e) {
      if (seq !== loadSeq) return;
      error = String(e);
      candles = [];
      smaCandles = [];
      events = [];
      priceAlerts = [];
      priceAlertEvents = [];
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
    for (const event of priceAlertEvents) {
      const snapshot = event.rule_snapshot as Partial<PriceAlertRule> | undefined;
      const label = typeof snapshot?.label === "string" ? snapshot.label : "price alert";
      dedup.set(`price-alert-${event.id}`, {
        time: toUtc(event.trigger_ts),
        position: "aboveBar",
        color: "#f9e2af",
        shape: "square",
        text: label,
      });
    }
    markersApi.setMarkers([...dedup.values()].sort((a, b) => (a.time as number) - (b.time as number)));
  }

  function clearPriceAlertLines() {
    if (!priceSeries) {
      priceAlertLines = [];
      return;
    }
    for (const line of priceAlertLines) priceSeries.removePriceLine(line);
    priceAlertLines = [];
  }

  function applyPriceAlertLines() {
    if (!priceSeries) return;
    clearPriceAlertLines();
    priceAlertLines = priceAlerts
      .filter((rule) => rule.status === "active")
      .map((rule) => priceSeries!.createPriceLine({
        price: rule.target_price,
        color: rule.origin === "ai" ? "#89b4fa" : "#f9e2af",
        lineWidth: 1,
        lineStyle: rule.direction === "above" ? LineStyle.Dashed : LineStyle.Dotted,
        axisLabelVisible: true,
        title: `${rule.origin === "ai" ? "AI" : "Alert"} ${rule.intent}`,
      }));
  }

  function clearSeries() {
    ensureChart();
    priceSeries?.setData([]);
    volSeries?.setData([]);
    rsiSeries?.setData([]);
    psoSeries?.setData([]);
    pso32Series?.setData([]);
    for (const series of smaSeries.values()) series.setData([]);
    markersApi?.setMarkers([]);
    clearPriceAlertLines();
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
    return visibleSma[window] && smaData(window).length > 0;
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
      crosshair: { mode: crosshairModeValue() },
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
    pso32Series = chart.addSeries(LineSeries, {
      color: "#cba6f7",
      lineWidth: 2,
      priceLineVisible: false,
      lastValueVisible: true,
      crosshairMarkerVisible: true,
      priceScaleId: "pso",
      title: "PSO 32",
      priceFormat: { type: "price", precision: 2, minMove: 0.01 },
    }, 2);
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
    volSeries.setData(showVolume ? vs : []);
    rsiSeries?.setData(showRsi ? rsiData(14) : []);
    psoSeries?.setData(showPso ? psoData() : []);
    pso32Series?.setData(showPso32 ? psoData(32, 5) : []);
    chart.timeScale().applyOptions({ timeVisible: isIntraday(interval), secondsVisible: false });
    for (const window of SMA_WINDOWS) {
      smaSeries.get(window)?.applyOptions({ title: smaLabel(window) });
      smaSeries.get(window)?.setData(visibleSma[window] ? smaData(window) : []);
    }
    applyMarkers();
    applyPriceAlertLines();
    if (cs.length > 0) chart.timeScale().fitContent();
  }

  $effect(() => {
    chart?.applyOptions({ crosshair: { mode: crosshairModeValue() } });
  });

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
    if (symbol && liveEvents.some((event) =>
      event.kind === "price_alert" && String(event.payload.symbol ?? "").toUpperCase() === symbol.toUpperCase()
    )) {
      void refreshPriceAlerts(symbol);
    }
  });

  onMount(() => {
    return () => {
      chart?.remove();
      chart = null;
      priceSeries = null;
      volSeries = null;
      rsiSeries = null;
      psoSeries = null;
      pso32Series = null;
      smaSeries.clear();
      markersApi = null;
      priceAlertLines = [];
    };
  });
</script>

<div class="chart-wrap">
  <div class="chart-topbar" data-testid="chart-topbar">
    <div class="symbol-cluster">
      <strong class="symbol-label">{symbol ?? "-"}</strong>
      <span class="chart-kind">Candles</span>
      {#if candles && candles.length > 0}
        {@const last = candles[candles.length - 1]}
        {@const move = lastChange()}
        <span class="quote-line" class:up={move?.up} class:down={!!move && !move.up}>
          <strong data-testid="chart-last-close">{last.close.toFixed(2)}</strong>
          {#if move}
            <span>{move.diff >= 0 ? "+" : ""}{move.diff.toFixed(2)}</span>
            <span>{move.pct >= 0 ? "+" : ""}{move.pct.toFixed(2)}%</span>
          {/if}
        </span>
      {/if}
    </div>

    <div class="interval-picker" aria-label="Chart interval">
      {#each INTERVALS as intv}
        <button
          class:active={interval === intv}
          data-testid={`interval-${intv}`}
          onclick={() => chooseInterval(intv)}
        >{intv}</button>
      {/each}
    </div>

    <div class="top-actions">
      <button
        type="button"
        class="toolbar-button"
        class:active={indicatorsOpen}
        data-testid="chart-indicators-button"
        aria-expanded={indicatorsOpen}
        aria-controls="chart-indicator-menu"
        onclick={() => (indicatorsOpen = !indicatorsOpen)}
      >
        Indicators <span>{activeIndicatorCount()}</span>
      </button>
      <button
        type="button"
        class="toolbar-button"
        class:active={alertMenuOpen}
        data-testid="chart-alert-button"
        onclick={openAlertMenu}
      >
        Alert <span>{priceAlerts.filter((rule) => rule.status === "active").length}</span>
      </button>
      <button type="button" class="toolbar-button" onclick={fitVisibleRange}>Fit</button>
    </div>

    <span class="meta sr-status" data-testid="chart-interval-status">
      <span class="muted">interval</span>
      <strong>{interval}</strong>
      <span class="muted">{range}</span>
    </span>
    <span class="meta" data-testid="chart-live-status">
      <span class="status-dot status-{liveStatus}"></span>
      <strong>{liveStatusLabel()}</strong>
    </span>
  </div>

  {#if indicatorsOpen}
    <div class="indicator-menu" id="chart-indicator-menu" data-testid="chart-indicator-menu">
      {#each SMA_WINDOWS as window}
        <label>
          <input type="checkbox" checked={visibleSma[window]} onchange={() => toggleSma(window)} />
          <span class="sma-key" style={`--sma-color: ${SMA_COLORS[window]}`}>{smaLabel(window)}</span>
        </label>
      {/each}
      <label>
        <input type="checkbox" checked={showVolume} onchange={() => { showVolume = !showVolume; render(); }} />
        Volume
      </label>
      <label>
        <input type="checkbox" checked={showRsi} onchange={() => { showRsi = !showRsi; render(); }} />
        RSI 14
      </label>
      <label>
        <input type="checkbox" checked={showPso} onchange={() => { showPso = !showPso; render(); }} />
        PSO 8/25
      </label>
      <label>
        <input type="checkbox" checked={showPso32} onchange={() => { showPso32 = !showPso32; render(); }} />
        PSO 32
      </label>
      <button type="button" onclick={resetIndicators}>Reset</button>
    </div>
  {/if}

  {#if alertMenuOpen}
    <div class="alert-menu" data-testid="chart-alert-menu">
      <div class="alert-form">
        <select bind:value={alertDirection} aria-label="Alert direction">
          <option value="above">above</option>
          <option value="below">below</option>
        </select>
        <select bind:value={alertIntent} aria-label="Alert intent">
          <option value="watch">watch</option>
          <option value="entry">entry</option>
          <option value="invalidation">invalidation</option>
          <option value="exit">exit</option>
        </select>
        <input inputmode="decimal" bind:value={alertTarget} placeholder="price" aria-label="Alert price" />
        <input bind:value={alertLabel} placeholder="label" aria-label="Alert label" />
        <button type="button" disabled={alertSaving || !symbol} onclick={saveManualAlert}>
          {alertSaving ? "Saving..." : "Create"}
        </button>
      </div>
      {#if alertError}
        <span class="err">{alertError}</span>
      {/if}
      {#if priceAlerts.filter((rule) => rule.status === "active").length > 0}
        <div class="alert-list">
          {#each priceAlerts.filter((rule) => rule.status === "active").slice(0, 6) as rule (rule.id)}
            <span class="alert-chip origin-{rule.origin}">
              {rule.origin === "ai" ? "AI" : "manual"} {rule.direction} {rule.target_price.toFixed(2)}
              <em>{rule.intent}</em>
              <button type="button" title="disable alert" onclick={() => disableRule(rule)}>×</button>
            </span>
          {/each}
        </div>
      {/if}
    </div>
  {/if}

  <div class="chart-stage">
    <aside class="tool-rail" data-testid="chart-left-tools" aria-label="Chart tools">
      <button
        type="button"
        class:active={crosshairMode === "magnet"}
        title="Magnet crosshair"
        aria-label="Magnet crosshair"
        onclick={() => setCrosshairMode("magnet")}
      ><span class="tool-icon icon-magnet"></span></button>
      <button
        type="button"
        class:active={crosshairMode === "free"}
        title="Free crosshair"
        aria-label="Free crosshair"
        onclick={() => setCrosshairMode("free")}
      ><span class="tool-icon icon-crosshair"></span></button>
      <button
        type="button"
        class:active={crosshairMode === "hidden"}
        title="Hide crosshair"
        aria-label="Hide crosshair"
        onclick={() => setCrosshairMode("hidden")}
      ><span class="tool-icon icon-pointer"></span></button>
      <button type="button" title="Fit content" aria-label="Fit content" onclick={fitVisibleRange}>
        <span class="tool-icon icon-fit"></span>
      </button>
    </aside>

    <section class="chart-shell">
      <div class="chart-overlay">
        <div class="legend-stack">
          {#if candles && candles.length > 0}
            <span>{candles.length} bars</span>
          {/if}
          {#if coverage?.start && coverage?.end}
            <span class="coverage" title={coverage.fetchError ?? ""}>
              {coverage.start.slice(0, 10)} - {coverage.end.slice(0, 10)}
              {#if coverage.fetchAttempts > 0} · {coverage.fetchAttempts} fetches{/if}
              {#if coverage.fetchError} · partial{/if}
            </span>
          {/if}
          {#if events.length > 0}
            <span>{events.length} events</span>
          {/if}
        </div>
        <div class="study-legend">
          {#if hasAnySma()}
            <span class="sma-legend" aria-label="SMA ribbon">
              {#each SMA_WINDOWS as window}
                {#if hasSma(window)}
                  <span class="sma-key" style={`--sma-color: ${SMA_COLORS[window]}`}>{smaLabel(window)}</span>
                {/if}
              {/each}
            </span>
          {/if}
          {#if candles && candles.length > 14 && showRsi}
            <span class="rsi-key" data-testid="rsi-legend">RSI 14</span>
          {/if}
          {#if candles && candles.length > 25 && showPso}
            <span class="pso-key" data-testid="pso-legend">PSO 8/25</span>
          {/if}
          {#if candles && candles.length > 32 && showPso32}
            <span class="pso32-key" data-testid="pso32-legend">PSO 32</span>
          {/if}
        </div>
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
    </section>
  </div>

  <div class="chart-bottombar">
    <div class="range-picker" aria-label="Chart range">
      {#each RANGES as r}
        <button
          class:active={range === r}
          data-testid={`range-${r}`}
          onclick={() => chooseRange(r)}
        >{r}</button>
      {/each}
    </div>
    <div class="bottom-status">
      {#if loading}<span class="muted">loading...</span>{/if}
      {#if error}<span class="err">{error}</span>{/if}
    </div>
  </div>
</div>

<style>
  .chart-wrap {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
    background: #0b0e14;
    color: #cdd6f4;
  }

  .chart-topbar,
  .chart-bottombar {
    display: flex;
    align-items: center;
    gap: .5rem;
    min-height: 38px;
    padding: .25rem .55rem;
    border-bottom: 1px solid #1f2733;
    flex-shrink: 0;
    font-size: .78rem;
    min-width: 0;
  }

  .chart-bottombar {
    border-top: 1px solid #1f2733;
    border-bottom: none;
    justify-content: space-between;
  }

  .symbol-cluster,
  .top-actions,
  .bottom-status,
  .meta {
    display: flex;
    align-items: center;
    gap: .35rem;
    min-width: 0;
  }

  .symbol-cluster {
    flex: 0 1 auto;
  }

  .symbol-label {
    font-size: .98rem;
    letter-spacing: 0;
    white-space: nowrap;
  }

  .chart-kind {
    border-left: 1px solid #263143;
    padding-left: .45rem;
    color: #9aa3b8;
    white-space: nowrap;
  }

  .quote-line {
    display: flex;
    align-items: baseline;
    gap: .28rem;
    color: #9aa3b8;
    white-space: nowrap;
  }

  .quote-line.up {
    color: #a6e3a1;
  }

  .quote-line.down {
    color: #f38ba8;
  }

  .interval-picker,
  .range-picker {
    display: flex;
    gap: .12rem;
    align-items: center;
    flex-wrap: nowrap;
    min-width: 0;
  }

  .interval-picker {
    flex: 1 1 auto;
    overflow-x: auto;
    scrollbar-width: thin;
  }

  .range-picker {
    flex: 0 1 auto;
    overflow-x: auto;
    scrollbar-width: thin;
  }

  .interval-picker button,
  .range-picker button,
  .toolbar-button,
  .indicator-menu button {
    border: 1px solid transparent;
    border-radius: 4px;
    background: transparent;
    color: #9aa3b8;
    cursor: pointer;
    font: inherit;
    min-height: 28px;
    padding: .18rem .45rem;
    white-space: nowrap;
  }

  .toolbar-button {
    border-color: #263143;
    background: #0f141d;
    display: inline-flex;
    align-items: center;
    gap: .35rem;
  }

  .toolbar-button span {
    color: #cdd6f4;
  }

  .interval-picker button:hover,
  .range-picker button:hover,
  .toolbar-button:hover,
  .indicator-menu button:hover {
    color: #cdd6f4;
    background: #151c28;
    border-color: #344159;
  }

  .interval-picker button.active,
  .range-picker button.active,
  .toolbar-button.active {
    background: #263143;
    color: #f5f7fb;
    border-color: #465873;
  }

  .indicator-menu {
    display: flex;
    align-items: center;
    gap: .65rem;
    flex-wrap: wrap;
    padding: .38rem .55rem;
    border-bottom: 1px solid #1f2733;
    background: #0f141d;
    font-size: .76rem;
  }

  .indicator-menu label {
    display: inline-flex;
    align-items: center;
    gap: .3rem;
    color: #bac2de;
    white-space: nowrap;
  }

  .indicator-menu input {
    accent-color: #89b4fa;
  }

  .alert-menu {
    display: grid;
    gap: .45rem;
    padding: .42rem .55rem;
    border-bottom: 1px solid #1f2733;
    background: #0f141d;
    font-size: .76rem;
  }

  .alert-form,
  .alert-list {
    display: flex;
    align-items: center;
    gap: .35rem;
    flex-wrap: wrap;
  }

  .alert-form select,
  .alert-form input {
    min-height: 28px;
    border: 1px solid #263143;
    border-radius: 4px;
    background: #0a0d14;
    color: #cdd6f4;
    font: inherit;
    padding: .18rem .4rem;
  }

  .alert-form input[aria-label="Alert price"] {
    width: 7rem;
  }

  .alert-form input[aria-label="Alert label"] {
    flex: 1 1 12rem;
    min-width: 12rem;
  }

  .alert-form button,
  .alert-chip button {
    min-height: 28px;
    border: 1px solid #2a3548;
    border-radius: 4px;
    background: #1b2230;
    color: #cdd6f4;
    font: inherit;
    cursor: pointer;
    padding: .18rem .55rem;
  }

  .alert-form button:disabled {
    opacity: .55;
    cursor: default;
  }

  .alert-chip {
    display: inline-flex;
    align-items: center;
    gap: .3rem;
    max-width: 100%;
    border: 1px solid #2a3548;
    border-radius: 4px;
    background: #11161f;
    color: #bac2de;
    padding: .12rem .28rem .12rem .42rem;
    white-space: nowrap;
  }

  .alert-chip.origin-ai {
    border-color: #344159;
    color: #89b4fa;
  }

  .alert-chip em {
    color: #6c7693;
    font-style: normal;
  }

  .alert-chip button {
    min-height: 20px;
    padding: 0 .3rem;
    line-height: 1;
  }

  .chart-stage {
    display: grid;
    grid-template-columns: 42px minmax(0, 1fr);
    flex: 1 1 auto;
    min-height: 0;
  }

  .tool-rail {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: .25rem;
    padding: .45rem .28rem;
    border-right: 1px solid #1f2733;
    background: #090d13;
  }

  .tool-rail button {
    width: 30px;
    height: 30px;
    display: grid;
    place-items: center;
    border: 1px solid transparent;
    border-radius: 4px;
    background: transparent;
    color: #9aa3b8;
    cursor: pointer;
  }

  .tool-rail button:hover,
  .tool-rail button.active {
    color: #f5f7fb;
    background: #151c28;
    border-color: #344159;
  }

  .tool-icon {
    width: 16px;
    height: 16px;
    position: relative;
    display: inline-block;
  }

  .icon-crosshair::before,
  .icon-crosshair::after,
  .icon-magnet::before,
  .icon-magnet::after,
  .icon-pointer::before,
  .icon-fit::before,
  .icon-fit::after {
    content: "";
    position: absolute;
    background: currentColor;
  }

  .icon-crosshair::before {
    width: 16px;
    height: 1px;
    top: 7px;
    left: 0;
  }

  .icon-crosshair::after {
    width: 1px;
    height: 16px;
    top: 0;
    left: 7px;
  }

  .icon-magnet {
    border: 2px solid currentColor;
    border-top: none;
    border-radius: 0 0 8px 8px;
    width: 14px;
    height: 13px;
  }

  .icon-magnet::before,
  .icon-magnet::after {
    width: 4px;
    height: 2px;
    top: 0;
  }

  .icon-magnet::before {
    left: -2px;
  }

  .icon-magnet::after {
    right: -2px;
  }

  .icon-pointer::before {
    width: 12px;
    height: 12px;
    clip-path: polygon(0 0, 12px 8px, 7px 10px, 5px 16px, 3px 15px, 5px 9px, 0 12px);
    left: 2px;
    top: 0;
  }

  .icon-fit::before {
    inset: 2px;
    border: 1px solid currentColor;
    background: transparent;
  }

  .icon-fit::after {
    width: 8px;
    height: 1px;
    top: 7px;
    left: 4px;
  }

  .chart-shell {
    position: relative;
    min-width: 0;
    min-height: 0;
    display: flex;
  }

  .chart-overlay {
    position: absolute;
    top: .42rem;
    left: .55rem;
    right: 5.4rem;
    z-index: 2;
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: .75rem;
    pointer-events: none;
  }

  .legend-stack,
  .study-legend {
    display: flex;
    align-items: center;
    gap: .45rem;
    flex-wrap: wrap;
    min-width: 0;
    color: #9aa3b8;
    font-size: .72rem;
  }

  .study-legend {
    justify-content: flex-end;
  }

  .sma-legend {
    display: flex;
    gap: .45rem;
    align-items: center;
    flex-wrap: wrap;
  }

  .sma-key {
    color: #bac2de;
    display: inline-flex;
    gap: .25rem;
    align-items: center;
    white-space: nowrap;
  }

  .sma-key::before {
    content: "";
    width: .9rem;
    height: 2px;
    background: var(--sma-color);
    display: inline-block;
    border-radius: 2px;
  }

  .rsi-key { color: #fab387; white-space: nowrap; }
  .pso-key { color: #94e2d5; white-space: nowrap; }
  .pso32-key { color: #cba6f7; white-space: nowrap; }

  .status-dot {
    width: .5rem;
    height: .5rem;
    border-radius: 999px;
    background: #6c7693;
    display: inline-block;
  }

  .status-live {
    background: #a6e3a1;
  }

  .status-delayed,
  .status-market_closed,
  .status-rate_limited {
    background: #f9e2af;
  }

  .status-entitlement_blocked,
  .status-disconnected {
    background: #f38ba8;
  }

  .muted {
    color: #6c7693;
  }

  .err {
    color: #f38ba8;
    font-size: .8rem;
  }

  .chart {
    flex: 1 1 auto;
    min-height: 0;
    position: relative;
  }

  .empty {
    position: absolute;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 1rem;
    text-align: center;
  }

  @media (max-width: 900px) {
    .chart-topbar {
      flex-wrap: wrap;
      align-items: stretch;
    }

    .symbol-cluster,
    .top-actions,
    .meta {
      min-height: 28px;
    }

    .interval-picker {
      order: 3;
      flex-basis: 100%;
    }

    .chart-stage {
      grid-template-columns: 36px minmax(0, 1fr);
    }

    .tool-rail button {
      width: 28px;
      height: 28px;
    }

    .chart-overlay {
      align-items: flex-start;
      flex-direction: column;
      right: 4.5rem;
    }
  }
</style>
