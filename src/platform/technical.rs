//! Derived technical state for chart/timing analysis.
//!
//! This is deliberately separate from a thesis. A symbol can have a bullish
//! thesis while the current chart state is extended or deteriorating.

use chrono::{DateTime, Datelike, Utc};
use serde::Serialize;

#[derive(Debug, Clone, Copy)]
pub struct TechnicalBar {
    pub ts: DateTime<Utc>,
    pub close: f64,
    pub high: f64,
    pub low: f64,
    pub volume: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SmaPoint {
    pub window: usize,
    pub value: Option<f64>,
    pub pct_vs: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct IntervalTechnical {
    pub interval: String,
    pub bars: usize,
    pub as_of: Option<DateTime<Utc>>,
    pub close: Option<f64>,
    pub rsi14: Option<f64>,
    pub rsi_zone: String,
    pub rsi_zone_bars: usize,
    pub rsi_zone_since: Option<DateTime<Utc>>,
    pub stochastic_k14: Option<f64>,
    pub stochastic_d3: Option<f64>,
    pub stochastic_zone: String,
    pub stochastic_zone_bars: usize,
    pub pso: Option<f64>,
    pub pso_delta: Option<f64>,
    pub pso_zone: String,
    pub pso_zone_bars: usize,
    pub pso32: Option<f64>,
    pub pso32_delta: Option<f64>,
    pub pso32_zone: String,
    pub pso32_zone_bars: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct DailyTechnical {
    pub as_of: DateTime<Utc>,
    pub close: f64,
    pub sma: Vec<SmaPoint>,
    pub pct_vs_252d_high: Option<f64>,
    pub pct_vs_252d_low: Option<f64>,
    pub macd: Option<MacdTechnical>,
    pub dmi: Option<DmiTechnical>,
    pub atr: Option<AtrTechnical>,
    pub bollinger: Option<BollingerTechnical>,
    pub volume: Option<VolumeTechnical>,
    pub relative_strength: Vec<RelativeStrengthTechnical>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MacdTechnical {
    pub line: f64,
    pub signal: f64,
    pub histogram: f64,
    pub histogram_delta: Option<f64>,
    pub state: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DmiTechnical {
    pub adx14: f64,
    pub plus_di14: f64,
    pub minus_di14: f64,
    pub state: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AtrTechnical {
    pub atr14: f64,
    pub natr14_pct: f64,
    pub state: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct BollingerTechnical {
    pub middle20: f64,
    pub upper20: f64,
    pub lower20: f64,
    pub bandwidth_pct: f64,
    pub pct_b: f64,
    pub state: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct VolumeTechnical {
    pub latest: f64,
    pub avg20: Option<f64>,
    pub avg50: Option<f64>,
    pub ratio_vs_20: Option<f64>,
    pub state: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RelativeStrengthTechnical {
    pub benchmark: String,
    pub rel_20d_pct: Option<f64>,
    pub rel_60d_pct: Option<f64>,
    pub rel_120d_pct: Option<f64>,
    pub state: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CrossTechnical {
    pub trend_state: String,
    pub momentum_state: String,
    pub volatility_state: String,
    pub volume_state: String,
    pub relative_strength_state: String,
    pub reversal_signal: String,
    pub buy_timing: String,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CrossEvent {
    pub window: usize,
    pub direction: String,
    pub at: DateTime<Utc>,
    pub close: f64,
    pub sma: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct AnalogEvent {
    pub kind: String,
    pub at: DateTime<Utc>,
    pub close: f64,
    pub rsi14: f64,
    pub forward_return_20d_pct: Option<f64>,
    pub max_drawdown_20d_pct: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TechnicalState {
    pub symbol: String,
    pub as_of: Option<DateTime<Utc>>,
    pub state: String,
    pub setup: TechnicalSetup,
    pub summary: String,
    pub daily: Option<DailyTechnical>,
    pub cross: Option<CrossTechnical>,
    pub intervals: Vec<IntervalTechnical>,
    pub last_crosses: Vec<CrossEvent>,
    pub analog_events: Vec<AnalogEvent>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TechnicalSetup {
    pub kind: String,
    pub entry_stance: String,
    pub summary: String,
}

#[must_use]
pub fn build_technical_state(
    symbol: &str,
    daily: &[TechnicalBar],
    intraday: &[(&str, Vec<TechnicalBar>)],
) -> TechnicalState {
    build_technical_state_with_benchmarks(symbol, daily, intraday, &[])
}

#[must_use]
pub fn build_technical_state_with_benchmarks(
    symbol: &str,
    daily: &[TechnicalBar],
    intraday: &[(&str, Vec<TechnicalBar>)],
    benchmarks: &[(&str, Vec<TechnicalBar>)],
) -> TechnicalState {
    let weekly = weekly_bars(daily);
    let mut intervals = Vec::new();
    intervals.push(interval_state("1d", daily));
    intervals.push(interval_state("1w", &weekly));
    for (label, bars) in intraday {
        intervals.push(interval_state(label, bars));
    }
    intervals.sort_by_key(|i| interval_rank(&i.interval));

    let daily_technical = daily_state(daily, benchmarks);
    let cross = cross_technical(daily, &daily_technical, &intervals);
    let state = classify_state(daily, &daily_technical, &intervals, cross.as_ref());
    let setup = classify_setup(daily, &daily_technical, &intervals, cross.as_ref(), &state);
    let summary = state_summary(&state, &daily_technical, &intervals, cross.as_ref());
    TechnicalState {
        symbol: symbol.to_ascii_uppercase(),
        as_of: daily.last().map(|b| b.ts),
        state,
        setup,
        summary,
        daily: daily_technical,
        cross,
        intervals,
        last_crosses: last_crosses(daily, &[50, 200], 6),
        analog_events: analog_events(daily, 5),
    }
}

fn interval_rank(label: &str) -> usize {
    match label {
        "30m" => 0,
        "2h" => 1,
        "4h" => 2,
        "1d" => 3,
        "1w" => 4,
        _ => 99,
    }
}

fn daily_state(
    bars: &[TechnicalBar],
    benchmarks: &[(&str, Vec<TechnicalBar>)],
) -> Option<DailyTechnical> {
    let latest = *bars.last()?;
    let closes = bars.iter().map(|b| b.close).collect::<Vec<_>>();
    let sma = [20, 50, 100, 200]
        .into_iter()
        .map(|window| {
            let value = sma_at(&closes, closes.len().saturating_sub(1), window);
            SmaPoint {
                window,
                value,
                pct_vs: value.and_then(|v| pct_vs(latest.close, v)),
            }
        })
        .collect::<Vec<_>>();
    let window = bars.iter().rev().take(252).copied().collect::<Vec<_>>();
    let high = window.iter().map(|b| b.high).fold(f64::MIN, f64::max);
    let low = window.iter().map(|b| b.low).fold(f64::MAX, f64::min);
    Some(DailyTechnical {
        as_of: latest.ts,
        close: latest.close,
        sma,
        pct_vs_252d_high: (high > 0.0).then(|| (latest.close - high) / high * 100.0),
        pct_vs_252d_low: (low > 0.0).then(|| (latest.close - low) / low * 100.0),
        macd: macd_state(&closes),
        dmi: dmi_state(bars),
        atr: atr_state(bars),
        bollinger: bollinger_state(&closes, latest.close),
        volume: volume_state(bars),
        relative_strength: benchmarks
            .iter()
            .filter_map(|(benchmark, benchmark_bars)| {
                relative_strength_state(bars, benchmark, benchmark_bars)
            })
            .collect(),
    })
}

fn interval_state(interval: &str, bars: &[TechnicalBar]) -> IntervalTechnical {
    let closes = bars.iter().map(|b| b.close).collect::<Vec<_>>();
    let rsi = rsi14_series(&closes);
    let latest_rsi = rsi.last().copied().flatten();
    let rsi_zone_name = latest_rsi.map_or_else(|| "unknown".to_string(), rsi_zone);
    let (rsi_zone_bars, rsi_zone_since) = current_zone_span(bars, &rsi, &rsi_zone_name, rsi_zone);
    let stochastic_k = stochastic_k_series(bars, 14);
    let stochastic_d = sma_optional(&stochastic_k, 3);
    let latest_stochastic_k = stochastic_k.last().copied().flatten();
    let latest_stochastic_d = stochastic_d.last().copied().flatten();
    let stochastic_zone_name =
        latest_stochastic_k.map_or_else(|| "unknown".to_string(), stochastic_zone);
    let (stochastic_zone_bars, _) =
        current_zone_span(bars, &stochastic_k, &stochastic_zone_name, stochastic_zone);
    let pso = pso_series(bars, 8, 5);
    let latest_pso = pso.last().copied().flatten();
    let pso_zone_name = latest_pso.map_or_else(|| "unknown".to_string(), pso_zone);
    let (pso_zone_bars, _) = current_zone_span(bars, &pso, &pso_zone_name, pso_zone);
    let pso32 = pso_series(bars, 32, 5);
    let latest_pso32 = pso32.last().copied().flatten();
    let pso32_zone_name = latest_pso32.map_or_else(|| "unknown".to_string(), pso_zone);
    let (pso32_zone_bars, _) = current_zone_span(bars, &pso32, &pso32_zone_name, pso_zone);
    IntervalTechnical {
        interval: interval.to_string(),
        bars: bars.len(),
        as_of: bars.last().map(|b| b.ts),
        close: bars.last().map(|b| b.close),
        rsi14: latest_rsi.map(round2),
        rsi_zone: rsi_zone_name,
        rsi_zone_bars,
        rsi_zone_since,
        stochastic_k14: latest_stochastic_k.map(round2),
        stochastic_d3: latest_stochastic_d.map(round2),
        stochastic_zone: stochastic_zone_name,
        stochastic_zone_bars,
        pso: latest_pso.map(round2),
        pso_delta: latest_delta(&pso),
        pso_zone: pso_zone_name,
        pso_zone_bars,
        pso32: latest_pso32.map(round2),
        pso32_delta: latest_delta(&pso32),
        pso32_zone: pso32_zone_name,
        pso32_zone_bars,
    }
}

fn classify_state(
    daily: &[TechnicalBar],
    daily_technical: &Option<DailyTechnical>,
    intervals: &[IntervalTechnical],
    cross: Option<&CrossTechnical>,
) -> String {
    let Some(d) = daily_technical else {
        return "unknown".to_string();
    };
    let rsi_1d = intervals
        .iter()
        .find(|i| i.interval == "1d")
        .and_then(|i| i.rsi14);
    let daily_interval = intervals.iter().find(|i| i.interval == "1d");
    let pct_vs_200 = d
        .sma
        .iter()
        .find(|s| s.window == 200)
        .and_then(|s| s.pct_vs);
    let pct_vs_50 = d.sma.iter().find(|s| s.window == 50).and_then(|s| s.pct_vs);
    if cross.is_some_and(|c| c.buy_timing == "pullback_reversal") {
        return "reversal_confirming".to_string();
    }
    if cross.is_some_and(|c| c.buy_timing == "pullback_watch") {
        return "pullback_watch".to_string();
    }
    if pct_vs_200.is_some_and(|v| v > 20.0)
        || d.pct_vs_252d_high.is_some_and(|v| v >= -2.0)
        || daily_interval.is_some_and(oscillator_extended)
    {
        return "extended".to_string();
    }
    let cross_pullback = cross
        .is_some_and(|c| c.buy_timing == "pullback_watch" || c.buy_timing == "pullback_reversal");
    let broken_trend = cross.is_some_and(|c| c.trend_state == "breakdown");
    let weak_rsi_without_pullback = rsi_1d.is_some_and(|v| v <= 40.0) && !cross_pullback;
    if broken_trend || pct_vs_200.is_some_and(|v| v < -5.0) || weak_rsi_without_pullback {
        return "deteriorating".to_string();
    }
    if pct_vs_50.is_some_and(|v| v.abs() <= 5.0) && daily.len() >= 50 {
        return "base_building".to_string();
    }
    if pct_vs_200.is_some_and(|v| v >= 0.0) && rsi_1d.is_some_and(|v| v < 70.0) {
        return "constructive".to_string();
    }
    "unknown".to_string()
}

fn classify_setup(
    daily: &[TechnicalBar],
    daily_technical: &Option<DailyTechnical>,
    intervals: &[IntervalTechnical],
    cross: Option<&CrossTechnical>,
    state: &str,
) -> TechnicalSetup {
    let Some(d) = daily_technical else {
        return TechnicalSetup {
            kind: "unknown".to_string(),
            entry_stance: "wait_data".to_string(),
            summary: "Need daily bars before classifying entry setup.".to_string(),
        };
    };
    let closes = daily.iter().map(|b| b.close).collect::<Vec<_>>();
    let sma200 = (0..closes.len())
        .map(|idx| sma_at(&closes, idx, 200))
        .collect::<Vec<_>>();
    let pct_vs_200 = d
        .sma
        .iter()
        .find(|s| s.window == 200)
        .and_then(|s| s.pct_vs);
    let rsi_1d = intervals
        .iter()
        .find(|i| i.interval == "1d")
        .and_then(|i| i.rsi14);
    let daily_interval = intervals.iter().find(|i| i.interval == "1d");
    let recent_up_200 = recent_cross_index(daily, &sma200, "up", 45);
    let recent_down_200 = recent_cross_index(daily, &sma200, "down", 180);
    let latest_idx = daily.len().saturating_sub(1);
    let had_prior_above = recent_down_200.is_some_and(|down_idx| {
        daily
            .iter()
            .enumerate()
            .take(down_idx)
            .any(|(idx, bar)| sma200[idx].is_some_and(|sma| bar.close >= sma))
    });
    let reclaim_after_break = recent_up_200
        .zip(recent_down_200)
        .is_some_and(|(up_idx, down_idx)| up_idx > down_idx && had_prior_above);
    let near_reclaim_after_break = recent_down_200.is_some_and(|down_idx| {
        had_prior_above
            && latest_idx > down_idx
            && pct_vs_200.is_some_and(|pct| (-3.0..=2.0).contains(&pct))
    });

    if reclaim_after_break && pct_vs_200.is_some_and(|pct| pct <= 8.0) {
        let entry_stance =
            if rsi_1d.is_some_and(|rsi| rsi >= 70.0) || daily_interval.is_some_and(pso_extreme) {
                "wait_retest"
            } else {
                "actionable"
            };
        return TechnicalSetup {
            kind: "200d_reclaim".to_string(),
            entry_stance: entry_stance.to_string(),
            summary: "Price recently reclaimed the 200-day SMA after a prior break below it."
                .to_string(),
        };
    }

    if near_reclaim_after_break {
        return TechnicalSetup {
            kind: "200d_reclaim_watch".to_string(),
            entry_stance: "wait_reclaim".to_string(),
            summary: "Price previously broke the 200-day SMA and is now close enough to watch for a reclaim.".to_string(),
        };
    }

    if state == "reversal_confirming" {
        return TechnicalSetup {
            kind: "pullback_reversal".to_string(),
            entry_stance: "starter_ok".to_string(),
            summary: "Trend support is intact and oversold momentum is turning up; consider only a starter or defined-risk entry.".to_string(),
        };
    }

    if state == "pullback_watch" {
        let detail = cross.map(|c| c.summary.clone()).unwrap_or_else(|| {
            "Trend support is intact but timing still needs confirmation.".to_string()
        });
        return TechnicalSetup {
            kind: "pullback_watch".to_string(),
            entry_stance: "wait_reversal".to_string(),
            summary: format!(
                "{detail} Wait for PSO/MACD turn-up, a failed breakdown, or 20D/50D reclaim before entry."
            ),
        };
    }

    if state == "extended" {
        return TechnicalSetup {
            kind: "extended_run".to_string(),
            entry_stance: "avoid_chase".to_string(),
            summary: "Chart is extended versus trend or RSI; bullish theses need a pullback, base, or retest before entry.".to_string(),
        };
    }

    if state == "base_building" {
        return TechnicalSetup {
            kind: "base_building".to_string(),
            entry_stance: "wait_breakout".to_string(),
            summary: "Price is compressing near moving averages; wait for a clean breakout or failed breakdown.".to_string(),
        };
    }

    if state == "deteriorating" {
        return TechnicalSetup {
            kind: "breakdown".to_string(),
            entry_stance: "avoid".to_string(),
            summary: "Price is below key trend or momentum is weak; do not treat bullish theses as actionable yet.".to_string(),
        };
    }

    TechnicalSetup {
        kind: "constructive_trend".to_string(),
        entry_stance: "constructive".to_string(),
        summary: "Trend is constructive but there is no fresh 200-day reclaim setup.".to_string(),
    }
}

fn recent_cross_index(
    bars: &[TechnicalBar],
    sma: &[Option<f64>],
    direction: &str,
    lookback_bars: usize,
) -> Option<usize> {
    if bars.len() < 2 {
        return None;
    }
    let start = bars.len().saturating_sub(lookback_bars + 1).max(1);
    (start..bars.len()).rev().find(|&idx| {
        let Some(prev_sma) = sma[idx - 1] else {
            return false;
        };
        let Some(curr_sma) = sma[idx] else {
            return false;
        };
        let prev = bars[idx - 1].close;
        let curr = bars[idx].close;
        match direction {
            "up" => prev <= prev_sma && curr > curr_sma,
            "down" => prev >= prev_sma && curr < curr_sma,
            _ => false,
        }
    })
}

fn state_summary(
    state: &str,
    daily: &Option<DailyTechnical>,
    intervals: &[IntervalTechnical],
    cross: Option<&CrossTechnical>,
) -> String {
    let Some(d) = daily else {
        return "Not enough price history to compute technical state.".to_string();
    };
    let pct_200 = d
        .sma
        .iter()
        .find(|s| s.window == 200)
        .and_then(|s| s.pct_vs);
    let rsi_1d = intervals
        .iter()
        .find(|i| i.interval == "1d")
        .and_then(|i| i.rsi14);
    let stochastic_1d = intervals
        .iter()
        .find(|i| i.interval == "1d")
        .and_then(|i| i.stochastic_k14);
    let pso_1d = intervals
        .iter()
        .find(|i| i.interval == "1d")
        .and_then(|i| i.pso);
    let pso32_1d = intervals
        .iter()
        .find(|i| i.interval == "1d")
        .and_then(|i| i.pso32);
    let mut parts = vec![format!("technical state is {state}")];
    if let Some(v) = pct_200 {
        parts.push(format!("{v:+.1}% vs 200-day SMA"));
    }
    if let Some(v) = rsi_1d {
        parts.push(format!("RSI 14 daily {v:.1}"));
    }
    if let Some(v) = stochastic_1d {
        parts.push(format!("Stoch 14 %K {v:.1}"));
    }
    if let Some(v) = pso_1d {
        parts.push(format!("PSO 8/25 {v:.2}"));
    }
    if let Some(v) = pso32_1d {
        parts.push(format!("PSO 32 {v:.2}"));
    }
    if let Some(v) = d.pct_vs_252d_high {
        parts.push(format!("{v:+.1}% vs 252-day high"));
    }
    if let Some(cross) = cross {
        parts.push(cross.summary.clone());
    }
    parts.join("; ")
}

fn current_zone_span(
    bars: &[TechnicalBar],
    values: &[Option<f64>],
    current_zone: &str,
    zone_fn: fn(f64) -> String,
) -> (usize, Option<DateTime<Utc>>) {
    if current_zone == "unknown" {
        return (0, None);
    }
    let mut count = 0;
    let mut since = None;
    for idx in (0..values.len()).rev() {
        let Some(value) = values[idx] else {
            break;
        };
        if zone_fn(value) != current_zone {
            break;
        }
        count += 1;
        since = bars.get(idx).map(|b| b.ts);
    }
    (count, since)
}

fn oscillator_extended(interval: &IntervalTechnical) -> bool {
    interval.rsi14.is_some_and(|v| v >= 70.0)
        || pso_extreme(interval)
        || (interval.stochastic_k14.is_some_and(|v| v >= 85.0)
            && interval.stochastic_d3.is_some_and(|v| v >= 80.0)
            && interval.pso.is_some_and(|v| v >= 0.2))
}

fn pso_extreme(interval: &IntervalTechnical) -> bool {
    interval.pso.is_some_and(|v| v >= 0.9) || interval.pso32.is_some_and(|v| v >= 0.9)
}

fn oscillator_weak(interval: &IntervalTechnical) -> bool {
    interval.pso.is_some_and(|v| v <= -0.2)
        || interval.pso32.is_some_and(|v| v <= -0.2)
        || (interval.stochastic_k14.is_some_and(|v| v <= 20.0)
            && interval.stochastic_d3.is_some_and(|v| v <= 25.0))
}

fn cross_technical(
    daily: &[TechnicalBar],
    daily_technical: &Option<DailyTechnical>,
    intervals: &[IntervalTechnical],
) -> Option<CrossTechnical> {
    let d = daily_technical.as_ref()?;
    let daily_interval = intervals.iter().find(|i| i.interval == "1d");
    let pct_vs_20 = sma_pct(d, 20);
    let pct_vs_50 = sma_pct(d, 50);
    let pct_vs_200 = sma_pct(d, 200);
    let above_or_near_200 = pct_vs_200.is_some_and(|v| v >= -2.0);
    let below_short_mas = pct_vs_20.is_some_and(|v| v < 0.0) && pct_vs_50.is_some_and(|v| v < 0.0);
    let near_high = d.pct_vs_252d_high.is_some_and(|v| v >= -2.0);
    let extended_from_trend = pct_vs_200.is_some_and(|v| v > 20.0) || near_high;
    let fast_oversold = daily_interval.is_some_and(|i| {
        i.rsi14.is_some_and(|v| v <= 35.0)
            || i.stochastic_k14.is_some_and(|v| v <= 20.0)
            || i.pso.is_some_and(|v| v <= -0.9)
    });
    let slow_oversold = daily_interval.is_some_and(|i| i.pso32.is_some_and(|v| v <= -0.9));
    let oscillator_weak_now = daily_interval.is_some_and(oscillator_weak);
    let oscillator_extended_now = daily_interval.is_some_and(oscillator_extended);
    let pso_turning = daily_interval.is_some_and(|i| {
        i.pso_delta.is_some_and(|v| v > 0.03)
            || (i.stochastic_k14.is_some_and(|k| k <= 45.0)
                && i.stochastic_d3
                    .zip(i.stochastic_k14)
                    .is_some_and(|(d, k)| k > d))
    });
    let macd_turning = d
        .macd
        .as_ref()
        .and_then(|m| m.histogram_delta)
        .is_some_and(|v| v > 0.0);
    let macd_bearish = d
        .macd
        .as_ref()
        .is_some_and(|m| m.histogram < 0.0 && m.histogram_delta.unwrap_or(0.0) <= 0.0);
    let dmi_bearish = d
        .dmi
        .as_ref()
        .is_some_and(|m| m.adx14 >= 20.0 && m.minus_di14 > m.plus_di14);
    let volume_distribution = d.volume.as_ref().is_some_and(|v| v.state == "distribution");
    let relative_strength_state = aggregate_relative_strength(&d.relative_strength);
    let rs_underperforming = relative_strength_state == "underperforming";
    let trend_state = if pct_vs_200.is_some_and(|v| v < -2.0)
        && (dmi_bearish || rs_underperforming || volume_distribution)
    {
        "breakdown"
    } else if extended_from_trend || oscillator_extended_now {
        "extended_chase"
    } else if above_or_near_200 && below_short_mas {
        "pullback_in_uptrend"
    } else if above_or_near_200 {
        "uptrend"
    } else if pct_vs_200.is_some_and(|v| (-2.0..=2.0).contains(&v)) {
        "testing_200d"
    } else {
        "trend_unclear"
    };
    let reversal_signal = if pso_turning && macd_turning {
        "confirmed"
    } else if pso_turning || macd_turning {
        "early"
    } else {
        "none"
    };
    let momentum_state = if oscillator_extended_now {
        "extended"
    } else if (fast_oversold || slow_oversold) && reversal_signal != "none" {
        "turning_up_from_oversold"
    } else if fast_oversold || slow_oversold {
        "oversold"
    } else if oscillator_weak_now || macd_bearish {
        "weak"
    } else if d.macd.as_ref().is_some_and(|m| m.histogram >= 0.0) {
        "positive"
    } else {
        "neutral"
    };
    let volatility_state = d
        .bollinger
        .as_ref()
        .map(|b| b.state.clone())
        .or_else(|| d.atr.as_ref().map(|a| a.state.clone()))
        .unwrap_or_else(|| "unknown".to_string());
    let volume_state = d
        .volume
        .as_ref()
        .map(|v| v.state.clone())
        .unwrap_or_else(|| "unknown".to_string());
    let buy_timing = if trend_state == "breakdown" {
        "avoid_breakdown"
    } else if trend_state == "extended_chase" {
        "avoid_chase"
    } else if above_or_near_200
        && (below_short_mas || fast_oversold || slow_oversold)
        && reversal_signal == "confirmed"
    {
        "pullback_reversal"
    } else if above_or_near_200 && (below_short_mas || fast_oversold || slow_oversold) {
        "pullback_watch"
    } else if above_or_near_200 && momentum_state == "positive" {
        "constructive"
    } else {
        "wait"
    };
    let summary = cross_summary(
        trend_state,
        momentum_state,
        reversal_signal,
        &relative_strength_state,
        &volume_state,
        daily.len(),
    );
    Some(CrossTechnical {
        trend_state: trend_state.to_string(),
        momentum_state: momentum_state.to_string(),
        volatility_state,
        volume_state,
        relative_strength_state,
        reversal_signal: reversal_signal.to_string(),
        buy_timing: buy_timing.to_string(),
        summary,
    })
}

fn cross_summary(
    trend: &str,
    momentum: &str,
    reversal: &str,
    relative_strength: &str,
    volume: &str,
    bars: usize,
) -> String {
    if bars < 200 {
        return "cross read has limited history".to_string();
    }
    format!(
        "cross read: trend {trend}, momentum {momentum}, reversal {reversal}, RS {relative_strength}, volume {volume}"
    )
}

fn sma_pct(daily: &DailyTechnical, window: usize) -> Option<f64> {
    daily
        .sma
        .iter()
        .find(|s| s.window == window)
        .and_then(|s| s.pct_vs)
}

fn macd_state(closes: &[f64]) -> Option<MacdTechnical> {
    let values = closes.iter().map(|v| Some(*v)).collect::<Vec<_>>();
    let ema12 = ema_optional(&values, 12);
    let ema26 = ema_optional(&values, 26);
    let macd = ema12
        .iter()
        .zip(ema26.iter())
        .map(|(fast, slow)| fast.zip(*slow).map(|(fast, slow)| fast - slow))
        .collect::<Vec<_>>();
    let signal = ema_optional(&macd, 9);
    let histogram = macd
        .iter()
        .zip(signal.iter())
        .map(|(line, signal)| line.zip(*signal).map(|(line, signal)| line - signal))
        .collect::<Vec<_>>();
    let latest_idx = histogram.iter().rposition(Option::is_some)?;
    let line = macd[latest_idx]?;
    let signal_value = signal[latest_idx]?;
    let hist = histogram[latest_idx]?;
    let delta = latest_delta(&histogram);
    let state = if hist >= 0.0 && delta.unwrap_or(0.0) >= 0.0 {
        "bullish"
    } else if hist < 0.0 && delta.unwrap_or(0.0) > 0.0 {
        "improving"
    } else if hist < 0.0 {
        "bearish"
    } else {
        "fading"
    };
    Some(MacdTechnical {
        line: round2(line),
        signal: round2(signal_value),
        histogram: round2(hist),
        histogram_delta: delta,
        state: state.to_string(),
    })
}

fn atr_state(bars: &[TechnicalBar]) -> Option<AtrTechnical> {
    let true_range = true_range_series(bars);
    let atr = wilder_optional(&true_range, 14);
    let latest_idx = atr.iter().rposition(Option::is_some)?;
    let atr14 = atr[latest_idx]?;
    let close = bars.get(latest_idx)?.close;
    if close <= 0.0 {
        return None;
    }
    let natr = atr14 / close * 100.0;
    let state = if natr >= 5.0 {
        "volatile"
    } else if natr <= 2.0 {
        "quiet"
    } else {
        "normal"
    };
    Some(AtrTechnical {
        atr14: round2(atr14),
        natr14_pct: round2(natr),
        state: state.to_string(),
    })
}

fn dmi_state(bars: &[TechnicalBar]) -> Option<DmiTechnical> {
    if bars.len() <= 15 {
        return None;
    }
    let true_range = true_range_series(bars);
    let mut plus_dm = vec![None; bars.len()];
    let mut minus_dm = vec![None; bars.len()];
    for idx in 1..bars.len() {
        let up_move = bars[idx].high - bars[idx - 1].high;
        let down_move = bars[idx - 1].low - bars[idx].low;
        plus_dm[idx] = Some(if up_move > down_move && up_move > 0.0 {
            up_move
        } else {
            0.0
        });
        minus_dm[idx] = Some(if down_move > up_move && down_move > 0.0 {
            down_move
        } else {
            0.0
        });
    }
    let tr14 = wilder_optional(&true_range, 14);
    let plus14 = wilder_optional(&plus_dm, 14);
    let minus14 = wilder_optional(&minus_dm, 14);
    let dx = tr14
        .iter()
        .zip(plus14.iter())
        .zip(minus14.iter())
        .map(|((tr, plus), minus)| {
            let tr = (*tr)?;
            if tr <= 0.0 {
                return None;
            }
            let plus_di = (*plus)? / tr * 100.0;
            let minus_di = (*minus)? / tr * 100.0;
            let denom = plus_di + minus_di;
            (denom > 0.0).then(|| (plus_di - minus_di).abs() / denom * 100.0)
        })
        .collect::<Vec<_>>();
    let adx = wilder_optional(&dx, 14);
    let latest_idx = adx.iter().rposition(Option::is_some)?;
    let tr = tr14[latest_idx]?;
    if tr <= 0.0 {
        return None;
    }
    let plus_di = plus14[latest_idx]? / tr * 100.0;
    let minus_di = minus14[latest_idx]? / tr * 100.0;
    let adx14 = adx[latest_idx]?;
    let state = if adx14 < 18.0 {
        "range"
    } else if plus_di > minus_di {
        "bull_trend"
    } else {
        "bear_trend"
    };
    Some(DmiTechnical {
        adx14: round2(adx14),
        plus_di14: round2(plus_di),
        minus_di14: round2(minus_di),
        state: state.to_string(),
    })
}

fn bollinger_state(closes: &[f64], close: f64) -> Option<BollingerTechnical> {
    const WINDOW: usize = 20;
    if closes.len() < WINDOW {
        return None;
    }
    let slice = &closes[closes.len() - WINDOW..];
    let middle = slice.iter().sum::<f64>() / WINDOW as f64;
    if middle <= 0.0 {
        return None;
    }
    let variance = slice
        .iter()
        .map(|v| {
            let diff = v - middle;
            diff * diff
        })
        .sum::<f64>()
        / WINDOW as f64;
    let stddev = variance.sqrt();
    let upper = middle + 2.0 * stddev;
    let lower = middle - 2.0 * stddev;
    let width = upper - lower;
    let pct_b = if width > 0.0 {
        (close - lower) / width
    } else {
        0.5
    };
    let bandwidth = width / middle * 100.0;
    let state = if bandwidth <= 10.0 {
        "compressed"
    } else if bandwidth >= 25.0 {
        "expanded"
    } else if pct_b >= 0.95 {
        "upper_band"
    } else if pct_b <= 0.05 {
        "lower_band"
    } else {
        "normal"
    };
    Some(BollingerTechnical {
        middle20: round2(middle),
        upper20: round2(upper),
        lower20: round2(lower),
        bandwidth_pct: round2(bandwidth),
        pct_b: round2(pct_b),
        state: state.to_string(),
    })
}

fn volume_state(bars: &[TechnicalBar]) -> Option<VolumeTechnical> {
    let latest = bars.last()?;
    if latest.volume <= 0.0 {
        return None;
    }
    let volumes = bars.iter().map(|b| b.volume).collect::<Vec<_>>();
    let avg20 = sma_at(&volumes, volumes.len().saturating_sub(1), 20);
    let avg50 = sma_at(&volumes, volumes.len().saturating_sub(1), 50);
    let ratio = avg20.and_then(|avg| (avg > 0.0).then(|| latest.volume / avg));
    let down_day = bars
        .get(bars.len().saturating_sub(2))
        .is_some_and(|prev| latest.close < prev.close);
    let up_day = bars
        .get(bars.len().saturating_sub(2))
        .is_some_and(|prev| latest.close > prev.close);
    let state = if ratio.is_some_and(|v| v >= 1.5) && down_day {
        "distribution"
    } else if ratio.is_some_and(|v| v >= 1.5) && up_day {
        "accumulation"
    } else if ratio.is_some_and(|v| v <= 0.6) {
        "quiet"
    } else {
        "normal"
    };
    Some(VolumeTechnical {
        latest: round2(latest.volume),
        avg20: avg20.map(round2),
        avg50: avg50.map(round2),
        ratio_vs_20: ratio.map(round2),
        state: state.to_string(),
    })
}

fn relative_strength_state(
    bars: &[TechnicalBar],
    benchmark: &str,
    benchmark_bars: &[TechnicalBar],
) -> Option<RelativeStrengthTechnical> {
    if benchmark_bars.is_empty() || bars.len() < 21 {
        return None;
    }
    let rel_20 = relative_return(bars, benchmark_bars, 20);
    let rel_60 = relative_return(bars, benchmark_bars, 60);
    let rel_120 = relative_return(bars, benchmark_bars, 120);
    let positives = [rel_20, rel_60, rel_120]
        .into_iter()
        .flatten()
        .filter(|v| *v > 0.0)
        .count();
    let negatives = [rel_20, rel_60, rel_120]
        .into_iter()
        .flatten()
        .filter(|v| *v < 0.0)
        .count();
    let state = if positives >= 2 {
        "outperforming"
    } else if negatives >= 2 {
        "underperforming"
    } else {
        "neutral"
    };
    Some(RelativeStrengthTechnical {
        benchmark: benchmark.to_string(),
        rel_20d_pct: rel_20.map(round2),
        rel_60d_pct: rel_60.map(round2),
        rel_120d_pct: rel_120.map(round2),
        state: state.to_string(),
    })
}

fn aggregate_relative_strength(rows: &[RelativeStrengthTechnical]) -> String {
    if rows.is_empty() {
        return "unknown".to_string();
    }
    let outperforming = rows.iter().filter(|r| r.state == "outperforming").count();
    let underperforming = rows.iter().filter(|r| r.state == "underperforming").count();
    if outperforming > underperforming {
        "outperforming".to_string()
    } else if underperforming > outperforming {
        "underperforming".to_string()
    } else {
        "neutral".to_string()
    }
}

fn last_crosses(bars: &[TechnicalBar], windows: &[usize], limit: usize) -> Vec<CrossEvent> {
    let closes = bars.iter().map(|b| b.close).collect::<Vec<_>>();
    let mut events = Vec::new();
    for &window in windows {
        for i in 1..bars.len() {
            let Some(prev_sma) = sma_at(&closes, i - 1, window) else {
                continue;
            };
            let Some(curr_sma) = sma_at(&closes, i, window) else {
                continue;
            };
            let prev = bars[i - 1].close;
            let curr = bars[i].close;
            let direction = if prev <= prev_sma && curr > curr_sma {
                Some("up")
            } else if prev >= prev_sma && curr < curr_sma {
                Some("down")
            } else {
                None
            };
            if let Some(direction) = direction {
                events.push(CrossEvent {
                    window,
                    direction: direction.to_string(),
                    at: bars[i].ts,
                    close: round2(curr),
                    sma: round2(curr_sma),
                });
            }
        }
    }
    events.sort_by_key(|e| std::cmp::Reverse(e.at));
    events.truncate(limit);
    events
}

fn analog_events(bars: &[TechnicalBar], limit: usize) -> Vec<AnalogEvent> {
    let closes = bars.iter().map(|b| b.close).collect::<Vec<_>>();
    let rsi = rsi14_series(&closes);
    let Some(current_zone) = rsi.last().copied().flatten().map(rsi_zone) else {
        return Vec::new();
    };
    if current_zone == "unknown" {
        return Vec::new();
    }
    let mut events = Vec::new();
    let end = bars.len().saturating_sub(21);
    for i in 1..end {
        let Some(value) = rsi[i] else {
            continue;
        };
        if rsi_zone(value) != current_zone {
            continue;
        }
        if rsi[i - 1].is_some_and(|prev| rsi_zone(prev) == current_zone) {
            continue;
        }
        let start = bars[i].close;
        if start <= 0.0 {
            continue;
        }
        let forward = (bars[i + 20].close - start) / start * 100.0;
        let min_low = bars[i + 1..=i + 20]
            .iter()
            .map(|b| b.low)
            .fold(f64::MAX, f64::min);
        events.push(AnalogEvent {
            kind: format!("daily_rsi_entered_{current_zone}"),
            at: bars[i].ts,
            close: round2(start),
            rsi14: round2(value),
            forward_return_20d_pct: Some(round2(forward)),
            max_drawdown_20d_pct: Some(round2((min_low - start) / start * 100.0)),
        });
    }
    events.sort_by_key(|e| std::cmp::Reverse(e.at));
    events.truncate(limit);
    events
}

fn rsi_zone(value: f64) -> String {
    if value >= 70.0 {
        "overbought".to_string()
    } else if value >= 60.0 {
        "strong".to_string()
    } else if value <= 30.0 {
        "oversold".to_string()
    } else if value <= 40.0 {
        "weak".to_string()
    } else {
        "neutral".to_string()
    }
}

fn stochastic_zone(value: f64) -> String {
    if value >= 80.0 {
        "overbought".to_string()
    } else if value >= 60.0 {
        "strong".to_string()
    } else if value <= 20.0 {
        "oversold".to_string()
    } else if value <= 40.0 {
        "weak".to_string()
    } else {
        "neutral".to_string()
    }
}

fn pso_zone(value: f64) -> String {
    if value >= 0.9 {
        "overbought".to_string()
    } else if value >= 0.2 {
        "strong".to_string()
    } else if value <= -0.9 {
        "oversold".to_string()
    } else if value <= -0.2 {
        "weak".to_string()
    } else {
        "neutral".to_string()
    }
}

fn sma_at(closes: &[f64], idx: usize, window: usize) -> Option<f64> {
    if idx + 1 < window {
        return None;
    }
    let start = idx + 1 - window;
    let slice = &closes[start..=idx];
    Some(slice.iter().sum::<f64>() / window as f64)
}

fn rsi14_series(closes: &[f64]) -> Vec<Option<f64>> {
    const WINDOW: usize = 14;
    let mut out = vec![None; closes.len()];
    if closes.len() <= WINDOW {
        return out;
    }
    let mut gains = 0.0;
    let mut losses = 0.0;
    for i in 1..=WINDOW {
        let change = closes[i] - closes[i - 1];
        if change >= 0.0 {
            gains += change;
        } else {
            losses += -change;
        }
    }
    let mut avg_gain = gains / WINDOW as f64;
    let mut avg_loss = losses / WINDOW as f64;
    out[WINDOW] = Some(rsi_from_avgs(avg_gain, avg_loss));
    for i in WINDOW + 1..closes.len() {
        let change = closes[i] - closes[i - 1];
        let gain = change.max(0.0);
        let loss = (-change).max(0.0);
        avg_gain = (avg_gain * (WINDOW as f64 - 1.0) + gain) / WINDOW as f64;
        avg_loss = (avg_loss * (WINDOW as f64 - 1.0) + loss) / WINDOW as f64;
        out[i] = Some(rsi_from_avgs(avg_gain, avg_loss));
    }
    out
}

fn stochastic_k_series(bars: &[TechnicalBar], window: usize) -> Vec<Option<f64>> {
    let mut out = vec![None; bars.len()];
    if window == 0 {
        return out;
    }
    for idx in 0..bars.len() {
        if idx + 1 < window {
            continue;
        }
        let slice = &bars[idx + 1 - window..=idx];
        let high = slice.iter().map(|b| b.high).fold(f64::MIN, f64::max);
        let low = slice.iter().map(|b| b.low).fold(f64::MAX, f64::min);
        let range = high - low;
        out[idx] = Some(if range.abs() < f64::EPSILON {
            50.0
        } else {
            ((bars[idx].close - low) / range * 100.0).clamp(0.0, 100.0)
        });
    }
    out
}

fn sma_optional(values: &[Option<f64>], window: usize) -> Vec<Option<f64>> {
    let mut out = vec![None; values.len()];
    if window == 0 {
        return out;
    }
    for idx in 0..values.len() {
        if idx + 1 < window {
            continue;
        }
        let slice = &values[idx + 1 - window..=idx];
        if slice.iter().all(Option::is_some) {
            out[idx] =
                Some(slice.iter().map(|v| v.unwrap_or_default()).sum::<f64>() / window as f64);
        }
    }
    out
}

fn ema_optional(values: &[Option<f64>], window: usize) -> Vec<Option<f64>> {
    let mut out = vec![None; values.len()];
    if window == 0 {
        return out;
    }
    let alpha = 2.0 / (window as f64 + 1.0);
    let mut ema = None;
    for (idx, value) in values.iter().enumerate() {
        let Some(value) = value else {
            continue;
        };
        let next = ema.map_or(*value, |prev| alpha * *value + (1.0 - alpha) * prev);
        ema = Some(next);
        out[idx] = Some(next);
    }
    out
}

fn pso_series(
    bars: &[TechnicalBar],
    stochastic_window: usize,
    smoothing_window: usize,
) -> Vec<Option<f64>> {
    let stochastic = stochastic_k_series(bars, stochastic_window);
    let normalized = stochastic
        .iter()
        .map(|value| value.map(|k| 0.1 * (k - 50.0)))
        .collect::<Vec<_>>();
    let first = ema_optional(&normalized, smoothing_window);
    let second = ema_optional(&first, smoothing_window);
    second
        .into_iter()
        .map(|value| {
            value.map(|v| {
                let exp = v.exp();
                (exp - 1.0) / (exp + 1.0)
            })
        })
        .collect()
}

fn true_range_series(bars: &[TechnicalBar]) -> Vec<Option<f64>> {
    let mut out = vec![None; bars.len()];
    for idx in 0..bars.len() {
        let range = if idx == 0 {
            bars[idx].high - bars[idx].low
        } else {
            let prev_close = bars[idx - 1].close;
            (bars[idx].high - bars[idx].low)
                .max((bars[idx].high - prev_close).abs())
                .max((bars[idx].low - prev_close).abs())
        };
        out[idx] = Some(range.max(0.0));
    }
    out
}

fn wilder_optional(values: &[Option<f64>], window: usize) -> Vec<Option<f64>> {
    let mut out = vec![None; values.len()];
    if window == 0 || values.len() < window {
        return out;
    }
    let mut smoothed = None;
    for idx in 0..values.len() {
        let Some(value) = values[idx] else {
            continue;
        };
        if smoothed.is_none() {
            if idx + 1 < window {
                continue;
            }
            let slice = &values[idx + 1 - window..=idx];
            if slice.iter().all(Option::is_some) {
                smoothed = Some(slice.iter().map(|v| v.unwrap_or_default()).sum::<f64>());
            }
        } else if let Some(prev) = smoothed {
            smoothed = Some(prev - (prev / window as f64) + value);
        }
        out[idx] = smoothed.map(|v| v / window as f64);
    }
    out
}

fn trailing_return(bars: &[TechnicalBar], window: usize) -> Option<f64> {
    if window == 0 || bars.len() <= window {
        return None;
    }
    let latest = bars.last()?.close;
    let base = bars.get(bars.len() - 1 - window)?.close;
    pct_vs(latest, base)
}

fn relative_return(
    bars: &[TechnicalBar],
    benchmark_bars: &[TechnicalBar],
    window: usize,
) -> Option<f64> {
    let symbol_return = trailing_return(bars, window)?;
    let benchmark_return = trailing_return(benchmark_bars, window)?;
    Some(round2(symbol_return - benchmark_return))
}

fn latest_delta(values: &[Option<f64>]) -> Option<f64> {
    let latest_idx = values.iter().rposition(Option::is_some)?;
    let latest = values[latest_idx]?;
    let prev_idx = values[..latest_idx].iter().rposition(Option::is_some)?;
    let prev = values[prev_idx]?;
    Some(round2(latest - prev))
}

fn rsi_from_avgs(avg_gain: f64, avg_loss: f64) -> f64 {
    if avg_loss == 0.0 {
        return 100.0;
    }
    let rs = avg_gain / avg_loss;
    100.0 - (100.0 / (1.0 + rs))
}

fn weekly_bars(daily: &[TechnicalBar]) -> Vec<TechnicalBar> {
    let mut out = Vec::new();
    for bar in daily {
        let iso = bar.ts.date_naive().iso_week();
        let same_week = out
            .last()
            .is_some_and(|last: &TechnicalBar| last.ts.date_naive().iso_week() == iso);
        if same_week {
            if let Some(last) = out.last_mut() {
                last.ts = bar.ts;
                last.close = bar.close;
                last.high = last.high.max(bar.high);
                last.low = last.low.min(bar.low);
                last.volume += bar.volume;
            }
        } else {
            out.push(*bar);
        }
    }
    out
}

fn pct_vs(value: f64, base: f64) -> Option<f64> {
    (base != 0.0)
        .then(|| (value - base) / base * 100.0)
        .map(round2)
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn bar(day: i64, close: f64) -> TechnicalBar {
        bar_ohlc(day, close * 1.01, close * 0.99, close)
    }

    fn bar_ohlc(day: i64, high: f64, low: f64, close: f64) -> TechnicalBar {
        bar_ohlcv(day, high, low, close, close * 10_000.0)
    }

    fn bar_ohlcv(day: i64, high: f64, low: f64, close: f64, volume: f64) -> TechnicalBar {
        TechnicalBar {
            ts: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap() + chrono::Duration::days(day),
            close,
            high,
            low,
            volume,
        }
    }

    #[test]
    fn classifies_extension_above_200_day_sma() {
        let mut bars = (0..220).map(|i| bar(i, 100.0)).collect::<Vec<_>>();
        bars.push(bar(220, 130.0));

        let state = build_technical_state("ENTG", &bars, &[]);

        assert_eq!(state.state, "extended");
        assert!(state.summary.contains("200-day SMA"));
        let sma200 = state
            .daily
            .as_ref()
            .unwrap()
            .sma
            .iter()
            .find(|s| s.window == 200)
            .unwrap();
        assert!(sma200.pct_vs.unwrap() > 20.0);
    }

    #[test]
    fn computes_rsi_zone_duration() {
        let bars = (0..40)
            .map(|i| bar(i, 100.0 + i as f64))
            .collect::<Vec<_>>();

        let state = build_technical_state("UP", &bars, &[]);
        let daily = state.intervals.iter().find(|i| i.interval == "1d").unwrap();

        assert_eq!(daily.rsi_zone, "overbought");
        assert!(daily.rsi_zone_bars > 1);
        assert!(daily.rsi_zone_since.is_some());
    }

    #[test]
    fn computes_stochastic_and_pso_for_interval_state() {
        let mut bars = (0..30)
            .map(|i| bar_ohlc(i, 105.0, 95.0, 100.0))
            .collect::<Vec<_>>();
        bars.extend((30..50).map(|i| {
            let close = 100.0 + (i - 29) as f64;
            bar_ohlc(i, close + 0.5, close - 3.0, close)
        }));

        let state = build_technical_state("MOMO", &bars, &[]);
        let daily = state.intervals.iter().find(|i| i.interval == "1d").unwrap();

        assert!(daily.stochastic_k14.unwrap() >= 80.0);
        assert!(daily.stochastic_d3.unwrap() >= 80.0);
        assert!(daily.pso.unwrap() >= 0.2);
        assert!(daily.pso32.is_some());
        assert_eq!(daily.stochastic_zone, "overbought");
        assert_ne!(daily.pso_zone, "unknown");
        assert_ne!(daily.pso32_zone, "unknown");
        assert!(state.summary.contains("Stoch 14 %K"));
        assert!(state.summary.contains("PSO 8/25"));
        assert!(state.summary.contains("PSO 32"));
    }

    #[test]
    fn oscillator_extension_blocks_constructive_entry_read() {
        let mut bars = (0..210)
            .map(|i| bar_ohlc(i, 101.0, 99.0, 100.0))
            .collect::<Vec<_>>();
        bars.extend((210..245).map(|i| {
            let close = 100.0 + (i - 209) as f64 * 0.32;
            bar_ohlc(i, close + 0.2, close - 2.0, close)
        }));

        let state = build_technical_state("AVGO", &bars, &[]);

        assert_eq!(state.state, "extended");
        assert_eq!(state.setup.entry_stance, "avoid_chase");
        let sma200 = state
            .daily
            .as_ref()
            .unwrap()
            .sma
            .iter()
            .find(|s| s.window == 200)
            .unwrap();
        assert!(sma200.pct_vs.unwrap() < 20.0);
    }

    #[test]
    fn records_200_day_crosses() {
        let mut bars = (0..210).map(|i| bar(i, 100.0)).collect::<Vec<_>>();
        bars.push(bar(210, 90.0));
        bars.push(bar(211, 120.0));

        let state = build_technical_state("X", &bars, &[]);

        assert!(
            state
                .last_crosses
                .iter()
                .any(|c| c.window == 200 && c.direction == "up")
        );
    }

    #[test]
    fn classifies_200_day_reclaim_as_actionable_setup() {
        let mut bars = (0..220).map(|i| bar(i, 100.0)).collect::<Vec<_>>();
        bars.push(bar(220, 92.0));
        bars.push(bar(221, 93.0));
        bars.push(bar(222, 94.0));
        bars.push(bar(223, 96.0));
        bars.push(bar(224, 100.0));
        bars.push(bar(225, 103.0));

        let state = build_technical_state("RCLM", &bars, &[]);

        assert_eq!(state.setup.kind, "200d_reclaim");
        assert_eq!(state.setup.entry_stance, "actionable");
    }

    #[test]
    fn classifies_near_200_day_reclaim_as_watch_setup() {
        let mut bars = (0..220).map(|i| bar(i, 100.0)).collect::<Vec<_>>();
        bars.push(bar(220, 92.0));
        bars.push(bar(221, 94.0));
        bars.push(bar(222, 96.0));
        bars.push(bar(223, 98.5));

        let state = build_technical_state("WATCH", &bars, &[]);

        assert_eq!(state.setup.kind, "200d_reclaim_watch");
        assert_eq!(state.setup.entry_stance, "wait_reclaim");
    }

    #[test]
    fn classifies_extended_run_as_avoid_chase() {
        let mut bars = (0..220).map(|i| bar(i, 100.0)).collect::<Vec<_>>();
        bars.push(bar(220, 135.0));

        let state = build_technical_state("HOT", &bars, &[]);

        assert_eq!(state.state, "extended");
        assert_eq!(state.setup.kind, "extended_run");
        assert_eq!(state.setup.entry_stance, "avoid_chase");
    }

    #[test]
    fn oversold_above_200_day_is_pullback_watch_not_deteriorating() {
        let mut bars = (0..220)
            .map(|i| {
                let close = 80.0 + i as f64 * 0.15;
                bar_ohlc(i, close + 1.0, close - 1.0, close)
            })
            .collect::<Vec<_>>();
        bars.extend((220..236).map(|i| {
            let close = 112.0 - (i - 219) as f64 * 0.7;
            bar_ohlc(i, close + 0.6, close - 1.4, close)
        }));

        let state = build_technical_state("AVGO", &bars, &[]);

        assert_eq!(state.state, "pullback_watch");
        assert_eq!(state.setup.kind, "pullback_watch");
        assert_eq!(state.setup.entry_stance, "wait_reversal");
        assert_eq!(
            state.cross.as_ref().map(|c| c.buy_timing.as_str()),
            Some("pullback_watch")
        );
        assert!(state.summary.contains("cross read"));
    }

    #[test]
    fn breakdown_below_200_day_with_distribution_is_deteriorating() {
        let mut bars = (0..220)
            .map(|i| bar_ohlcv(i, 101.0, 99.0, 100.0, 10_000.0))
            .collect::<Vec<_>>();
        bars.push(bar_ohlcv(220, 96.0, 94.0, 95.0, 12_000.0));
        bars.push(bar_ohlcv(221, 90.0, 87.5, 88.0, 100_000.0));

        let state = build_technical_state("BRK", &bars, &[]);

        assert_eq!(state.state, "deteriorating");
        assert_eq!(state.setup.kind, "breakdown");
        assert_eq!(state.setup.entry_stance, "avoid");
        let cross = state.cross.as_ref().unwrap();
        assert_eq!(cross.trend_state, "breakdown");
        assert_eq!(cross.buy_timing, "avoid_breakdown");
        assert_eq!(
            state
                .daily
                .as_ref()
                .and_then(|d| d.volume.as_ref())
                .map(|v| v.state.as_str()),
            Some("distribution")
        );
    }

    #[test]
    fn computes_relative_strength_against_benchmarks() {
        let bars = (0..240)
            .map(|i| bar(i, 100.0 + i as f64 * 0.12))
            .collect::<Vec<_>>();
        let qqq = (0..240)
            .map(|i| bar(i, 100.0 + i as f64 * 0.04))
            .collect::<Vec<_>>();
        let smh = (0..240)
            .map(|i| bar(i, 110.0 - i as f64 * 0.02))
            .collect::<Vec<_>>();

        let state = build_technical_state_with_benchmarks(
            "LEAD",
            &bars,
            &[],
            &[("QQQ", qqq), ("SMH", smh)],
        );

        let daily = state.daily.as_ref().unwrap();
        assert_eq!(daily.relative_strength.len(), 2);
        assert!(
            daily
                .relative_strength
                .iter()
                .all(|row| row.state == "outperforming")
        );
        assert_eq!(
            state
                .cross
                .as_ref()
                .map(|c| c.relative_strength_state.as_str()),
            Some("outperforming")
        );
    }

    #[test]
    fn builds_weekly_interval_from_daily_bars() {
        let bars = (0..30)
            .map(|i| bar(i, 100.0 + i as f64))
            .collect::<Vec<_>>();

        let state = build_technical_state("W", &bars, &[]);
        let weekly = state.intervals.iter().find(|i| i.interval == "1w").unwrap();

        assert!(weekly.bars < bars.len());
        assert_eq!(weekly.close, bars.last().map(|b| b.close));
    }
}
