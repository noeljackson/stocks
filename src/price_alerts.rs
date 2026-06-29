//! Price alert rules and trigger evaluation.
//!
//! Rules are mutable operator/system intent. Events are append-only trigger
//! receipts that fan out through existing alert and attention surfaces.

use std::time::Duration;

use anyhow::Result;
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::platform::{
    bus::Bus,
    domain::AlertKind,
    store::Store,
    subjects,
    technical::{
        CrossTechnical, TechnicalBar, TechnicalState, build_technical_state_with_benchmarks,
    },
};

pub const ATTENTION_KIND: &str = "price_alert";
pub const SOURCE: &str = "price_alert";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PriceAlertRule {
    pub id: i64,
    pub symbol: String,
    pub thesis_id: Option<Uuid>,
    pub origin: String,
    pub intent: String,
    pub direction: String,
    pub target_price: f64,
    pub label: String,
    pub rationale: Option<String>,
    pub semantic_key: Option<String>,
    pub status: String,
    pub source_ref: serde_json::Value,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub triggered_at: Option<DateTime<Utc>>,
    pub disabled_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PriceAlertEvent {
    pub id: i64,
    pub rule_id: i64,
    pub symbol: String,
    pub thesis_id: Option<Uuid>,
    pub triggered_at: DateTime<Utc>,
    pub trigger_ts: DateTime<Utc>,
    pub trigger_interval: String,
    pub trigger_price: f64,
    pub rule_snapshot: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceAlertRuleInput {
    pub symbol: String,
    #[serde(default)]
    pub thesis_id: Option<Uuid>,
    #[serde(default = "manual_origin")]
    pub origin: String,
    #[serde(default = "watch_intent")]
    pub intent: String,
    pub direction: String,
    pub target_price: f64,
    pub label: String,
    #[serde(default)]
    pub rationale: Option<String>,
    #[serde(default)]
    pub semantic_key: Option<String>,
    #[serde(default)]
    pub source_ref: serde_json::Value,
    #[serde(default)]
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct PriceAlertRulePatch {
    #[serde(default)]
    pub intent: Option<String>,
    #[serde(default)]
    pub direction: Option<String>,
    #[serde(default)]
    pub target_price: Option<f64>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub rationale: Option<String>,
    #[serde(default)]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub status: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PriceTrigger {
    pub ts: DateTime<Utc>,
    pub interval: String,
    pub price: f64,
}

#[derive(Debug, Clone, PartialEq)]
struct ParsedMarketBar {
    symbol: String,
    interval: String,
    ts: DateTime<Utc>,
    high: f64,
    low: f64,
    close: f64,
}

#[must_use]
pub fn rule_crossed(rule: &PriceAlertRule, high: f64, low: f64) -> bool {
    match rule.direction.as_str() {
        "above" => high >= rule.target_price,
        "below" => low <= rule.target_price,
        _ => false,
    }
}

pub fn validate_rule_input(input: &PriceAlertRuleInput) -> Result<()> {
    let symbol = input.symbol.trim();
    anyhow::ensure!(!symbol.is_empty(), "symbol required");
    anyhow::ensure!(input.target_price > 0.0, "target_price must be positive");
    validate_origin(&input.origin)?;
    validate_intent(&input.intent)?;
    validate_direction(&input.direction)?;
    anyhow::ensure!(!input.label.trim().is_empty(), "label required");
    Ok(())
}

pub fn validate_rule_patch(patch: &PriceAlertRulePatch) -> Result<()> {
    if let Some(intent) = &patch.intent {
        validate_intent(intent)?;
    }
    if let Some(direction) = &patch.direction {
        validate_direction(direction)?;
    }
    if let Some(price) = patch.target_price {
        anyhow::ensure!(price > 0.0, "target_price must be positive");
    }
    if let Some(label) = &patch.label {
        anyhow::ensure!(!label.trim().is_empty(), "label required");
    }
    if let Some(status) = &patch.status {
        validate_status(status)?;
    }
    Ok(())
}

fn validate_origin(value: &str) -> Result<()> {
    anyhow::ensure!(matches!(value, "manual" | "ai"), "invalid origin");
    Ok(())
}

fn validate_intent(value: &str) -> Result<()> {
    anyhow::ensure!(
        matches!(value, "watch" | "entry" | "invalidation" | "exit"),
        "invalid intent"
    );
    Ok(())
}

fn validate_direction(value: &str) -> Result<()> {
    anyhow::ensure!(matches!(value, "above" | "below"), "invalid direction");
    Ok(())
}

fn validate_status(value: &str) -> Result<()> {
    anyhow::ensure!(
        matches!(value, "active" | "triggered" | "disabled" | "expired"),
        "invalid status"
    );
    Ok(())
}

pub async fn run(store: Store, bus: Bus, interval: Duration) -> Result<()> {
    bus.ensure_stream(subjects::STREAM_MARKET, subjects::MARKET_STREAM_SUBJECTS)
        .await?;
    let _handle = consume_market_bars(store.clone(), bus.clone()).await?;
    loop {
        if let Err(e) = expire_rules(&store).await {
            error!(error = %e, "price alert expiry pass failed");
        }
        if let Err(e) = generate_ai_rules(&store).await {
            error!(error = %e, "price alert AI generation pass failed");
        }
        if let Err(e) = sweep_latest_prices(&store, &bus).await {
            error!(error = %e, "price alert latest-price sweep failed");
        }
        tokio::time::sleep(interval).await;
    }
}

async fn consume_market_bars(
    store: Store,
    bus: Bus,
) -> Result<crate::platform::bus::ConsumerHandle> {
    bus.clone()
        .consume(
            subjects::STREAM_MARKET,
            "price-alerts-market-bars",
            subjects::MARKET_BAR_FILTER,
            move |msg| {
                let store = store.clone();
                let bus = bus.clone();
                async move {
                    let Some(payload) =
                        parse_market_bar_payload(&msg.payload, msg.subject.as_str())
                    else {
                        debug!(subject = %msg.subject, "skipping non-price market.bar payload");
                        return Ok(());
                    };
                    trigger_symbol_rules(
                        &store,
                        &bus,
                        &payload.symbol,
                        &payload.interval,
                        payload.ts,
                        payload.high,
                        payload.low,
                        payload.close,
                    )
                    .await
                }
            },
        )
        .await
}

fn parse_market_bar_payload(payload: &[u8], subject: &str) -> Option<ParsedMarketBar> {
    let value: Value = serde_json::from_slice(payload).ok()?;
    let symbol = value_text(value.get("symbol"))
        .or_else(|| subject_market_bar_part(subject, 3))
        .map(|s| s.to_uppercase())?;
    let interval =
        value_text(value.get("interval")).or_else(|| subject_market_bar_part(subject, 2))?;
    let time = value_text(value.get("time"))
        .or_else(|| value_text(value.get("ts")))
        .or_else(|| value_text(value.get("start")))
        .or_else(|| value_text(value.get("at")))?;
    let ts = parse_bar_time(&time)?;
    let close = value_number(value.get("close").or_else(|| value.get("c")))?;
    let high = value_number(value.get("high").or_else(|| value.get("h"))).unwrap_or(close);
    let low = value_number(value.get("low").or_else(|| value.get("l"))).unwrap_or(close);
    Some(ParsedMarketBar {
        symbol,
        interval,
        ts,
        high,
        low,
        close,
    })
}

fn subject_market_bar_part(subject: &str, index: usize) -> Option<String> {
    let parts: Vec<&str> = subject.split('.').collect();
    if parts.len() < 4 || parts.first() != Some(&"market") || parts.get(1) != Some(&"bar") {
        return None;
    }
    if index == 3 {
        Some(parts[3..].join("."))
    } else {
        parts.get(index).map(|s| (*s).to_string())
    }
}

fn parse_bar_time(value: &str) -> Option<DateTime<Utc>> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(value) {
        return Some(dt.with_timezone(&Utc));
    }
    let date = NaiveDate::parse_from_str(value, "%Y-%m-%d").ok()?;
    let naive = date.and_hms_opt(0, 0, 0)?;
    Some(DateTime::from_naive_utc_and_offset(naive, Utc))
}

fn value_text(value: Option<&Value>) -> Option<String> {
    value?
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn value_number(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Number(n) => n.as_f64().filter(|n| n.is_finite()),
        Value::String(s) => s.trim().parse::<f64>().ok().filter(|n| n.is_finite()),
        _ => None,
    }
}

pub async fn sweep_latest_prices(store: &Store, bus: &Bus) -> Result<()> {
    for symbol in store.active_price_alert_symbols().await? {
        if let Some((ts, high, low, close)) = store.latest_daily_bar_for_alert(&symbol).await? {
            trigger_symbol_rules(store, bus, &symbol, "1D", ts, high, low, close).await?;
        }
        if let Some((ts, high, low, close)) = store.latest_intraday_bar_for_alert(&symbol).await? {
            trigger_symbol_rules(store, bus, &symbol, "1m", ts, high, low, close).await?;
        }
    }
    Ok(())
}

async fn expire_rules(store: &Store) -> Result<()> {
    let count = store.expire_price_alert_rules().await?;
    if count > 0 {
        info!(count, "expired price alert rules");
    }
    Ok(())
}

async fn trigger_symbol_rules(
    store: &Store,
    bus: &Bus,
    symbol: &str,
    interval: &str,
    ts: DateTime<Utc>,
    high: f64,
    low: f64,
    close: f64,
) -> Result<()> {
    for rule in store.active_price_alert_rules_for_symbol(symbol).await? {
        if !rule_crossed(&rule, high, low) {
            continue;
        }
        let trigger = PriceTrigger {
            ts,
            interval: interval.to_string(),
            price: close,
        };
        if let Some(event) = store.trigger_price_alert_rule(&rule, &trigger).await? {
            emit_trigger(store, bus, &rule, &event).await?;
        }
    }
    Ok(())
}

async fn emit_trigger(
    store: &Store,
    bus: &Bus,
    rule: &PriceAlertRule,
    event: &PriceAlertEvent,
) -> Result<()> {
    let payload = json!({
        "symbol": rule.symbol,
        "rule_id": rule.id,
        "event_id": event.id,
        "thesis_id": rule.thesis_id,
        "origin": rule.origin,
        "intent": rule.intent,
        "direction": rule.direction,
        "target_price": rule.target_price,
        "trigger_price": event.trigger_price,
        "trigger_interval": event.trigger_interval,
        "label": rule.label,
        "rationale": rule.rationale,
    });
    store
        .insert_alert(
            AlertKind::PriceAlert,
            Some(&rule.symbol),
            payload.to_string().as_bytes(),
        )
        .await?;
    let severity = if matches!(rule.intent.as_str(), "entry" | "exit") {
        crate::attention::severity::DECISION
    } else {
        crate::attention::severity::REVIEW
    };
    store
        .upsert_attention(
            ATTENTION_KIND,
            Some(&rule.symbol),
            rule.thesis_id,
            None,
            severity,
            &format!("{} price alert hit", rule.symbol),
            Some(&format!(
                "{} crossed {} {:.2}: {}",
                rule.symbol, rule.direction, rule.target_price, rule.label
            )),
            SOURCE,
            payload.clone(),
        )
        .await?;
    bus.publish(
        subjects::PRICE_ALERT_TRIGGERED,
        payload.to_string().as_bytes(),
    )
    .await?;
    Ok(())
}

pub async fn generate_ai_rules(store: &Store) -> Result<usize> {
    let symbols = store.priority_scan_symbols(80).await?;
    let mut created = 0_usize;
    for symbol in symbols {
        match generate_ai_rules_for_symbol(store, &symbol).await {
            Ok(n) => created += n,
            Err(e) => warn!(symbol, error = %e, "generate price alert rules failed"),
        }
    }
    Ok(created)
}

async fn generate_ai_rules_for_symbol(store: &Store, symbol: &str) -> Result<usize> {
    let daily = store.daily_technical_bars_for(symbol, 365 * 30).await?;
    if daily.len() < 60 {
        return Ok(0);
    }
    let benchmarks = benchmark_bars(store, symbol).await;
    let state = build_technical_state_with_benchmarks(symbol, &daily, &[], &benchmarks);
    let Some(cross) = state.cross.as_ref() else {
        return Ok(0);
    };
    let mut created = 0_usize;
    for input in candidate_ai_rules(symbol, &daily, &state, cross) {
        if store.create_price_alert_rule(input).await?.is_some() {
            created += 1;
        }
    }
    Ok(created)
}

async fn benchmark_bars(store: &Store, symbol: &str) -> Vec<(&'static str, Vec<TechnicalBar>)> {
    let mut out = Vec::new();
    for benchmark in ["QQQ", "SMH"] {
        if benchmark.eq_ignore_ascii_case(symbol) {
            continue;
        }
        if let Ok(rows) = store.daily_technical_bars_for(benchmark, 365 * 30).await {
            if !rows.is_empty() {
                out.push((benchmark, rows));
            }
        }
    }
    out
}

fn candidate_ai_rules(
    symbol: &str,
    daily: &[TechnicalBar],
    state: &TechnicalState,
    cross: &CrossTechnical,
) -> Vec<PriceAlertRuleInput> {
    let close = daily.last().map(|b| b.close).unwrap_or_default();
    if close <= 0.0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    let add_rule = |out: &mut Vec<PriceAlertRuleInput>,
                    intent: &str,
                    direction: &str,
                    target_price: f64,
                    semantic_key: &str,
                    label: String,
                    rationale: String| {
        if target_price <= 0.0 || (target_price - close).abs() / close < 0.0025 {
            return;
        }
        out.push(PriceAlertRuleInput {
            symbol: symbol.to_ascii_uppercase(),
            thesis_id: None,
            origin: "ai".to_string(),
            intent: intent.to_string(),
            direction: direction.to_string(),
            target_price,
            label,
            rationale: Some(rationale),
            semantic_key: Some(semantic_key.to_string()),
            source_ref: json!({
                "source": "cross_technical",
                "technical_state": state.state,
                "entry_stance": state.setup.entry_stance,
                "buy_timing": cross.buy_timing,
                "trend_state": cross.trend_state,
                "momentum_state": cross.momentum_state,
            }),
            expires_at: Some(Utc::now() + chrono::Duration::days(30)),
        });
    };

    if matches!(
        cross.buy_timing.as_str(),
        "pullback_reversal" | "pullback_watch"
    ) {
        if let Some(level) = sma_value(state, 20).or_else(|| sma_value(state, 50)) {
            add_rule(
                &mut out,
                "entry",
                "above",
                level,
                "technical_reclaim_short_ma",
                format!("{symbol} entry watch: reclaim short moving average"),
                "Trend support is intact; reclaiming the short moving average would confirm timing."
                    .to_string(),
            );
        }
        if let Some(level) = recent_high(daily, 10) {
            add_rule(
                &mut out,
                "entry",
                "above",
                level,
                "technical_break_short_swing_high",
                format!("{symbol} entry watch: break short swing high"),
                "Oversold momentum is stabilizing; a short swing-high break would confirm reversal."
                    .to_string(),
            );
        }
    }

    if !matches!(cross.trend_state.as_str(), "extended_chase" | "breakdown") {
        if let Some(level) = sma_value(state, 200).or_else(|| recent_low(daily, 20)) {
            add_rule(
                &mut out,
                "invalidation",
                "below",
                level,
                "technical_break_trend_support",
                format!("{symbol} risk watch: lose trend support"),
                "A break of trend support would change the technical read from pullback to damage."
                    .to_string(),
            );
        }
    }

    out.truncate(3);
    out
}

fn sma_value(state: &TechnicalState, window: usize) -> Option<f64> {
    state
        .daily
        .as_ref()?
        .sma
        .iter()
        .find(|s| s.window == window)
        .and_then(|s| s.value)
}

fn recent_high(daily: &[TechnicalBar], window: usize) -> Option<f64> {
    let start = daily.len().saturating_sub(window);
    daily
        .get(start..)?
        .iter()
        .map(|bar| bar.high)
        .reduce(f64::max)
}

fn recent_low(daily: &[TechnicalBar], window: usize) -> Option<f64> {
    let start = daily.len().saturating_sub(window);
    daily
        .get(start..)?
        .iter()
        .map(|bar| bar.low)
        .reduce(f64::min)
}

fn manual_origin() -> String {
    "manual".to_string()
}

fn watch_intent() -> String {
    "watch".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn rule(direction: &str, target_price: f64) -> PriceAlertRule {
        PriceAlertRule {
            id: 1,
            symbol: "AVGO".to_string(),
            thesis_id: None,
            origin: "manual".to_string(),
            intent: "watch".to_string(),
            direction: direction.to_string(),
            target_price,
            label: "test".to_string(),
            rationale: None,
            semantic_key: None,
            status: "active".to_string(),
            source_ref: json!({}),
            expires_at: None,
            created_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            updated_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            triggered_at: None,
            disabled_at: None,
        }
    }

    #[test]
    fn rule_crossed_uses_high_for_above_and_low_for_below() {
        assert!(rule_crossed(&rule("above", 105.0), 106.0, 99.0));
        assert!(!rule_crossed(&rule("above", 107.0), 106.0, 99.0));
        assert!(rule_crossed(&rule("below", 99.0), 106.0, 98.5));
        assert!(!rule_crossed(&rule("below", 98.0), 106.0, 98.5));
    }

    #[test]
    fn validation_rejects_bad_rule_input() {
        let mut input = PriceAlertRuleInput {
            symbol: "AVGO".to_string(),
            thesis_id: None,
            origin: "ai".to_string(),
            intent: "entry".to_string(),
            direction: "above".to_string(),
            target_price: 100.0,
            label: "entry".to_string(),
            rationale: None,
            semantic_key: None,
            source_ref: json!({}),
            expires_at: None,
        };
        assert!(validate_rule_input(&input).is_ok());
        input.direction = "around".to_string();
        assert!(validate_rule_input(&input).is_err());
    }

    #[test]
    fn parse_market_bar_payload_accepts_current_shape() {
        let payload = serde_json::to_vec(&json!({
            "symbol": "avgo",
            "interval": "1D",
            "time": "2026-06-01",
            "open": 100.0,
            "high": 104.0,
            "low": 99.0,
            "close": 103.0,
            "volume": 1000.0,
            "status": "delayed",
        }))
        .unwrap();
        let bar = parse_market_bar_payload(&payload, "market.bar.1D.AVGO").unwrap();
        assert_eq!(bar.symbol, "AVGO");
        assert_eq!(bar.interval, "1D");
        assert_eq!(bar.ts, Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap());
        assert_eq!(bar.high, 104.0);
        assert_eq!(bar.low, 99.0);
        assert_eq!(bar.close, 103.0);
    }

    #[test]
    fn parse_market_bar_payload_accepts_subject_and_short_keys() {
        let payload = serde_json::to_vec(&json!({
            "ts": "2026-06-01T14:30:00Z",
            "h": "204.50",
            "l": "199.25",
            "c": "203.00",
        }))
        .unwrap();
        let bar = parse_market_bar_payload(&payload, "market.bar.1m.2454.TW").unwrap();
        assert_eq!(bar.symbol, "2454.TW");
        assert_eq!(bar.interval, "1m");
        assert_eq!(bar.high, 204.5);
        assert_eq!(bar.low, 199.25);
        assert_eq!(bar.close, 203.0);
    }

    #[test]
    fn parse_market_bar_payload_skips_status_only_messages() {
        let payload = serde_json::to_vec(&json!({
            "symbol": "AVGO",
            "interval": "1W",
            "status": "entitlement_blocked",
            "provider": "fmp",
            "reason": "not entitled",
        }))
        .unwrap();
        assert!(parse_market_bar_payload(&payload, "market.bar.1W.AVGO").is_none());
    }
}
