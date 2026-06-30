use chrono::{DateTime, Duration, NaiveTime, Utc};
use serde::Serialize;
use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HaltState {
    Unknown,
    NotHalted,
    Halted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NoTradeWindow {
    pub label: String,
    pub start_utc: NaiveTime,
    pub end_utc: NaiveTime,
}

#[derive(Debug, Clone)]
pub struct MarketReadinessInput {
    pub now: DateTime<Utc>,
    pub latest_bar_at: Option<DateTime<Utc>>,
    pub latest_price: Option<f64>,
    pub previous_close: Option<f64>,
    pub max_age_days: i64,
    pub max_gap_pct: f64,
    pub session_open: bool,
    pub session_label: String,
    pub halt_state: HaltState,
    pub corporate_actions_adjusted: bool,
    pub no_trade_windows_utc: Vec<NoTradeWindow>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MarketReadinessDecision {
    pub status: String,
    pub blocked_reasons: Vec<String>,
    pub snapshot: Value,
}

pub fn evaluate_market_readiness(input: &MarketReadinessInput) -> MarketReadinessDecision {
    let latest_price = input.latest_price.filter(|price| *price > 0.0);
    let latest_bar_age_days = input
        .latest_bar_at
        .map(|latest| input.now.signed_duration_since(latest).num_days());
    let gap_pct = match (
        latest_price,
        input.previous_close.filter(|price| *price > 0.0),
    ) {
        (Some(latest), Some(previous)) => Some(((latest - previous).abs() / previous) * 100.0),
        _ => None,
    };
    let active_window = input
        .no_trade_windows_utc
        .iter()
        .find(|window| window_contains(window, input.now.time()));

    let mut blocked_reasons = Vec::new();
    if latest_price.is_none() {
        blocked_reasons.push("market_price_missing".to_string());
    }
    match input.latest_bar_at {
        Some(latest) => {
            if input.now.signed_duration_since(latest) > Duration::days(input.max_age_days.max(1)) {
                blocked_reasons.push("market_bar_stale".to_string());
            }
        }
        None => blocked_reasons.push("market_bar_missing".to_string()),
    }
    if gap_pct.is_some_and(|gap| gap > input.max_gap_pct.max(0.0)) {
        blocked_reasons.push("market_bar_gap_anomaly".to_string());
    }
    if !input.session_open {
        blocked_reasons.push("market_session_closed".to_string());
    }
    if input.halt_state == HaltState::Halted {
        blocked_reasons.push("market_halt_or_suspension".to_string());
    }
    if !input.corporate_actions_adjusted {
        blocked_reasons.push("corporate_action_state_unsupported".to_string());
    }
    if active_window.is_some() {
        blocked_reasons.push("no_trade_window".to_string());
    }
    blocked_reasons.sort();
    blocked_reasons.dedup();

    let status = if blocked_reasons.is_empty() {
        "ready"
    } else {
        "blocked"
    };

    MarketReadinessDecision {
        status: status.to_string(),
        blocked_reasons: blocked_reasons.clone(),
        snapshot: json!({
            "status": status,
            "blocked_reasons": blocked_reasons,
            "latest_bar_at": input.latest_bar_at,
            "latest_bar_age_days": latest_bar_age_days,
            "max_age_days": input.max_age_days,
            "latest_price": latest_price,
            "previous_close": input.previous_close,
            "gap_pct": gap_pct,
            "max_gap_pct": input.max_gap_pct,
            "session_open": input.session_open,
            "session_label": input.session_label,
            "halt_state": input.halt_state,
            "corporate_actions_adjusted": input.corporate_actions_adjusted,
            "active_no_trade_window": active_window.map(|window| window.label.clone()),
            "no_trade_windows_utc": input.no_trade_windows_utc,
        }),
    }
}

fn window_contains(window: &NoTradeWindow, time: NaiveTime) -> bool {
    if window.start_utc <= window.end_utc {
        time >= window.start_utc && time <= window.end_utc
    } else {
        time >= window.start_utc || time <= window.end_utc
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use chrono::TimeZone;
    use pretty_assertions::assert_eq;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, 30, 15, 0, 0).unwrap()
    }

    fn base_input() -> MarketReadinessInput {
        MarketReadinessInput {
            now: now(),
            latest_bar_at: Some(now() - Duration::hours(2)),
            latest_price: Some(100.0),
            previous_close: Some(99.0),
            max_age_days: 1,
            max_gap_pct: 20.0,
            session_open: true,
            session_label: "regular".to_string(),
            halt_state: HaltState::NotHalted,
            corporate_actions_adjusted: true,
            no_trade_windows_utc: Vec::new(),
        }
    }

    fn reasons(input: MarketReadinessInput) -> Vec<String> {
        evaluate_market_readiness(&input).blocked_reasons
    }

    #[test]
    fn fresh_market_inputs_are_ready() {
        let decision = evaluate_market_readiness(&base_input());

        assert_eq!(decision.status, "ready");
        assert!(decision.blocked_reasons.is_empty());
    }

    #[test]
    fn missing_latest_price_blocks() {
        let mut input = base_input();
        input.latest_price = None;

        assert!(
            reasons(input)
                .iter()
                .any(|reason| reason == "market_price_missing")
        );
    }

    #[test]
    fn stale_daily_bar_blocks() {
        let mut input = base_input();
        input.latest_bar_at = Some(now() - Duration::days(3));

        assert!(
            reasons(input)
                .iter()
                .any(|reason| reason == "market_bar_stale")
        );
    }

    #[test]
    fn suspicious_gap_blocks() {
        let mut input = base_input();
        input.latest_price = Some(140.0);
        input.previous_close = Some(100.0);

        assert!(
            reasons(input)
                .iter()
                .any(|reason| reason == "market_bar_gap_anomaly")
        );
    }

    #[test]
    fn closed_session_blocks() {
        let mut input = base_input();
        input.session_open = false;
        input.session_label = "closed".to_string();

        assert!(
            reasons(input)
                .iter()
                .any(|reason| reason == "market_session_closed")
        );
    }

    #[test]
    fn halt_indicator_blocks() {
        let mut input = base_input();
        input.halt_state = HaltState::Halted;

        assert!(
            reasons(input)
                .iter()
                .any(|reason| reason == "market_halt_or_suspension")
        );
    }

    #[test]
    fn unsupported_corporate_action_state_blocks() {
        let mut input = base_input();
        input.corporate_actions_adjusted = false;

        assert!(
            reasons(input)
                .iter()
                .any(|reason| reason == "corporate_action_state_unsupported")
        );
    }

    #[test]
    fn configured_no_trade_window_blocks() {
        let mut input = base_input();
        input.no_trade_windows_utc = vec![NoTradeWindow {
            label: "open_auction".to_string(),
            start_utc: NaiveTime::from_hms_opt(14, 55, 0).unwrap(),
            end_utc: NaiveTime::from_hms_opt(15, 5, 0).unwrap(),
        }];

        assert!(
            reasons(input)
                .iter()
                .any(|reason| reason == "no_trade_window")
        );
    }
}
