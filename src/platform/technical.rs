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
}

#[derive(Debug, Clone, Serialize)]
pub struct DailyTechnical {
    pub as_of: DateTime<Utc>,
    pub close: f64,
    pub sma: Vec<SmaPoint>,
    pub pct_vs_252d_high: Option<f64>,
    pub pct_vs_252d_low: Option<f64>,
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
    let weekly = weekly_bars(daily);
    let mut intervals = Vec::new();
    intervals.push(interval_state("1d", daily));
    intervals.push(interval_state("1w", &weekly));
    for (label, bars) in intraday {
        intervals.push(interval_state(label, bars));
    }
    intervals.sort_by_key(|i| interval_rank(&i.interval));

    let daily_technical = daily_state(daily);
    let state = classify_state(daily, &daily_technical, &intervals);
    let setup = classify_setup(daily, &daily_technical, &intervals, &state);
    let summary = state_summary(&state, &daily_technical, &intervals);
    TechnicalState {
        symbol: symbol.to_ascii_uppercase(),
        as_of: daily.last().map(|b| b.ts),
        state,
        setup,
        summary,
        daily: daily_technical,
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

fn daily_state(bars: &[TechnicalBar]) -> Option<DailyTechnical> {
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
    })
}

fn interval_state(interval: &str, bars: &[TechnicalBar]) -> IntervalTechnical {
    let closes = bars.iter().map(|b| b.close).collect::<Vec<_>>();
    let rsi = rsi14_series(&closes);
    let latest_rsi = rsi.last().copied().flatten();
    let zone = latest_rsi.map_or_else(|| "unknown".to_string(), rsi_zone);
    let (zone_bars, zone_since) = current_zone_span(bars, &rsi, &zone);
    IntervalTechnical {
        interval: interval.to_string(),
        bars: bars.len(),
        as_of: bars.last().map(|b| b.ts),
        close: bars.last().map(|b| b.close),
        rsi14: latest_rsi.map(round2),
        rsi_zone: zone,
        rsi_zone_bars: zone_bars,
        rsi_zone_since: zone_since,
    }
}

fn classify_state(
    daily: &[TechnicalBar],
    daily_technical: &Option<DailyTechnical>,
    intervals: &[IntervalTechnical],
) -> String {
    let Some(d) = daily_technical else {
        return "unknown".to_string();
    };
    let rsi_1d = intervals
        .iter()
        .find(|i| i.interval == "1d")
        .and_then(|i| i.rsi14);
    let pct_vs_200 = d
        .sma
        .iter()
        .find(|s| s.window == 200)
        .and_then(|s| s.pct_vs);
    let pct_vs_50 = d.sma.iter().find(|s| s.window == 50).and_then(|s| s.pct_vs);
    if pct_vs_200.is_some_and(|v| v > 20.0)
        || d.pct_vs_252d_high.is_some_and(|v| v >= -2.0)
        || rsi_1d.is_some_and(|v| v >= 70.0)
    {
        return "extended".to_string();
    }
    if pct_vs_200.is_some_and(|v| v < -5.0) || rsi_1d.is_some_and(|v| v <= 40.0) {
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
        let entry_stance = if rsi_1d.is_some_and(|rsi| rsi >= 70.0) {
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
    let mut parts = vec![format!("technical state is {state}")];
    if let Some(v) = pct_200 {
        parts.push(format!("{v:+.1}% vs 200-day SMA"));
    }
    if let Some(v) = rsi_1d {
        parts.push(format!("RSI 14 daily {v:.1}"));
    }
    if let Some(v) = d.pct_vs_252d_high {
        parts.push(format!("{v:+.1}% vs 252-day high"));
    }
    parts.join("; ")
}

fn current_zone_span(
    bars: &[TechnicalBar],
    rsi: &[Option<f64>],
    current_zone: &str,
) -> (usize, Option<DateTime<Utc>>) {
    if current_zone == "unknown" {
        return (0, None);
    }
    let mut count = 0;
    let mut since = None;
    for idx in (0..rsi.len()).rev() {
        let Some(value) = rsi[idx] else {
            break;
        };
        if rsi_zone(value) != current_zone {
            break;
        }
        count += 1;
        since = bars.get(idx).map(|b| b.ts);
    }
    (count, since)
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
        TechnicalBar {
            ts: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap() + chrono::Duration::days(day),
            close,
            high: close * 1.01,
            low: close * 0.99,
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
