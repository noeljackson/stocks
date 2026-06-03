//! HTTP routes — REST + SSE + SPA fallback.

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result as AnyResult};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response, Sse, sse::Event},
    routing::{get, post},
};
use chrono::NaiveDate;
use futures::{
    StreamExt,
    stream::{self, Stream},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::Row;
use tokio_stream::wrappers::BroadcastStream;
use tracing::warn;

use super::Gateway;
use crate::llm::prompts;
use crate::platform::{
    subjects,
    technical::{TechnicalState, build_technical_state},
};
use crate::web::Dist;

pub(super) fn build(gw: Arc<Gateway>) -> Router {
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/api/alerts", get(list_alerts))
        .route("/api/alerts/{id}/ack", post(ack_alert))
        .route("/api/regime", get(get_regime))
        .route("/api/tickers", get(list_tickers))
        .route("/api/theses", get(list_theses))
        .route("/api/thesis-declines", get(list_thesis_declines))
        .route(
            "/api/evidence-requirements",
            get(list_evidence_requirements),
        )
        .route("/api/evidence-items", get(list_evidence_items))
        .route("/api/research-evidence", get(list_research_evidence))
        .route(
            "/api/theses/{thesis_id}/transition",
            post(transition_thesis),
        )
        .route("/api/ticker-context", get(get_ticker_context))
        .route("/api/candles", get(get_candles))
        .route("/api/technical-state", get(get_technical_state))
        .route("/api/chat-analyst", post(post_chat_analyst))
        .route("/api/symbol-events", get(get_symbol_events))
        .route("/api/decisions", get(get_decisions).post(record_decision))
        .route("/api/decisions/{id}/replay", get(get_decision_replay))
        .route("/api/positions", get(get_positions))
        .route("/api/calibration", get(get_calibration))
        .route(
            "/api/watchlists",
            get(list_watchlists).post(create_watchlist),
        )
        .route(
            "/api/watchlists/{id}",
            axum::routing::delete(delete_watchlist),
        )
        .route(
            "/api/watchlists/{id}/members",
            get(list_watchlist_members).post(add_watchlist_member),
        )
        .route(
            "/api/watchlists/{id}/members/{symbol}",
            axum::routing::delete(remove_watchlist_member),
        )
        .route("/api/portfolio", get(get_portfolio).put(put_portfolio))
        .route("/api/discovery/candidates", get(list_pending_candidates))
        .route(
            "/api/discovery/candidates/{id}/confirm",
            post(confirm_candidate),
        )
        .route(
            "/api/discovery/candidates/{id}/reject",
            post(reject_candidate),
        )
        .route("/api/discovery-pool", get(list_discovery_pool))
        .route("/api/system-status", get(get_system_status))
        .route("/api/brain", get(get_brain_overview))
        .route("/api/brain-journal", get(get_brain_journal))
        .route("/api/brain-status", get(get_brain_status))
        .route("/api/attention", get(list_attention_items))
        .route("/api/attention/{id}/dismiss", post(dismiss_attention_item))
        .route(
            "/api/attention/{id}/transition",
            post(transition_attention_item),
        )
        .route(
            "/api/symbols/{symbol}/refresh-context",
            post(trigger_refresh_context),
        )
        .route("/api/stream", get(stream))
        .fallback(spa_handler)
        .with_state(gw)
}

#[derive(Debug, Deserialize)]
struct ThesesQuery {
    symbol: Option<String>,
}

async fn list_theses(
    State(gw): State<Arc<Gateway>>,
    Query(q): Query<ThesesQuery>,
) -> impl IntoResponse {
    let Some(sym) = q.symbol else {
        return (StatusCode::BAD_REQUEST, "symbol query param required").into_response();
    };
    match gw.store.theses_for_symbol(&sym).await {
        Ok(rows) => (StatusCode::OK, Json(rows)).into_response(),
        Err(e) => {
            warn!(symbol = %sym, error = %e, "list_theses failed");
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
    }
}

async fn list_thesis_declines(
    State(gw): State<Arc<Gateway>>,
    Query(q): Query<ThesesQuery>,
) -> impl IntoResponse {
    let Some(sym) = q.symbol else {
        return (StatusCode::BAD_REQUEST, "symbol query param required").into_response();
    };
    match gw.store.thesis_declines_for_symbol(&sym, 25).await {
        Ok(rows) => (StatusCode::OK, Json(rows)).into_response(),
        Err(e) => {
            warn!(symbol = %sym, error = %e, "list_thesis_declines failed");
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
    }
}

async fn list_evidence_requirements(
    State(gw): State<Arc<Gateway>>,
    Query(q): Query<ThesesQuery>,
) -> impl IntoResponse {
    let Some(sym) = q.symbol else {
        return (StatusCode::BAD_REQUEST, "symbol query param required").into_response();
    };
    match gw.store.evidence_requirements_for_symbol(&sym).await {
        Ok(v) => (StatusCode::OK, Json(v)).into_response(),
        Err(e) => {
            warn!(symbol = %sym, error = %e, "list_evidence_requirements failed");
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
    }
}

async fn list_research_evidence(
    State(gw): State<Arc<Gateway>>,
    Query(q): Query<ThesesQuery>,
) -> impl IntoResponse {
    let Some(sym) = q.symbol else {
        return (StatusCode::BAD_REQUEST, "symbol query param required").into_response();
    };
    match gw.store.research_evidence_for_symbol(&sym).await {
        Ok(v) => (StatusCode::OK, Json(v)).into_response(),
        Err(e) => {
            warn!(symbol = %sym, error = %e, "list_research_evidence failed");
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
    }
}

async fn list_evidence_items(
    State(gw): State<Arc<Gateway>>,
    Query(q): Query<ThesesQuery>,
) -> impl IntoResponse {
    let Some(sym) = q.symbol else {
        return (StatusCode::BAD_REQUEST, "symbol query param required").into_response();
    };
    match gw.store.evidence_items_for_symbol(&sym).await {
        Ok(v) => (StatusCode::OK, Json(v)).into_response(),
        Err(e) => {
            warn!(symbol = %sym, error = %e, "list_evidence_items failed");
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
    }
}

#[derive(Debug, Deserialize)]
struct TransitionReq {
    to: crate::platform::domain::ThesisState,
    #[serde(default)]
    rationale: String,
}

#[derive(Debug, Serialize)]
struct TransitionErr {
    error: String,
    missing: Vec<String>,
}

fn thesis_transition_event_payload(
    thesis_id: uuid::Uuid,
    thesis: &crate::platform::domain::ThesisDetail,
    to: crate::platform::domain::ThesisState,
    rationale: &str,
    at: chrono::DateTime<chrono::Utc>,
) -> serde_json::Value {
    serde_json::json!({
        "thesis_id": thesis_id,
        "symbol": thesis.symbol,
        "cluster_id": thesis.cluster_id,
        "from": thesis.state.as_str(),
        "to": to.as_str(),
        "rationale": rationale,
        "forecast": thesis.forecast,
        "conviction_tier": thesis.conviction_tier,
        "system_confidence": thesis.system_confidence,
        "system_confidence_components": thesis.system_confidence_components,
        "instrument": thesis.instrument,
        "intended_size": thesis.intended_size,
        "at": at,
    })
}

async fn transition_thesis(
    State(gw): State<Arc<Gateway>>,
    Path(thesis_id): Path<uuid::Uuid>,
    Json(req): Json<TransitionReq>,
) -> impl IntoResponse {
    use crate::thesis::substance;

    // 1. Load the thesis (we only need it for substance + current state).
    let theses = match gw.store.theses_for_symbol_id(thesis_id).await {
        Ok(v) => v,
        Err(e) => {
            warn!(thesis_id = %thesis_id, error = %e, "transition: load failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }
    };
    let Some(t) = theses.into_iter().next() else {
        return (
            StatusCode::NOT_FOUND,
            format!("thesis {thesis_id} not found"),
        )
            .into_response();
    };

    // 2. Build the SubstanceInput from the loaded thesis.
    let parse_conds = |v: &serde_json::Value| -> Vec<crate::platform::domain::Condition> {
        serde_json::from_value(v.clone()).unwrap_or_default()
    };
    let forecast_present = !t.forecast.is_null()
        && !matches!(&t.forecast, serde_json::Value::Object(o) if o.is_empty());
    let intended_size_present = !t.intended_size.is_null()
        && !matches!(&t.intended_size, serde_json::Value::Object(o) if o.is_empty());
    let sub_input = substance::Thesis {
        forecast_present,
        intended_size_present,
        conviction: parse_conds(&t.conviction_conditions),
        trigger: parse_conds(&t.trigger_conditions),
        invalidation: parse_conds(&t.invalidation_conditions),
        fulfillment: parse_conds(&t.fulfillment_conditions),
    };

    // 3. Check legality + substance.
    if let Err(missing) = substance::promotion_allowed(t.state, req.to, &sub_input) {
        let body = TransitionErr {
            error: if missing
                .first()
                .is_some_and(|s| s.starts_with("illegal transition"))
            {
                missing[0].clone()
            } else {
                format!("blocked by missing substance: {}", missing.join(", "))
            },
            missing,
        };
        return (StatusCode::BAD_REQUEST, Json(body)).into_response();
    }
    if matches!(req.to, crate::platform::domain::ThesisState::Actionable)
        && t.substance
            .as_ref()
            .is_some_and(|s| s.freshness_score < 0.85)
    {
        let sub = t.substance.as_ref().expect("checked above");
        let mut missing = vec![format!(
            "freshness_score {:.0}% below actionable threshold",
            sub.freshness_score * 100.0
        )];
        missing.extend(sub.freshness_penalties.clone());
        let body = TransitionErr {
            error: format!(
                "blocked by stale evidence: confidence capped at {}",
                sub.confidence_cap.as_deref().unwrap_or("medium")
            ),
            missing,
        };
        return (StatusCode::BAD_REQUEST, Json(body)).into_response();
    }

    // 4. Apply the transition + write a thesis_state_history row.
    if let Err(e) = gw
        .store
        .apply_state_transition(thesis_id, t.state, req.to, &req.rationale)
        .await
    {
        warn!(thesis_id = %thesis_id, error = %e, "transition: apply failed");
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }

    // 5. Emit the matching thesis.* event so downstream services (and the
    //    SSE feed) see it.
    let topic = match req.to {
        crate::platform::domain::ThesisState::Actionable => {
            crate::platform::subjects::THESIS_ACTIONABLE
        }
        crate::platform::domain::ThesisState::Disqualified => {
            crate::platform::subjects::THESIS_INVALIDATED
        }
        crate::platform::domain::ThesisState::Closed => crate::platform::subjects::THESIS_FULFILLED,
        _ => crate::platform::subjects::THESIS_UPDATED,
    };
    let payload =
        thesis_transition_event_payload(thesis_id, &t, req.to, &req.rationale, chrono::Utc::now());
    if let Err(e) = gw.bus.publish(topic, payload.to_string().as_bytes()).await {
        warn!(error = %e, "transition publish failed (best-effort)");
    }

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "thesis_id": thesis_id,
            "from": t.state,
            "to": req.to,
        })),
    )
        .into_response()
}

#[derive(Debug, Deserialize, Default)]
struct CalibrationQuery {
    #[serde(default)]
    days: Option<i64>,
}

async fn get_calibration(
    State(gw): State<Arc<Gateway>>,
    Query(q): Query<CalibrationQuery>,
) -> impl IntoResponse {
    let lookback = q.days.unwrap_or(90).max(1);
    match crate::reflection::service::calibration_summary(&gw.store.pool, lookback).await {
        Ok(summary) => (StatusCode::OK, Json(summary)).into_response(),
        Err(e) => {
            warn!(error = %e, "get_calibration failed");
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
    }
}

async fn get_ticker_context(
    State(gw): State<Arc<Gateway>>,
    Query(q): Query<ThesesQuery>,
) -> impl IntoResponse {
    let Some(sym) = q.symbol else {
        return (StatusCode::BAD_REQUEST, "symbol query param required").into_response();
    };
    match gw.store.latest_ticker_context(&sym).await {
        Ok(Some(row)) => (StatusCode::OK, Json(row)).into_response(),
        Ok(None) => (StatusCode::NO_CONTENT).into_response(),
        Err(e) => {
            warn!(symbol = %sym, error = %e, "get_ticker_context failed");
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
    }
}

#[derive(Debug, Deserialize, Default)]
struct CandlesQuery {
    symbol: Option<String>,
    /// 1D / 5D / 1M / 3M / 6M / 200D / 1Y / 2Y / ALL.
    #[serde(default)]
    range: Option<String>,
    /// 1m / 3m / 5m / 15m / 30m / 1h / 2h / 4h / 1D / 1W / 3W / 1M.
    #[serde(default)]
    interval: Option<String>,
}

#[derive(Debug, Clone, Copy)]
struct ChartInterval {
    label: &'static str,
    native: Option<&'static str>,
    bucket_minutes: i64,
}

impl ChartInterval {
    fn parse(raw: Option<&str>) -> Self {
        match raw.unwrap_or("1D") {
            "1m" => Self {
                label: "1m",
                native: Some("1min"),
                bucket_minutes: 1,
            },
            "3m" => Self {
                label: "3m",
                native: Some("1min"),
                bucket_minutes: 3,
            },
            "5m" => Self {
                label: "5m",
                native: Some("5min"),
                bucket_minutes: 5,
            },
            "15m" => Self {
                label: "15m",
                native: Some("15min"),
                bucket_minutes: 15,
            },
            "30m" => Self {
                label: "30m",
                native: Some("30min"),
                bucket_minutes: 30,
            },
            "1h" => Self {
                label: "1h",
                native: Some("1hour"),
                bucket_minutes: 60,
            },
            "2h" => Self {
                label: "2h",
                native: Some("1hour"),
                bucket_minutes: 120,
            },
            "4h" => Self {
                label: "4h",
                native: Some("4hour"),
                bucket_minutes: 240,
            },
            "1W" => Self {
                label: "1W",
                native: None,
                bucket_minutes: 0,
            },
            "3W" => Self {
                label: "3W",
                native: None,
                bucket_minutes: 0,
            },
            "1M" => Self {
                label: "1M",
                native: None,
                bucket_minutes: 0,
            },
            _ => Self {
                label: "1D",
                native: None,
                bucket_minutes: 0,
            },
        }
    }

    fn is_intraday(self) -> bool {
        self.native.is_some()
    }
}

fn chart_lookback_days(range: Option<&str>, interval: ChartInterval) -> i64 {
    match range.unwrap_or(if interval.is_intraday() { "5D" } else { "1Y" }) {
        "1D" => 2,
        "5D" => 7,
        "1M" => 35,
        "3M" => 100,
        "6M" => 200,
        "200D" => 320,
        "1Y" => 380,
        "2Y" => 760,
        "ALL" => 365 * 30,
        _ => {
            if interval.is_intraday() {
                7
            } else {
                380
            }
        }
    }
}

const MAX_INTRADAY_FETCHES_PER_CHART_REQUEST: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct IntradayFetchWindow {
    from: NaiveDate,
    to: NaiveDate,
}

fn intraday_chunk_days(native_interval: &str) -> i64 {
    match native_interval {
        "1min" => 7,
        "5min" => 14,
        "15min" => 45,
        "30min" => 45,
        "1hour" => 120,
        "4hour" => 365,
        _ => 45,
    }
}

fn intraday_history_windows(
    target_from: NaiveDate,
    oldest: NaiveDate,
    chunk_days: i64,
    max_windows: usize,
) -> Vec<IntradayFetchWindow> {
    if oldest <= target_from || max_windows == 0 {
        return Vec::new();
    }
    let mut windows = Vec::new();
    let mut to = oldest - chrono::Duration::days(1);
    while to >= target_from && windows.len() < max_windows {
        let from = std::cmp::max(target_from, to - chrono::Duration::days(chunk_days - 1));
        windows.push(IntradayFetchWindow { from, to });
        if from <= target_from {
            break;
        }
        to = from - chrono::Duration::days(1);
    }
    windows
}

async fn fetch_intraday_window(
    gw: &Gateway,
    symbol: &str,
    native_interval: &str,
    window: IntradayFetchWindow,
) -> AnyResult<usize> {
    let rows = gw
        .fmp_intraday
        .fetch_range(symbol, native_interval, window.from, window.to)
        .await?;
    let count = rows.len();
    if count > 0 {
        gw.store.upsert_intraday_price_bars(&rows).await?;
    }
    Ok(count)
}

async fn ensure_intraday_chart_coverage(
    gw: &Gateway,
    symbol: &str,
    native_interval: &str,
    lookback_days: i64,
) -> (usize, Option<String>) {
    let today = chrono::Utc::now().date_naive();
    let target_from = today - chrono::Duration::days(lookback_days);
    let chunk_days = intraday_chunk_days(native_interval);
    let mut attempts = 0usize;
    let mut fetch_error = None;

    let coverage = match gw
        .store
        .intraday_bar_coverage(symbol, native_interval)
        .await
    {
        Ok(coverage) => coverage,
        Err(e) => {
            warn!(symbol = %symbol, interval = native_interval, error = %e, "intraday coverage lookup failed");
            return (attempts, Some(e.to_string()));
        }
    };
    let latest_stale = coverage
        .latest
        .map(|ts| chrono::Utc::now() - ts > chrono::Duration::minutes(30))
        .unwrap_or(true);
    if latest_stale && attempts < MAX_INTRADAY_FETCHES_PER_CHART_REQUEST {
        let from = std::cmp::max(target_from, today - chrono::Duration::days(chunk_days - 1));
        let window = IntradayFetchWindow { from, to: today };
        attempts += 1;
        if let Err(e) = fetch_intraday_window(gw, symbol, native_interval, window).await {
            warn!(symbol = %symbol, interval = native_interval, from = %window.from, to = %window.to, error = %e, "fetch latest intraday chart window failed");
            fetch_error = Some(e.to_string());
        }
    }

    let coverage = match gw
        .store
        .intraday_bar_coverage(symbol, native_interval)
        .await
    {
        Ok(coverage) => coverage,
        Err(e) => {
            warn!(symbol = %symbol, interval = native_interval, error = %e, "intraday coverage refresh lookup failed");
            return (attempts, fetch_error.or_else(|| Some(e.to_string())));
        }
    };
    let Some(oldest) = coverage.oldest.map(|ts| ts.date_naive()) else {
        return (attempts, fetch_error);
    };
    let remaining = MAX_INTRADAY_FETCHES_PER_CHART_REQUEST.saturating_sub(attempts);
    for window in intraday_history_windows(target_from, oldest, chunk_days, remaining) {
        attempts += 1;
        if let Err(e) = fetch_intraday_window(gw, symbol, native_interval, window).await {
            warn!(symbol = %symbol, interval = native_interval, from = %window.from, to = %window.to, error = %e, "fetch older intraday chart window failed");
            fetch_error = Some(e.to_string());
            break;
        }
    }

    (attempts, fetch_error)
}

fn candle_rows_response(
    rows: Vec<serde_json::Value>,
    fetch_attempts: usize,
    fetch_error: Option<String>,
) -> Response {
    let first = rows
        .first()
        .and_then(|r| r.get("time"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let last = rows
        .last()
        .and_then(|r| r.get("time"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let bars = rows.len().to_string();
    let mut resp = (StatusCode::OK, Json(rows)).into_response();
    insert_header(&mut resp, "x-chart-bars", &bars);
    insert_header(
        &mut resp,
        "x-chart-fetch-attempts",
        &fetch_attempts.to_string(),
    );
    if let Some(first) = first {
        insert_header(&mut resp, "x-chart-coverage-start", &first);
    }
    if let Some(last) = last {
        insert_header(&mut resp, "x-chart-coverage-end", &last);
    }
    if let Some(error) = fetch_error {
        insert_header(&mut resp, "x-chart-fetch-error", &error);
    }
    resp
}

fn insert_header(resp: &mut Response, name: &'static str, value: &str) {
    if let Ok(value) = HeaderValue::from_str(value) {
        resp.headers_mut()
            .insert(header::HeaderName::from_static(name), value);
    }
}

async fn get_candles(
    State(gw): State<Arc<Gateway>>,
    Query(q): Query<CandlesQuery>,
) -> impl IntoResponse {
    let Some(sym) = q.symbol else {
        return (StatusCode::BAD_REQUEST, "symbol query param required").into_response();
    };
    let interval = ChartInterval::parse(q.interval.as_deref());
    let lookback_days = chart_lookback_days(q.range.as_deref(), interval);
    if let Some(native) = interval.native {
        if !gw.fmp_intraday.configured() {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "FMP_API_KEY required for intraday chart bars",
            )
                .into_response();
        }
        let (fetch_attempts, fetch_error) =
            ensure_intraday_chart_coverage(&gw, &sym, native, lookback_days).await;
        match gw
            .store
            .intraday_candles_for(&sym, native, lookback_days, interval.bucket_minutes)
            .await
        {
            Ok(rows) => {
                if rows.is_empty() {
                    if let Some(e) = fetch_error.clone() {
                        return (
                            StatusCode::BAD_GATEWAY,
                            format!(
                                "{intv} bars unavailable from FMP for {sym}: {e}",
                                intv = interval.label,
                            ),
                        )
                            .into_response();
                    }
                }
                return candle_rows_response(rows, fetch_attempts, fetch_error);
            }
            Err(e) => {
                warn!(symbol = %sym, interval = native, error = %e, "get intraday candles failed");
                return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
            }
        }
    }
    match gw
        .store
        .candles_for(&sym, lookback_days, interval.label)
        .await
    {
        Ok(rows) => candle_rows_response(rows, 0, None),
        Err(e) => {
            warn!(symbol = %sym, error = %e, "get_candles failed");
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
    }
}

async fn get_technical_state(
    State(gw): State<Arc<Gateway>>,
    Query(q): Query<ThesesQuery>,
) -> impl IntoResponse {
    let Some(sym) = q.symbol else {
        return (StatusCode::BAD_REQUEST, "symbol query param required").into_response();
    };
    match technical_state_for(&gw, &sym).await {
        Ok(state) => (StatusCode::OK, Json(state)).into_response(),
        Err(e) => {
            warn!(symbol = %sym, error = %e, "get_technical_state failed");
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
    }
}

async fn technical_state_for(gw: &Gateway, symbol: &str) -> AnyResult<TechnicalState> {
    let daily = gw
        .store
        .daily_technical_bars_for(symbol, 365 * 30)
        .await
        .context("daily technical bars")?;
    let intraday_specs = [
        ("30m", "30min", 90, 30),
        ("2h", "1hour", 180, 120),
        ("4h", "4hour", 365, 240),
    ];
    let mut intraday = Vec::new();
    for (label, native_interval, lookback_days, bucket_minutes) in intraday_specs {
        maybe_backfill_intraday(gw, symbol, native_interval, lookback_days).await;
        match gw
            .store
            .intraday_technical_bars_for(symbol, native_interval, lookback_days, bucket_minutes)
            .await
        {
            Ok(rows) => intraday.push((label, rows)),
            Err(e) => {
                warn!(symbol = %symbol, interval = native_interval, error = %e, "get intraday technical bars failed");
                intraday.push((label, Vec::new()));
            }
        }
    }
    Ok(build_technical_state(symbol, &daily, &intraday))
}

async fn maybe_backfill_intraday(
    gw: &Gateway,
    symbol: &str,
    native_interval: &str,
    lookback_days: i64,
) {
    if !gw.fmp_intraday.configured() {
        return;
    }
    let latest = match gw
        .store
        .latest_intraday_bar_ts(symbol, native_interval)
        .await
    {
        Ok(latest) => latest,
        Err(e) => {
            warn!(symbol = %symbol, interval = native_interval, error = %e, "latest intraday technical bar lookup failed");
            return;
        }
    };
    let stale = latest
        .map(|ts| chrono::Utc::now() - ts > chrono::Duration::minutes(30))
        .unwrap_or(true);
    if !stale {
        return;
    }
    match gw
        .fmp_intraday
        .fetch_one(symbol, native_interval, lookback_days)
        .await
    {
        Ok(rows) => {
            if let Err(e) = gw.store.upsert_intraday_price_bars(&rows).await {
                warn!(symbol = %symbol, interval = native_interval, error = %e, "persist intraday technical bars failed");
            }
        }
        Err(e) => {
            warn!(symbol = %symbol, interval = native_interval, error = %e, "fetch intraday technical bars failed");
        }
    }
}

#[derive(Debug, Deserialize)]
struct ChatAnalystRequest {
    question: String,
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    scope: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChatEvidenceRef {
    source: String,
    #[serde(default)]
    evidence_id: Option<i64>,
    summary: String,
    #[serde(default)]
    observed_at: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ChatTechnicalRead {
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    timing_implication: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChatThesisImpact {
    kind: String,
    #[serde(default)]
    reason: Option<String>,
}

impl Default for ChatThesisImpact {
    fn default() -> Self {
        Self {
            kind: "no_change".to_string(),
            reason: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RequestedEvidence {
    requirement_key: String,
    source_type: String,
    priority: String,
    reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChatAttentionRequest {
    kind: String,
    #[serde(default)]
    reason: Option<String>,
}

impl Default for ChatAttentionRequest {
    fn default() -> Self {
        Self {
            kind: "none".to_string(),
            reason: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChatAnalystAnswer {
    answer: String,
    confidence: String,
    #[serde(default)]
    evidence_used: Vec<ChatEvidenceRef>,
    #[serde(default)]
    technical_read: ChatTechnicalRead,
    #[serde(default)]
    thesis_impact: ChatThesisImpact,
    #[serde(default)]
    requested_evidence: Vec<RequestedEvidence>,
    #[serde(default)]
    attention_request: ChatAttentionRequest,
}

#[derive(Debug, Serialize)]
struct ChatAnalystEnvelope {
    scope: String,
    symbol: Option<String>,
    answer: ChatAnalystAnswer,
    queued_evidence: usize,
    used_fallback: bool,
    fallback_reason: Option<String>,
}

async fn post_chat_analyst(
    State(gw): State<Arc<Gateway>>,
    Json(req): Json<ChatAnalystRequest>,
) -> impl IntoResponse {
    let question = req.question.trim();
    if question.is_empty() {
        return (StatusCode::BAD_REQUEST, "question required").into_response();
    }
    if question.chars().count() > 4_000 {
        return (StatusCode::BAD_REQUEST, "question too long").into_response();
    }
    let symbol = req.symbol.as_deref().and_then(normalize_chat_symbol);
    let scope = normalize_chat_scope(req.scope.as_deref(), question, symbol.as_deref());
    if matches!(scope.as_str(), "symbol" | "technical" | "decision") && symbol.is_none() {
        return (
            StatusCode::BAD_REQUEST,
            "symbol required for symbol, technical, and decision questions",
        )
            .into_response();
    }

    let package = match build_chat_evidence_package(&gw, question, &scope, symbol.as_deref()).await
    {
        Ok(package) => package,
        Err(e) => {
            warn!(symbol = ?symbol, scope = %scope, error = %e, "build chat evidence package failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }
    };
    let (mut answer, fallback_reason) = match invoke_chat_analyst(
        &gw,
        question,
        &scope,
        symbol.as_deref(),
        &package,
    )
    .await
    {
        Ok(answer) => answer,
        Err(e) => {
            warn!(symbol = ?symbol, scope = %scope, error = %e, "chat analyst invocation failed");
            return (StatusCode::BAD_GATEWAY, e.to_string()).into_response();
        }
    };
    answer.requested_evidence = answer
        .requested_evidence
        .iter()
        .map(canonical_requested_evidence)
        .collect();
    if answer.requested_evidence.is_empty() && fallback_should_request_evidence(question, &package)
    {
        answer
            .requested_evidence
            .push(default_product_research_request(
                question,
                symbol.as_deref(),
            ));
    }
    let queued_evidence = if let Some(symbol) = symbol.as_deref() {
        match queue_requested_evidence(&gw, symbol, &answer.requested_evidence, question).await {
            Ok(n) => n,
            Err(e) => {
                warn!(symbol = %symbol, error = %e, "queue requested chat evidence failed");
                return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
            }
        }
    } else {
        0
    };

    (
        StatusCode::OK,
        Json(ChatAnalystEnvelope {
            scope,
            symbol,
            answer,
            queued_evidence,
            used_fallback: fallback_reason.is_some(),
            fallback_reason,
        }),
    )
        .into_response()
}

async fn build_chat_evidence_package(
    gw: &Gateway,
    question: &str,
    scope: &str,
    symbol: Option<&str>,
) -> AnyResult<serde_json::Value> {
    let brain_theses = relevant_brain_theses(gw, symbol).await?;
    let mut ticker_context = serde_json::Value::Null;
    let mut technical_state = serde_json::Value::Null;
    let mut current_thesis = serde_json::Value::Null;
    let mut thesis_history = json!([]);
    let mut evidence_items = json!([]);
    let mut evidence_requirements = json!([]);
    let mut research_evidence = json!([]);
    let mut decisions = json!([]);
    let mut positions = json!([]);

    if let Some(symbol) = symbol {
        ticker_context = serde_json::to_value(gw.store.latest_ticker_context(symbol).await?)
            .context("encode ticker_context")?;
        technical_state =
            serde_json::to_value(technical_state_for(gw, symbol).await?).context("technical")?;
        let theses = gw.store.theses_for_symbol(symbol).await?;
        current_thesis = serde_json::to_value(
            theses
                .iter()
                .find(|t| !["closed", "disqualified"].contains(&t.state.as_str()))
                .or_else(|| theses.first()),
        )
        .context("encode current_thesis")?;
        thesis_history = serde_json::to_value(theses).context("encode thesis_history")?;
        evidence_items = serde_json::to_value(gw.store.evidence_items_for_symbol(symbol).await?)
            .context("encode evidence_items")?;
        evidence_requirements =
            serde_json::to_value(gw.store.evidence_requirements_for_symbol(symbol).await?)
                .context("encode evidence_requirements")?;
        research_evidence =
            serde_json::to_value(gw.store.research_evidence_for_symbol(symbol).await?)
                .context("encode research_evidence")?;
        decisions = serde_json::to_value(gw.store.decisions_for_symbol(symbol).await?)
            .context("encode decisions")?;
        positions =
            serde_json::to_value(positions_for_symbol(gw, symbol).await?).context("positions")?;
    }

    Ok(json!({
        "question": question,
        "scope": scope,
        "symbol": symbol,
        "brain_theses": brain_theses,
        "ticker_context": ticker_context,
        "technical_state": technical_state,
        "current_thesis": current_thesis,
        "thesis_history": thesis_history,
        "evidence_items": evidence_items,
        "evidence_requirements": evidence_requirements,
        "research_evidence": research_evidence,
        "decisions": decisions,
        "positions": positions,
    }))
}

async fn invoke_chat_analyst(
    gw: &Gateway,
    question: &str,
    scope: &str,
    symbol: Option<&str>,
    package: &serde_json::Value,
) -> AnyResult<(ChatAnalystAnswer, Option<String>)> {
    let prompt = gw
        .prompts
        .get("chat-analyst")
        .ok_or_else(|| anyhow::anyhow!("chat-analyst prompt not loaded"))?;
    let mut vars = HashMap::new();
    vars.insert("today", chrono::Utc::now().date_naive().to_string());
    vars.insert("scope", scope.to_string());
    vars.insert("symbol", symbol.unwrap_or("").to_string());
    let user_message = serde_json::to_string_pretty(package).context("encode chat package")?;
    let response = match tokio::time::timeout(
        Duration::from_secs(45),
        prompts::invoke(
            gw.llm.as_ref(),
            Some(gw.store.as_ref() as &dyn prompts::InvocationRecorder),
            prompt,
            &vars,
            &user_message,
            &gw.llm_provider_name,
            Some(&gw.llm_model),
        ),
    )
    .await
    {
        Ok(resp) => resp?,
        Err(_) => {
            record_chat_timeout_invocation(gw, prompt, &vars, &user_message).await?;
            return Ok((
                fallback_chat_answer(question, scope, symbol, package),
                Some("provider timeout".to_string()),
            ));
        }
    };
    match parse_chat_analyst_answer(&response.content) {
        Ok(answer) => Ok((answer, None)),
        Err(_e) if is_mock_response(&response.content) => Ok((
            fallback_chat_answer(question, scope, symbol, package),
            Some("mock provider".to_string()),
        )),
        Err(e) => Err(e),
    }
}

async fn record_chat_timeout_invocation(
    gw: &Gateway,
    prompt: &prompts::Prompt,
    vars: &HashMap<&str, String>,
    user_message: &str,
) -> AnyResult<()> {
    let request_summary =
        chat_summary(&format!("{}\n\n{}", prompt.render(vars), user_message), 200);
    gw.store
        .record_llm_invocation(
            &prompt.name,
            &prompt.hash,
            &gw.llm_provider_name,
            &gw.llm_model,
            0,
            0,
            45_000,
            &request_summary,
            "provider timed out; deterministic chat analyst fallback returned",
        )
        .await
        .context("record chat analyst timeout invocation")
}

fn chat_summary(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        value.to_string()
    } else {
        format!("{}...", value.chars().take(max_chars).collect::<String>())
    }
}

fn parse_chat_analyst_answer(content: &str) -> AnyResult<ChatAnalystAnswer> {
    let raw = prompts::extract_json(content);
    serde_json::from_str::<ChatAnalystAnswer>(raw).context("parse chat analyst JSON")
}

fn is_mock_response(content: &str) -> bool {
    content.trim() == r#"{"mock":true}"#
}

fn fallback_chat_answer(
    question: &str,
    scope: &str,
    symbol: Option<&str>,
    package: &serde_json::Value,
) -> ChatAnalystAnswer {
    let technical = package
        .get("technical_state")
        .unwrap_or(&serde_json::Value::Null);
    let state = technical
        .get("state")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    let summary = technical
        .get("summary")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let mut evidence_used = Vec::new();
    if let Some(items) = package
        .get("evidence_items")
        .and_then(serde_json::Value::as_array)
    {
        for item in items.iter().take(5) {
            evidence_used.push(ChatEvidenceRef {
                source: item
                    .get("source")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("evidence_item")
                    .to_string(),
                evidence_id: item.get("id").and_then(serde_json::Value::as_i64),
                summary: item
                    .get("summary")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("evidence fact")
                    .to_string(),
                observed_at: item
                    .get("observed_at")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
            });
        }
    }
    let requested_evidence = if fallback_should_request_evidence(question, package) {
        vec![default_product_research_request(question, symbol)]
    } else {
        Vec::new()
    };
    let symbol_text = symbol.unwrap_or("the selected scope");
    let answer = if let Some(summary) = summary.as_deref() {
        format!(
            "{symbol_text}: {summary}. This is timing context, not a thesis mutation. Use the cited evidence and requested evidence list to decide what to refresh before acting."
        )
    } else {
        format!(
            "{symbol_text}: I can answer only from the loaded evidence package. No technical summary was available, so missing evidence should be queued before drawing a stronger conclusion."
        )
    };
    ChatAnalystAnswer {
        answer,
        confidence: if evidence_used.is_empty() {
            "low".to_string()
        } else {
            "medium".to_string()
        },
        evidence_used,
        technical_read: ChatTechnicalRead {
            state: Some(state.to_string()),
            summary,
            timing_implication: Some(
                if state == "extended" {
                    "standing thesis may remain intact, but timing quality is lower until extension cools or a fresh trigger appears"
                } else {
                    "treat chart state as timing context separate from thesis direction"
                }
                .to_string(),
            ),
        },
        thesis_impact: ChatThesisImpact {
            kind: if scope == "technical" && state == "extended" {
                "weakens".to_string()
            } else {
                "no_change".to_string()
            },
            reason: Some("Deterministic fallback used loaded evidence only; no thesis was mutated.".to_string()),
        },
        requested_evidence,
        attention_request: ChatAttentionRequest::default(),
    }
}

fn fallback_should_request_evidence(question: &str, package: &serde_json::Value) -> bool {
    let q = question.to_ascii_lowercase();
    let asks_for_research = ["missing", "search", "source", "evidence", "article", "news"]
        .iter()
        .any(|needle| q.contains(needle));
    let no_evidence = package
        .get("evidence_items")
        .and_then(serde_json::Value::as_array)
        .is_none_or(Vec::is_empty);
    asks_for_research || no_evidence
}

fn default_product_research_request(question: &str, symbol: Option<&str>) -> RequestedEvidence {
    RequestedEvidence {
        requirement_key: "product_research".to_string(),
        source_type: "web_research".to_string(),
        priority: "high".to_string(),
        reason: format!(
            "Operator analyst question needs fresh public research for {}: {}",
            symbol.unwrap_or("current scope"),
            question
        ),
    }
}

fn normalize_chat_symbol(raw: &str) -> Option<String> {
    let symbol = raw.trim().to_ascii_uppercase();
    let valid = !symbol.is_empty()
        && symbol.len() <= 10
        && symbol
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '.' || c == '-');
    valid.then_some(symbol)
}

fn normalize_chat_scope(raw: Option<&str>, question: &str, symbol: Option<&str>) -> String {
    if let Some(scope) = raw {
        let scope = scope.trim().to_ascii_lowercase();
        if matches!(
            scope.as_str(),
            "symbol" | "theme" | "macro" | "technical" | "decision"
        ) {
            return scope;
        }
    }
    classify_chat_scope(question, symbol)
}

fn classify_chat_scope(question: &str, symbol: Option<&str>) -> String {
    let q = question.to_ascii_lowercase();
    if [
        "rsi",
        "sma",
        "chart",
        "technical",
        "entry",
        "timing",
        "200-day",
        "200 day",
    ]
    .iter()
    .any(|needle| q.contains(needle))
    {
        "technical".to_string()
    } else if ["decision", "position", "enter", "exit", "trade", "size"]
        .iter()
        .any(|needle| q.contains(needle))
    {
        "decision".to_string()
    } else if ["macro", "rates", "credit", "liquidity", "market regime"]
        .iter()
        .any(|needle| q.contains(needle))
    {
        "macro".to_string()
    } else if ["sector", "theme", "metals", "copper", "wheat"]
        .iter()
        .any(|needle| q.contains(needle))
    {
        "theme".to_string()
    } else if symbol.is_some() {
        "symbol".to_string()
    } else {
        "macro".to_string()
    }
}

async fn relevant_brain_theses(
    gw: &Gateway,
    symbol: Option<&str>,
) -> AnyResult<Vec<serde_json::Value>> {
    let rows = sqlx::query(
        r#"SELECT bt.id, bt.scope, bt.key, bt.name, bt.state, bt.direction,
                  bt.summary, bt.core_claim, bt.why_now, bt.evidence,
                  bt.invalidation_conditions, bt.open_questions, bt.missing_evidence,
                  bt.last_evaluated_at, bt.version, btt.symbol AS linked_symbol,
                  btt.role, btt.rationale, btt.conviction
             FROM brain_thesis bt
        LEFT JOIN brain_thesis_ticker btt
               ON btt.brain_thesis_id = bt.id
              AND btt.symbol = $1
            WHERE bt.active = true
              AND (bt.scope = 'macro' OR ($1::text IS NOT NULL AND btt.symbol = $1))
         ORDER BY CASE bt.scope WHEN 'macro' THEN 0 WHEN 'sector' THEN 1 ELSE 2 END,
                  bt.name
            LIMIT 12"#,
    )
    .bind(symbol)
    .fetch_all(&gw.store.pool)
    .await
    .context("relevant_brain_theses")?;
    rows.into_iter()
        .map(|r| {
            let id: Option<uuid::Uuid> = r.try_get("id")?;
            let last_evaluated_at: Option<chrono::DateTime<chrono::Utc>> =
                r.try_get("last_evaluated_at")?;
            Ok::<serde_json::Value, sqlx::Error>(json!({
                "id": id,
                "scope": r.try_get::<String, _>("scope")?,
                "key": r.try_get::<String, _>("key")?,
                "name": r.try_get::<String, _>("name")?,
                "state": r.try_get::<String, _>("state")?,
                "direction": r.try_get::<String, _>("direction")?,
                "summary": r.try_get::<String, _>("summary")?,
                "core_claim": r.try_get::<String, _>("core_claim")?,
                "why_now": r.try_get::<Option<String>, _>("why_now")?,
                "evidence": r.try_get::<serde_json::Value, _>("evidence")?,
                "invalidation_conditions": r.try_get::<serde_json::Value, _>("invalidation_conditions")?,
                "open_questions": r.try_get::<serde_json::Value, _>("open_questions")?,
                "missing_evidence": r.try_get::<serde_json::Value, _>("missing_evidence")?,
                "last_evaluated_at": last_evaluated_at,
                "version": r.try_get::<i32, _>("version")?,
                "linked_symbol": r.try_get::<Option<String>, _>("linked_symbol")?,
                "role": r.try_get::<Option<String>, _>("role")?,
                "rationale": r.try_get::<Option<String>, _>("rationale")?,
                "conviction": r.try_get::<Option<f64>, _>("conviction")?,
            }))
        })
        .collect::<Result<_, _>>()
        .context("encode relevant_brain_theses")
}

fn canonical_requested_evidence(req: &RequestedEvidence) -> RequestedEvidence {
    let key = match req.requirement_key.trim() {
        "price_history" | "company_profile" | "company_facts" | "earnings_calendar"
        | "recent_news" | "analyst_estimates" | "analyst_opinion" | "product_research" => {
            req.requirement_key.trim()
        }
        _ => "product_research",
    };
    let priority = match req.priority.trim() {
        "blocking" | "high" | "medium" | "low" => req.priority.trim(),
        _ => "medium",
    };
    RequestedEvidence {
        requirement_key: key.to_string(),
        source_type: source_type_for_requirement(key).to_string(),
        priority: priority.to_string(),
        reason: if req.reason.trim().is_empty() {
            format!("Chat analyst requested {key}")
        } else {
            req.reason.trim().chars().take(500).collect()
        },
    }
}

fn source_type_for_requirement(requirement_key: &str) -> &'static str {
    match requirement_key {
        "price_history" => "price",
        "company_profile" => "profile",
        "company_facts" => "fundamentals",
        "earnings_calendar" => "catalysts",
        "recent_news" => "news",
        "analyst_estimates" => "estimates",
        "analyst_opinion" => "analyst_opinion",
        "product_research" => "web_research",
        _ => "web_research",
    }
}

fn actions_for_requirement(requirement_key: &str) -> &'static [&'static str] {
    match requirement_key {
        "price_history" => &["fmp_price_backfill"],
        "company_facts" => &["sec_company_tickers_cik_lookup", "sec_companyfacts_xbrl"],
        "recent_news" => &["fmp_news", "massive_news", "llm_sentiment_scoring"],
        "analyst_estimates" => &["fmp_analyst_estimates"],
        "analyst_opinion" => &[
            "fmp_price_target_consensus",
            "fmp_grades_historical",
            "fmp_price_target_news",
            "fmp_grades_latest_news",
        ],
        "company_profile" => &["fmp_company_profile"],
        "earnings_calendar" => &["fmp_earnings_calendar"],
        "product_research" => &["gdelt_doc_search", "bing_news_rss_search"],
        _ => &["gdelt_doc_search", "bing_news_rss_search"],
    }
}

fn provider_for_action(action: &str, source_type: &str) -> &'static str {
    if action.starts_with("fmp_") {
        "fmp"
    } else if action.starts_with("massive_") {
        "massive"
    } else if action.starts_with("sec_") {
        "sec"
    } else if action.starts_with("gdelt_") {
        "gdelt"
    } else if action.starts_with("bing_") {
        "bing"
    } else if action.starts_with("llm_") {
        "llm"
    } else {
        match source_type {
            "web_research" => "gdelt",
            _ => "system",
        }
    }
}

async fn queue_requested_evidence(
    gw: &Gateway,
    symbol: &str,
    requested: &[RequestedEvidence],
    question: &str,
) -> AnyResult<usize> {
    if requested.is_empty() {
        return Ok(0);
    }
    let mut tx = gw
        .store
        .pool
        .begin()
        .await
        .context("begin chat evidence tx")?;
    let mut queued = 0usize;
    for raw in requested {
        let req = canonical_requested_evidence(raw);
        let actions = actions_for_requirement(&req.requirement_key);
        let actions_vec = actions.iter().map(|a| (*a).to_string()).collect::<Vec<_>>();
        let source_ref = json!({
            "requested_by": "chat_analyst",
            "question": question,
            "fetch_actions": actions_vec,
        });
        let blocking_state: String = sqlx::query_scalar(
            r#"INSERT INTO evidence_requirement
                    (symbol, requirement_key, source_type, reason, priority,
                     blocking_state, next_retry_at, source_ref)
               VALUES ($1, $2, $3, $4, $5, 'missing', now(), $6)
               ON CONFLICT (symbol, requirement_key) DO UPDATE SET
                    source_type = EXCLUDED.source_type,
                    reason = EXCLUDED.reason,
                    priority = EXCLUDED.priority,
                    blocking_state = CASE
                        WHEN evidence_requirement.blocking_state = 'satisfied'
                        THEN evidence_requirement.blocking_state
                        ELSE 'missing'
                    END,
                    next_retry_at = CASE
                        WHEN evidence_requirement.blocking_state = 'satisfied'
                        THEN evidence_requirement.next_retry_at
                        ELSE now()
                    END,
                    source_ref = evidence_requirement.source_ref || EXCLUDED.source_ref,
                    updated_at = now(),
                    satisfied_at = CASE
                        WHEN evidence_requirement.blocking_state = 'satisfied'
                        THEN evidence_requirement.satisfied_at
                        ELSE NULL
                    END
             RETURNING blocking_state"#,
        )
        .bind(symbol)
        .bind(&req.requirement_key)
        .bind(&req.source_type)
        .bind(&req.reason)
        .bind(&req.priority)
        .bind(&source_ref)
        .fetch_one(&mut *tx)
        .await
        .context("upsert chat evidence_requirement")?;
        if blocking_state == "satisfied" {
            continue;
        }
        for action in actions {
            let provider = provider_for_action(action, &req.source_type);
            let task_ref = json!({
                "requested_by": "chat_analyst",
                "question": question,
                "reason": req.reason,
            });
            let res = sqlx::query(
                r#"INSERT INTO source_task
                        (source_type, requirement_key, action, scope, target_id,
                         provider, limiter_key, state, priority, due_at, source_ref)
                   VALUES ($1, $2, $3, 'symbol', $4, $5, $5, 'queued', $6, now(), $7)
                   ON CONFLICT (scope, target_id, requirement_key, action) DO UPDATE SET
                        source_type = EXCLUDED.source_type,
                        provider = EXCLUDED.provider,
                        limiter_key = EXCLUDED.limiter_key,
                        state = CASE
                            WHEN source_task.state = 'fetching' THEN source_task.state
                            ELSE 'queued'
                        END,
                        priority = EXCLUDED.priority,
                        due_at = CASE
                            WHEN source_task.state = 'fetching' THEN source_task.due_at
                            ELSE now()
                        END,
                        next_retry_at = NULL,
                        last_error = NULL,
                        source_ref = source_task.source_ref || EXCLUDED.source_ref,
                        updated_at = now()"#,
            )
            .bind(&req.source_type)
            .bind(&req.requirement_key)
            .bind(action)
            .bind(symbol)
            .bind(provider)
            .bind(&req.priority)
            .bind(&task_ref)
            .execute(&mut *tx)
            .await
            .context("upsert chat source_task")?;
            queued += usize::try_from(res.rows_affected()).unwrap_or(0);
        }
    }
    tx.commit().await.context("commit chat evidence tx")?;
    Ok(queued)
}

async fn get_decisions(
    State(gw): State<Arc<Gateway>>,
    Query(q): Query<ThesesQuery>,
) -> impl IntoResponse {
    let Some(sym) = q.symbol else {
        return (StatusCode::BAD_REQUEST, "symbol query param required").into_response();
    };
    match gw.store.decisions_for_symbol(&sym).await {
        Ok(rows) => (StatusCode::OK, Json(rows)).into_response(),
        Err(e) => {
            warn!(symbol = %sym, error = %e, "get_decisions failed");
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
    }
}

async fn get_decision_replay(
    State(gw): State<Arc<Gateway>>,
    Path(id): Path<uuid::Uuid>,
) -> impl IntoResponse {
    match gw.store.decision_replay(id).await {
        Ok(Some(row)) => (StatusCode::OK, Json(row)).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, "decision replay not found").into_response(),
        Err(e) => {
            warn!(decision_id = %id, error = %e, "get_decision_replay failed");
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
    }
}

async fn get_positions(
    State(gw): State<Arc<Gateway>>,
    Query(q): Query<ThesesQuery>,
) -> impl IntoResponse {
    let Some(sym) = q.symbol else {
        return (StatusCode::BAD_REQUEST, "symbol query param required").into_response();
    };
    match positions_for_symbol(&gw, &sym).await {
        Ok(body) => (StatusCode::OK, Json(body)).into_response(),
        Err(e) => {
            warn!(symbol = %sym, error = %e, "get_positions failed");
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
    }
}

async fn positions_for_symbol(gw: &Gateway, sym: &str) -> AnyResult<Vec<serde_json::Value>> {
    let symbol = sym.trim().to_ascii_uppercase();
    let rows = sqlx::query(
        r#"SELECT p.position_id, p.thesis_id, p.symbol, COALESCE(p.side, 'long') AS side,
                  p.instrument,
                  p.qty::float8 AS qty,
                  p.avg_price::float8 AS avg_price,
                  COALESCE(p.delta_notional, 0)::float8 AS delta_notional,
                  COALESCE(p.premium_at_risk, 0)::float8 AS premium_at_risk,
                  p.opened_at, p.closed_at,
                  p.realized_pnl::float8 AS realized_pnl,
                  t.state AS thesis_state,
                  t.forecast->>'direction' AS thesis_direction,
                  latest.close AS latest_price,
                  latest.ts AS latest_price_at,
                  COALESCE(fills.fill_count, 0) AS fill_count,
                  CASE
                    WHEN p.closed_at IS NOT NULL OR latest.close IS NULL THEN NULL
                    WHEN COALESCE(p.side, 'long') = 'short' THEN
                      (p.avg_price::float8 - latest.close) * p.qty::float8 *
                      CASE WHEN p.instrument IN ('leaps','options') THEN 100.0 ELSE 1.0 END
                    ELSE
                      (latest.close - p.avg_price::float8) * p.qty::float8 *
                      CASE WHEN p.instrument IN ('leaps','options') THEN 100.0 ELSE 1.0 END
                  END AS unrealized_pnl
             FROM position p
             LEFT JOIN thesis t ON t.thesis_id = p.thesis_id
             LEFT JOIN LATERAL (
                   SELECT close::float8 AS close, ts
                     FROM price_bar
                    WHERE symbol = p.symbol
                 ORDER BY ts DESC
                    LIMIT 1
             ) latest ON TRUE
             LEFT JOIN LATERAL (
                   SELECT COUNT(*)::int AS fill_count
                     FROM position_fill pf
                    WHERE pf.position_id = p.position_id
             ) fills ON TRUE
            WHERE p.symbol = $1
         ORDER BY p.closed_at IS NULL DESC, p.opened_at DESC"#,
    )
    .bind(&symbol)
    .fetch_all(&gw.store.pool)
    .await
    .context("positions_for_symbol")?;

    rows.into_iter()
        .map(|r| {
            let opened_at: chrono::DateTime<chrono::Utc> = r.try_get("opened_at")?;
            let closed_at: Option<chrono::DateTime<chrono::Utc>> = r.try_get("closed_at")?;
            let latest_price_at: Option<chrono::DateTime<chrono::Utc>> =
                r.try_get("latest_price_at")?;
            Ok::<_, sqlx::Error>(json!({
                "position_id": r.try_get::<uuid::Uuid, _>("position_id")?,
                "thesis_id": r.try_get::<Option<uuid::Uuid>, _>("thesis_id")?,
                "symbol": r.try_get::<String, _>("symbol")?,
                "side": r.try_get::<String, _>("side")?,
                "instrument": r.try_get::<String, _>("instrument")?,
                "qty": r.try_get::<f64, _>("qty")?,
                "avg_price": r.try_get::<f64, _>("avg_price")?,
                "delta_notional": r.try_get::<f64, _>("delta_notional")?,
                "premium_at_risk": r.try_get::<f64, _>("premium_at_risk")?,
                "opened_at": opened_at,
                "closed_at": closed_at,
                "realized_pnl": r.try_get::<Option<f64>, _>("realized_pnl")?,
                "unrealized_pnl": r.try_get::<Option<f64>, _>("unrealized_pnl")?,
                "latest_price": r.try_get::<Option<f64>, _>("latest_price")?,
                "latest_price_at": latest_price_at,
                "fill_count": r.try_get::<i32, _>("fill_count")?,
                "thesis_state": r.try_get::<Option<String>, _>("thesis_state")?,
                "thesis_direction": r.try_get::<Option<String>, _>("thesis_direction")?,
            }))
        })
        .collect::<Result<_, _>>()
        .context("encode positions_for_symbol")
}

async fn get_symbol_events(
    State(gw): State<Arc<Gateway>>,
    Query(q): Query<CandlesQuery>,
) -> impl IntoResponse {
    let Some(sym) = q.symbol else {
        return (StatusCode::BAD_REQUEST, "symbol query param required").into_response();
    };
    let interval = ChartInterval::parse(q.interval.as_deref());
    let lookback_days = chart_lookback_days(q.range.as_deref(), interval);
    match gw.store.symbol_events(&sym, lookback_days).await {
        Ok(rows) => (StatusCode::OK, Json(rows)).into_response(),
        Err(e) => {
            warn!(symbol = %sym, error = %e, "get_symbol_events failed");
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
    }
}

#[derive(Debug, Deserialize, Default)]
struct AlertsQuery {
    /// `?unacked=true` filters to dismissable alerts (live feed default).
    #[serde(default)]
    unacked: Option<bool>,
}

async fn list_alerts(
    State(gw): State<Arc<Gateway>>,
    Query(q): Query<AlertsQuery>,
) -> impl IntoResponse {
    let only_unacked = q.unacked.unwrap_or(false);
    match gw.store.recent_alerts_filtered(100, only_unacked).await {
        Ok(alerts) => (StatusCode::OK, Json(alerts)).into_response(),
        Err(e) => {
            warn!(error = %e, "list_alerts failed");
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
    }
}

async fn ack_alert(State(gw): State<Arc<Gateway>>, Path(id): Path<i64>) -> impl IntoResponse {
    match gw.store.acknowledge_alert(id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, format!("alert {id} not found")).into_response(),
        Err(e) => {
            warn!(id, error = %e, "ack_alert failed");
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
    }
}

async fn get_regime(State(gw): State<Arc<Gateway>>) -> impl IntoResponse {
    match gw.store.latest_market_state().await {
        Ok(Some(r)) => (StatusCode::OK, Json(r)).into_response(),
        Ok(None) => (
            StatusCode::OK,
            Json(serde_json::json!({"regime": "unknown", "capitulation": false, "indicators": {}})),
        )
            .into_response(),
        Err(e) => {
            warn!(error = %e, "get_regime failed");
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
    }
}

async fn list_tickers(State(gw): State<Arc<Gateway>>) -> impl IntoResponse {
    match gw.store.active_tickers().await {
        Ok(rows) => (StatusCode::OK, Json(rows)).into_response(),
        Err(e) => {
            warn!(error = %e, "list_tickers failed");
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
    }
}

async fn stream(
    State(gw): State<Arc<Gateway>>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = gw.hub.subscribe();
    let connected = stream::once(async {
        Ok(Event::default().data(
            json!({
                "subject": "stream.connected",
                "kind": "stream",
                "payload": { "status": "open" }
            })
            .to_string(),
        ))
    });
    let broadcast = BroadcastStream::new(rx).filter_map(|res| async move {
        match res {
            Ok(s) => Some(Ok(Event::default().data(s))),
            Err(_) => None, // Lagged → drop
        }
    });
    let stream = connected.chain(broadcast).boxed();
    Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(Duration::from_secs(25))
            .text("keepalive"),
    )
}

#[derive(Debug, Deserialize)]
struct DecisionReq {
    #[serde(default)]
    thesis_id: String,
    action: String,
    #[serde(default)]
    user_choice: String,
    #[serde(default)]
    disagreement_reason: String,
    #[serde(default)]
    disagreement_detail: String,
    #[serde(default)]
    human_conviction: String,
    #[serde(default)]
    reason: String,
    #[serde(default)]
    sizing: Option<serde_json::Value>,
    #[serde(default)]
    manual_fill: Option<ManualFillReq>,
    #[serde(default)]
    chart_range_seen: Option<String>,
}

const DISAGREEMENT_REASONS: &[&str] = &[
    "wrong_cluster",
    "not_my_edge",
    "signal_too_weak",
    "valuation_priced",
    "data_stale",
    "llm_overreached",
    "risk_too_high",
    "other",
];

const HUMAN_CONVICTIONS: &[&str] = &["low", "medium", "high"];

fn normalize_disagreement(
    action: &str,
    user_choice: &str,
    raw_reason: &str,
    raw_detail: &str,
) -> Result<(Option<String>, Option<String>), String> {
    let reason = raw_reason.trim().to_ascii_lowercase();
    let detail = raw_detail.trim().to_string();
    let required = action == "skip" || user_choice == "rejected";

    if reason.is_empty() {
        if required {
            return Err("disagreement_reason required for skip/rejected decisions".into());
        }
        if !detail.is_empty() {
            return Err("disagreement_reason required when disagreement_detail is provided".into());
        }
        return Ok((None, None));
    }

    if !DISAGREEMENT_REASONS.contains(&reason.as_str()) {
        return Err(format!("invalid disagreement_reason: {reason}"));
    }
    if reason == "other" && detail.is_empty() {
        return Err("disagreement_detail required when disagreement_reason is other".into());
    }

    Ok((
        Some(reason),
        if detail.is_empty() {
            None
        } else {
            Some(detail)
        },
    ))
}

fn normalize_human_conviction(raw: &str) -> Result<String, String> {
    let value = raw.trim().to_ascii_lowercase();
    if value.is_empty() {
        return Err("human_conviction required".into());
    }
    if !HUMAN_CONVICTIONS.contains(&value.as_str()) {
        return Err(format!("invalid human_conviction: {value}"));
    }
    Ok(value)
}

#[derive(Debug, Clone, Deserialize)]
struct ManualFillReq {
    #[serde(default)]
    position_id: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    instrument: Option<String>,
    qty: f64,
    price: f64,
    #[serde(default)]
    fees: Option<f64>,
    #[serde(default)]
    filled_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    notes: Option<String>,
    #[serde(default)]
    delta_notional: Option<f64>,
    #[serde(default)]
    premium_at_risk: Option<f64>,
}

#[derive(Debug, Deserialize, Default)]
struct AttentionQuery {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    limit: Option<i64>,
}

async fn list_discovery_pool(State(gw): State<Arc<Gateway>>) -> impl IntoResponse {
    use sqlx::Row;
    let rows = sqlx::query(
        r#"SELECT dp.symbol, dp.company_name, dp.sector, dp.industry,
                  dp.market_cap, dp.first_seen_at,
                  latest.thesis_id AS latest_thesis_id,
                  latest.state AS thesis_state,
                  latest.direction AS thesis_direction,
                  tech.technical_state AS technical_state,
                  tech.entry_stance AS entry_stance,
                  tech.pct_vs_200d AS technical_pct_vs_200d,
                  (SELECT count(*) FROM thesis th
                    WHERE th.symbol = dp.symbol
                      AND th.state NOT IN ('closed','disqualified')) AS open_theses
             FROM discovery_pool dp
        LEFT JOIN LATERAL (
             SELECT th.thesis_id, th.state, th.forecast->>'direction' AS direction
               FROM thesis th
              WHERE th.symbol = dp.symbol
                AND th.state NOT IN ('closed','disqualified')
           ORDER BY th.updated_at DESC
              LIMIT 1
        ) latest ON TRUE
        LEFT JOIN LATERAL (
            WITH bars AS (
                SELECT ts, close::float8 AS close, high::float8 AS high
                  FROM price_bar
                 WHERE symbol = dp.symbol
              ORDER BY ts DESC
                 LIMIT 260
            ), ranked AS (
                SELECT ts, close, high, row_number() OVER (ORDER BY ts DESC) AS rn
                  FROM bars
            ), latest_bar AS (
                SELECT close
                  FROM ranked
                 WHERE rn = 1
            ), stats AS (
                SELECT count(*) FILTER (WHERE rn <= 200) AS bars_200,
                       avg(close) FILTER (WHERE rn <= 50) AS sma50,
                       avg(close) FILTER (WHERE rn <= 200) AS sma200,
                       max(high) FILTER (WHERE rn <= 252) AS high252
                  FROM ranked
            ), classified AS (
                SELECT CASE
                         WHEN stats.bars_200 < 200 OR stats.sma200 IS NULL THEN 'unknown'
                         WHEN ((latest_bar.close - stats.sma200) / NULLIF(stats.sma200, 0) * 100.0) > 20.0
                           OR ((latest_bar.close - stats.high252) / NULLIF(stats.high252, 0) * 100.0) >= -2.0 THEN 'extended'
                         WHEN ((latest_bar.close - stats.sma200) / NULLIF(stats.sma200, 0) * 100.0) < -5.0 THEN 'deteriorating'
                         WHEN stats.sma50 IS NOT NULL
                           AND abs((latest_bar.close - stats.sma50) / NULLIF(stats.sma50, 0) * 100.0) <= 5.0 THEN 'base_building'
                         WHEN ((latest_bar.close - stats.sma200) / NULLIF(stats.sma200, 0) * 100.0) >= 0.0 THEN 'constructive'
                         ELSE 'unknown'
                       END AS technical_state,
                       ((latest_bar.close - stats.sma200) / NULLIF(stats.sma200, 0) * 100.0)::float8 AS pct_vs_200d
                  FROM latest_bar CROSS JOIN stats
            )
            SELECT technical_state,
                   CASE technical_state
                     WHEN 'extended' THEN 'avoid_chase'
                     WHEN 'deteriorating' THEN 'avoid'
                     WHEN 'base_building' THEN 'wait_breakout'
                     WHEN 'constructive' THEN 'constructive'
                     ELSE 'wait_data'
                   END AS entry_stance,
                   pct_vs_200d
              FROM classified
        ) tech ON TRUE
            WHERE dp.dropped_at IS NULL
         ORDER BY dp.market_cap DESC NULLS LAST, dp.symbol"#,
    )
    .fetch_all(&gw.store.pool)
    .await;
    match rows {
        Ok(rows) => {
            let out: Vec<serde_json::Value> = rows
                .into_iter()
                .map(|r| {
                    let first_seen: chrono::DateTime<chrono::Utc> = r
                        .try_get("first_seen_at")
                        .unwrap_or_else(|_| chrono::Utc::now());
                    serde_json::json!({
                        "symbol": r.try_get::<String, _>("symbol").unwrap_or_default(),
                        "company_name": r.try_get::<Option<String>, _>("company_name").ok().flatten(),
                        "sector": r.try_get::<Option<String>, _>("sector").ok().flatten(),
                        "industry": r.try_get::<Option<String>, _>("industry").ok().flatten(),
                        "market_cap": r.try_get::<Option<i64>, _>("market_cap").ok().flatten(),
                        "first_seen_at": first_seen,
                        "latest_thesis_id": r.try_get::<Option<uuid::Uuid>, _>("latest_thesis_id").ok().flatten(),
                        "thesis_state": r.try_get::<Option<String>, _>("thesis_state").ok().flatten(),
                        "thesis_direction": r.try_get::<Option<String>, _>("thesis_direction").ok().flatten(),
                        "technical_state": r.try_get::<Option<String>, _>("technical_state").ok().flatten(),
                        "entry_stance": r.try_get::<Option<String>, _>("entry_stance").ok().flatten(),
                        "technical_pct_vs_200d": r.try_get::<Option<f64>, _>("technical_pct_vs_200d").ok().flatten(),
                        "open_theses": r.try_get::<i64, _>("open_theses").unwrap_or(0),
                    })
                })
                .collect();
            (StatusCode::OK, Json(out)).into_response()
        }
        Err(e) => {
            warn!(error = %e, "list_discovery_pool failed");
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
    }
}

/// One JSON snapshot of every key service for #92 Diagnostics panel.
/// All queries hit indexed columns and return aggregates — designed to be
/// polled every 30s without blowing up the DB.
async fn get_system_status(State(gw): State<Arc<Gateway>>) -> impl IntoResponse {
    use sqlx::Row;
    let pool = &gw.store.pool;

    // ---- ingest sources ----
    let mut ingest = serde_json::Map::new();
    // ingest_event-backed sources (fred, edgar, etc)
    if let Ok(rows) = sqlx::query(
        r#"SELECT source, MAX(ingested_at) AS last_at,
                  COUNT(*) FILTER (WHERE ingested_at > now() - interval '24 hours') AS cnt_24h
             FROM ingest_event GROUP BY source"#,
    )
    .fetch_all(pool)
    .await
    {
        for r in rows {
            let src: String = r.try_get("source").unwrap_or_default();
            let last: Option<chrono::DateTime<chrono::Utc>> = r.try_get("last_at").ok();
            let cnt: i64 = r.try_get("cnt_24h").unwrap_or(0);
            ingest.insert(src, json!({"last_at": last, "count_24h": cnt}));
        }
    }
    // news_article (own table, not via ingest_event)
    if let Ok(r) = sqlx::query(
        r#"SELECT MAX(ingested_at) AS last_at,
                  COUNT(*) FILTER (WHERE ingested_at > now() - interval '24 hours') AS cnt_24h,
                  COUNT(DISTINCT symbol) FILTER (WHERE ingested_at > now() - interval '24 hours') AS sym_24h
             FROM news_article"#,
    )
    .fetch_one(pool)
    .await
    {
        let last: Option<chrono::DateTime<chrono::Utc>> = r.try_get("last_at").ok();
        let cnt: i64 = r.try_get("cnt_24h").unwrap_or(0);
        let sym: i64 = r.try_get("sym_24h").unwrap_or(0);
        ingest.insert(
            "news".to_string(),
            json!({"last_at": last, "count_24h": cnt, "symbols_24h": sym}),
        );
    }
    // estimate_snapshot
    if let Ok(r) = sqlx::query(
        r#"SELECT MAX(snapshot_at) AS last_at,
                  COUNT(*) FILTER (WHERE snapshot_at > now() - interval '24 hours') AS cnt_24h,
                  COUNT(DISTINCT symbol) FILTER (WHERE snapshot_at > now() - interval '24 hours') AS sym_24h
             FROM estimate_snapshot"#,
    )
    .fetch_one(pool)
    .await
    {
        let last: Option<chrono::DateTime<chrono::Utc>> = r.try_get("last_at").ok();
        let cnt: i64 = r.try_get("cnt_24h").unwrap_or(0);
        let sym: i64 = r.try_get("sym_24h").unwrap_or(0);
        ingest.insert(
            "estimates".to_string(),
            json!({"last_at": last, "count_24h": cnt, "symbols_24h": sym}),
        );
    }

    // Explicit pass-level source health (#132). This is authoritative for
    // "checked but no new rows" vs "source failed"; the table may be absent
    // briefly before migrations run, so diagnostics degrade gracefully.
    let source_health: Vec<serde_json::Value> = sqlx::query(
        r#"SELECT source, last_status, last_started_at, last_success_at,
                  last_failure_at, last_failure_kind, last_error, retry_after_at,
                  rows_seen, rows_inserted, symbols_attempted, symbols_failed,
                  updated_at
             FROM source_health
         ORDER BY source"#,
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|r| {
        let now = chrono::Utc::now();
        let last_started_at: Option<chrono::DateTime<chrono::Utc>> =
            r.try_get("last_started_at").ok();
        let last_success_at: Option<chrono::DateTime<chrono::Utc>> =
            r.try_get("last_success_at").ok();
        let last_failure_at: Option<chrono::DateTime<chrono::Utc>> =
            r.try_get("last_failure_at").ok();
        let retry_after_at: Option<chrono::DateTime<chrono::Utc>> =
            r.try_get("retry_after_at").ok();
        let updated_at: Option<chrono::DateTime<chrono::Utc>> = r.try_get("updated_at").ok();
        let last_status = r.try_get::<String, _>("last_status").unwrap_or_default();
        let (effective_status, stale_running, running_age_minutes) =
            source_health_effective_status(&last_status, last_started_at, now);
        json!({
            "source": r.try_get::<String, _>("source").unwrap_or_default(),
            "last_status": last_status,
            "effective_status": effective_status,
            "stale_running": stale_running,
            "running_age_minutes": running_age_minutes,
            "last_started_at": last_started_at,
            "last_success_at": last_success_at,
            "last_failure_at": last_failure_at,
            "last_failure_kind": r.try_get::<Option<String>, _>("last_failure_kind").ok().flatten(),
            "last_error": r.try_get::<Option<String>, _>("last_error").ok().flatten(),
            "retry_after_at": retry_after_at,
            "rows_seen": r.try_get::<i64, _>("rows_seen").unwrap_or(0),
            "rows_inserted": r.try_get::<i64, _>("rows_inserted").unwrap_or(0),
            "symbols_attempted": r.try_get::<i32, _>("symbols_attempted").unwrap_or(0),
            "symbols_failed": r.try_get::<i32, _>("symbols_failed").unwrap_or(0),
            "updated_at": updated_at,
        })
    })
    .collect();

    let price_freshness = {
        let expected =
            crate::platform::market_calendar::expected_latest_us_equity_session(chrono::Utc::now());
        let row = sqlx::query(
            r#"WITH latest AS (
                   SELECT symbol, MAX(ts)::date AS latest_session
                     FROM price_bar
                 GROUP BY symbol
               )
               SELECT MAX(latest_session) AS latest_session,
                      COUNT(*) AS symbols_total,
                      COUNT(*) FILTER (WHERE latest_session >= $1) AS symbols_fresh
                 FROM latest"#,
        )
        .bind(expected)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();
        let latest: Option<chrono::NaiveDate> =
            row.as_ref().and_then(|r| r.try_get("latest_session").ok());
        let total: i64 = row
            .as_ref()
            .and_then(|r| r.try_get("symbols_total").ok())
            .unwrap_or(0);
        let fresh: i64 = row
            .as_ref()
            .and_then(|r| r.try_get("symbols_fresh").ok())
            .unwrap_or(0);
        json!({
            "expected_latest_session": expected,
            "actual_latest_session": latest,
            "symbols_total": total,
            "symbols_fresh": fresh,
            "status": if latest.is_some_and(|d| d >= expected) { "ok" } else { "stale" },
        })
    };

    // ---- discovery ----
    let discovery = {
        let last_at: Option<chrono::DateTime<chrono::Utc>> =
            sqlx::query_scalar("SELECT MAX(proposed_at) FROM discovery_candidate")
                .fetch_optional(pool)
                .await
                .ok()
                .flatten()
                .flatten();
        let open: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM discovery_candidate WHERE status = 'proposed'",
        )
        .fetch_one(pool)
        .await
        .unwrap_or(0);
        let by_signal: Vec<serde_json::Value> = sqlx::query(
            r#"SELECT signal_name, COUNT(*) AS n
                 FROM discovery_candidate
                WHERE status = 'proposed'
             GROUP BY signal_name ORDER BY n DESC"#,
        )
        .fetch_all(pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|r| {
            json!({
                "signal": r.try_get::<String, _>("signal_name").unwrap_or_default(),
                "count": r.try_get::<i64, _>("n").unwrap_or(0),
            })
        })
        .collect();
        let pool_size: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM discovery_pool WHERE dropped_at IS NULL")
                .fetch_one(pool)
                .await
                .unwrap_or(0);
        json!({
            "last_pass_at": last_at,
            "open_candidates": open,
            "by_signal": by_signal,
            "pool_size": pool_size,
        })
    };

    // ---- cognition (context + thesis) ----
    let cognition = {
        let ctx_24h: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM ticker_context WHERE created_at > now() - interval '24 hours'",
        )
        .fetch_one(pool)
        .await
        .unwrap_or(0);
        let ctx_total: i64 =
            sqlx::query_scalar("SELECT COUNT(DISTINCT symbol) FROM ticker_context")
                .fetch_one(pool)
                .await
                .unwrap_or(0);
        let by_state: Vec<serde_json::Value> = sqlx::query(
            r#"SELECT state, COUNT(*) AS n FROM thesis
                GROUP BY state ORDER BY n DESC"#,
        )
        .fetch_all(pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|r| {
            json!({
                "state": r.try_get::<String, _>("state").unwrap_or_default(),
                "count": r.try_get::<i64, _>("n").unwrap_or(0),
            })
        })
        .collect();
        let runs_24h: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM cognition_run WHERE started_at > now() - interval '24 hours'",
        )
        .fetch_one(pool)
        .await
        .unwrap_or(0);
        let runs_by_status: Vec<serde_json::Value> = sqlx::query(
            r#"SELECT status, COUNT(*) AS n
                 FROM cognition_run
                WHERE started_at > now() - interval '24 hours'
             GROUP BY status ORDER BY n DESC, status"#,
        )
        .fetch_all(pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|r| {
            json!({
                "status": r.try_get::<String, _>("status").unwrap_or_default(),
                "count": r.try_get::<i64, _>("n").unwrap_or(0),
            })
        })
        .collect();
        let latest_runs: Vec<serde_json::Value> = sqlx::query(
            r#"SELECT id, symbol, trigger, sweep_reason, status, reason,
                      context_version, thesis_id, thesis_classification,
                      evidence_open_count, evidence_blocking_count,
                      started_at, finished_at, next_retry_at, error, source_ref
                 FROM cognition_run
             ORDER BY started_at DESC
                LIMIT 8"#,
        )
        .fetch_all(pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(cognition_run_json)
        .collect();
        json!({
            "contexts_24h": ctx_24h,
            "contexts_total_symbols": ctx_total,
            "thesis_by_state": by_state,
            "runs_24h": runs_24h,
            "runs_by_status": runs_by_status,
            "latest_runs": latest_runs,
        })
    };

    // ---- evidence requirements ----
    let evidence = {
        let by_state: Vec<serde_json::Value> = sqlx::query(
            r#"SELECT blocking_state, COUNT(*) AS n
                 FROM evidence_requirement
             GROUP BY blocking_state ORDER BY n DESC"#,
        )
        .fetch_all(pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|r| {
            json!({
                "state": r.try_get::<String, _>("blocking_state").unwrap_or_default(),
                "count": r.try_get::<i64, _>("n").unwrap_or(0),
            })
        })
        .collect();
        let open: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM evidence_requirement WHERE blocking_state <> 'satisfied'",
        )
        .fetch_one(pool)
        .await
        .unwrap_or(0);
        let by_reason: Vec<serde_json::Value> = sqlx::query(
            r#"SELECT reason, COUNT(*) AS n
                 FROM (
                    SELECT COALESCE(
                               NULLIF(source_ref->>'acquisition_state', ''),
                               blocking_state
                           ) AS reason
                      FROM evidence_requirement
                     WHERE blocking_state <> 'satisfied'
                 ) reasons
             GROUP BY reason ORDER BY n DESC"#,
        )
        .fetch_all(pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|r| {
            json!({
                "reason": r.try_get::<String, _>("reason").unwrap_or_default(),
                "count": r.try_get::<i64, _>("n").unwrap_or(0),
            })
        })
        .collect();
        let source_tasks_by_state: Vec<serde_json::Value> = sqlx::query(
            r#"SELECT state, COUNT(*) AS n
                 FROM source_task
             GROUP BY state ORDER BY n DESC, state"#,
        )
        .fetch_all(pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|r| {
            json!({
                "state": r.try_get::<String, _>("state").unwrap_or_default(),
                "count": r.try_get::<i64, _>("n").unwrap_or(0),
            })
        })
        .collect();
        let source_tasks_by_action: Vec<serde_json::Value> = sqlx::query(
            r#"SELECT provider,
                      action,
                      state,
                      COUNT(*) AS n,
                      COUNT(*) FILTER (
                          WHERE state IN ('queued', 'no_rows', 'failed', 'rate_limited', 'blocked', 'satisfied')
                            AND due_at <= now()
                      ) AS due_count,
                      COUNT(*) FILTER (
                          WHERE state = 'fetching'
                            AND updated_at < now() - interval '15 minutes'
                      ) AS stale_fetching_count,
                      MIN(due_at) FILTER (
                          WHERE state IN ('queued', 'no_rows', 'failed', 'rate_limited', 'blocked', 'satisfied')
                            AND due_at <= now()
                      ) AS next_due_at,
                      MAX(updated_at) AS last_updated_at,
                      (array_agg(target_id ORDER BY
                          CASE priority
                            WHEN 'blocking' THEN 0
                            WHEN 'high' THEN 1
                            WHEN 'medium' THEN 2
                            ELSE 3
                          END,
                          due_at,
                          target_id
                       ))[1:5] AS sample_targets
                 FROM source_task
             GROUP BY provider, action, state
             ORDER BY
                      COUNT(*) FILTER (
                          WHERE state IN ('queued', 'no_rows', 'failed', 'rate_limited', 'blocked', 'satisfied')
                            AND due_at <= now()
                      ) DESC,
                      COUNT(*) FILTER (
                          WHERE state = 'fetching'
                            AND updated_at < now() - interval '15 minutes'
                      ) DESC,
                      COUNT(*) DESC,
                      provider,
                      action,
                      state
                LIMIT 30"#,
        )
        .fetch_all(pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|r| {
            json!({
                "provider": r.try_get::<String, _>("provider").unwrap_or_default(),
                "action": r.try_get::<String, _>("action").unwrap_or_default(),
                "state": r.try_get::<String, _>("state").unwrap_or_default(),
                "count": r.try_get::<i64, _>("n").unwrap_or(0),
                "due_count": r.try_get::<i64, _>("due_count").unwrap_or(0),
                "stale_fetching_count": r.try_get::<i64, _>("stale_fetching_count").unwrap_or(0),
                "next_due_at": r.try_get::<Option<chrono::DateTime<chrono::Utc>>, _>("next_due_at").ok().flatten(),
                "last_updated_at": r.try_get::<Option<chrono::DateTime<chrono::Utc>>, _>("last_updated_at").ok().flatten(),
                "sample_targets": r.try_get::<Vec<String>, _>("sample_targets").unwrap_or_default(),
            })
        })
        .collect();
        let source_tasks_due: i64 = sqlx::query_scalar(
            r#"SELECT COUNT(*) FROM source_task
                WHERE state IN ('queued', 'no_rows', 'failed', 'rate_limited', 'blocked')
                  AND due_at <= now()"#,
        )
        .fetch_one(pool)
        .await
        .unwrap_or(0);
        let source_tasks_stale_fetching: i64 = sqlx::query_scalar(
            r#"SELECT COUNT(*) FROM source_task
                WHERE state = 'fetching'
                  AND updated_at < now() - interval '15 minutes'"#,
        )
        .fetch_one(pool)
        .await
        .unwrap_or(0);
        json!({
            "open_requirements": open,
            "by_state": by_state,
            "by_reason": by_reason,
            "source_tasks_due": source_tasks_due,
            "source_tasks_stale_fetching": source_tasks_stale_fetching,
            "source_tasks_by_state": source_tasks_by_state,
            "source_tasks_by_action": source_tasks_by_action,
        })
    };

    // ---- attention queue ----
    let attention = {
        let open: i64 = sqlx::query_scalar(
            r#"SELECT COUNT(*) FROM attention_item
                WHERE status = 'open'
                  AND (
                    fsm_state <> 'operator_deferred'
                    OR (resurface_at IS NOT NULL AND resurface_at <= now())
                  )"#,
        )
        .fetch_one(pool)
        .await
        .unwrap_or(0);
        let deferred: i64 = sqlx::query_scalar(
            r#"SELECT COUNT(*) FROM attention_item
                WHERE status = 'open'
                  AND fsm_state = 'operator_deferred'
                  AND (resurface_at IS NULL OR resurface_at > now())"#,
        )
        .fetch_one(pool)
        .await
        .unwrap_or(0);
        let by_kind: Vec<serde_json::Value> = sqlx::query(
            r#"SELECT kind, COUNT(*) AS n FROM attention_item
                WHERE status = 'open'
                  AND (
                    fsm_state <> 'operator_deferred'
                    OR (resurface_at IS NOT NULL AND resurface_at <= now())
                  )
             GROUP BY kind ORDER BY n DESC"#,
        )
        .fetch_all(pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|r| {
            json!({
                "kind": r.try_get::<String, _>("kind").unwrap_or_default(),
                "count": r.try_get::<i64, _>("n").unwrap_or(0),
            })
        })
        .collect();
        let by_state: Vec<serde_json::Value> = sqlx::query(
            r#"SELECT fsm_state, COUNT(*) AS n FROM attention_item
                WHERE status = 'open'
             GROUP BY fsm_state ORDER BY n DESC, fsm_state"#,
        )
        .fetch_all(pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|r| {
            json!({
                "state": r.try_get::<String, _>("fsm_state").unwrap_or_default(),
                "count": r.try_get::<i64, _>("n").unwrap_or(0),
            })
        })
        .collect();
        let by_owner: Vec<serde_json::Value> = sqlx::query(
            r#"SELECT owner, COUNT(*) AS n FROM attention_item
                WHERE status = 'open'
             GROUP BY owner ORDER BY n DESC, owner"#,
        )
        .fetch_all(pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|r| {
            json!({
                "owner": r.try_get::<String, _>("owner").unwrap_or_default(),
                "count": r.try_get::<i64, _>("n").unwrap_or(0),
            })
        })
        .collect();
        json!({
            "open_items": open,
            "deferred_items": deferred,
            "by_kind": by_kind,
            "by_state": by_state,
            "by_owner": by_owner,
        })
    };

    // ---- llm audit ----
    let llm = {
        let calls_24h: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM llm_invocation WHERE at > now() - interval '24 hours'",
        )
        .fetch_one(pool)
        .await
        .unwrap_or(0);
        let avg_ms: Option<f64> = sqlx::query_scalar(
            "SELECT AVG(latency_ms)::float8 FROM llm_invocation
              WHERE at > now() - interval '24 hours'",
        )
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .flatten();
        let by_prompt: Vec<serde_json::Value> = sqlx::query(
            r#"SELECT prompt_name, COUNT(*) AS n,
                      round(AVG(latency_ms))::int AS avg_ms,
                      MAX(at) AS last_at
                 FROM llm_invocation
                WHERE at > now() - interval '24 hours'
             GROUP BY prompt_name ORDER BY n DESC LIMIT 10"#,
        )
        .fetch_all(pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|r| {
            let last: Option<chrono::DateTime<chrono::Utc>> = r.try_get("last_at").ok();
            json!({
                "prompt": r.try_get::<String, _>("prompt_name").unwrap_or_default(),
                "count": r.try_get::<i64, _>("n").unwrap_or(0),
                "avg_ms": r.try_get::<i32, _>("avg_ms").ok(),
                "last_at": last,
            })
        })
        .collect();
        json!({
            "calls_24h": calls_24h,
            "avg_latency_ms": avg_ms.map(|v| v.round() as i64),
            "by_prompt": by_prompt,
        })
    };

    let body = json!({
        "as_of": chrono::Utc::now(),
        "ingest": serde_json::Value::Object(ingest),
        "source_health": source_health,
        "price_freshness": price_freshness,
        "discovery": discovery,
        "cognition": cognition,
        "evidence": evidence,
        "attention": attention,
        "llm": llm,
    });
    (StatusCode::OK, Json(body)).into_response()
}

async fn get_brain_overview(State(gw): State<Arc<Gateway>>) -> impl IntoResponse {
    use sqlx::Row;

    let pool = &gw.store.pool;
    let rows = match sqlx::query(
        r#"SELECT bt.id, bt.scope, bt.key, bt.name, bt.state, bt.direction,
                  bt.summary, bt.core_claim, bt.why_now,
                  bt.evidence, bt.invalidation_conditions, bt.beneficiaries,
                  bt.losers, bt.open_questions, bt.missing_evidence,
                  bt.source_ref, bt.freshness_target_minutes,
                  bt.last_evaluated_at, bt.version, bt.created_at, bt.updated_at,
                  CASE
                    WHEN bt.last_evaluated_at IS NULL THEN 'missing'
                    WHEN bt.last_evaluated_at < now() - (bt.freshness_target_minutes::text || ' minutes')::interval THEN 'stale'
                    ELSE 'fresh'
                  END AS freshness,
                  COALESCE(ticker_map.tickers, '[]'::jsonb) AS tickers,
                  COALESCE(watchlist_map.watchlists, '[]'::jsonb) AS watchlists,
                  COALESCE(nomination_map.nominations, '[]'::jsonb) AS nominations,
                  COALESCE(change_map.latest_changes, '[]'::jsonb) AS latest_changes
             FROM brain_thesis bt
        LEFT JOIN LATERAL (
             SELECT jsonb_agg(jsonb_build_object(
                        'symbol', btt.symbol,
                        'role', btt.role,
                        'rationale', btt.rationale,
                        'conviction', btt.conviction,
                        'thesis_state', latest.state,
                        'thesis_direction', latest.direction,
                        'open_theses', COALESCE(open_count.n, 0)
                    ) ORDER BY COALESCE(btt.conviction, 0) DESC, btt.symbol) AS tickers
               FROM brain_thesis_ticker btt
          LEFT JOIN LATERAL (
                    SELECT th.state, th.forecast->>'direction' AS direction
                      FROM thesis th
                     WHERE th.symbol = btt.symbol
                       AND th.state NOT IN ('closed', 'disqualified')
                  ORDER BY th.updated_at DESC
                     LIMIT 1
                ) latest ON TRUE
          LEFT JOIN LATERAL (
                    SELECT count(*) AS n
                      FROM thesis th
                     WHERE th.symbol = btt.symbol
                       AND th.state NOT IN ('closed', 'disqualified')
                ) open_count ON TRUE
              WHERE btt.brain_thesis_id = bt.id
        ) ticker_map ON TRUE
        LEFT JOIN LATERAL (
             SELECT jsonb_agg(jsonb_build_object(
                        'id', wl.id,
                        'name', wl.name,
                        'color', wl.color,
                        'is_system', wl.is_system
                    ) ORDER BY wl.name) AS watchlists
               FROM (
                    SELECT DISTINCT w.id, w.name, w.color, w.is_system
                      FROM brain_thesis_watchlist btw
                      JOIN watchlist w ON w.id = btw.watchlist_id
                     WHERE btw.brain_thesis_id = bt.id
                    UNION
                    SELECT DISTINCT w.id, w.name, w.color, w.is_system
                      FROM brain_thesis_ticker btt
                      JOIN watchlist_member wm ON wm.symbol = btt.symbol
                      JOIN watchlist w ON w.id = wm.watchlist_id
                     WHERE btt.brain_thesis_id = bt.id
               ) wl
        ) watchlist_map ON TRUE
        LEFT JOIN LATERAL (
             SELECT jsonb_agg(jsonb_build_object(
                        'candidate_id', dc.id,
                        'symbol', dc.symbol,
                        'signal_name', dc.signal_name,
                        'signal_value', dc.signal_value,
                        'reasoning', dc.reasoning,
                        'proposed_at', dc.proposed_at
                    ) ORDER BY dc.proposed_at DESC) AS nominations
               FROM (
                    SELECT dc.id, dc.symbol, dc.signal_name, dc.signal_value,
                           dc.reasoning, dc.proposed_at
                      FROM discovery_candidate dc
                     WHERE dc.status = 'proposed'
                       AND EXISTS (
                           SELECT 1
                             FROM brain_thesis_ticker btt
                            WHERE btt.brain_thesis_id = bt.id
                              AND btt.symbol = dc.symbol
                       )
                  ORDER BY dc.proposed_at DESC
                     LIMIT 8
               ) dc
        ) nomination_map ON TRUE
        LEFT JOIN LATERAL (
             SELECT jsonb_agg(jsonb_build_object(
                        'version', vh.version,
                        'rationale', vh.rationale,
                        'at', vh.at
                    ) ORDER BY vh.at DESC) AS latest_changes
               FROM (
                    SELECT version, rationale, at
                      FROM brain_thesis_version_history
                     WHERE brain_thesis_id = bt.id
                  ORDER BY at DESC
                     LIMIT 5
               ) vh
        ) change_map ON TRUE
            WHERE bt.active = true
         ORDER BY CASE bt.scope WHEN 'macro' THEN 0 WHEN 'sector' THEN 1 ELSE 2 END,
                  bt.name"#,
    )
    .fetch_all(pool)
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            warn!(error = %e, "get_brain_overview failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }
    };

    let market_state = sqlx::query(
        r#"SELECT as_of, regime, capitulation, indicators, subsector_rs
             FROM market_state
         ORDER BY as_of DESC
            LIMIT 1"#,
    )
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
    .map(|r| {
        let as_of: Option<chrono::DateTime<chrono::Utc>> = r.try_get("as_of").ok();
        json!({
            "as_of": as_of,
            "regime": r.try_get::<String, _>("regime").unwrap_or_else(|_| "unknown".to_string()),
            "capitulation": r.try_get::<bool, _>("capitulation").unwrap_or(false),
            "indicators": r.try_get::<serde_json::Value, _>("indicators").unwrap_or_else(|_| json!({})),
            "subsector_rs": r.try_get::<serde_json::Value, _>("subsector_rs").unwrap_or_else(|_| json!({})),
        })
    });

    let mut macro_thesis = None;
    let mut sectors = Vec::new();
    for r in rows {
        let id: Option<uuid::Uuid> = r.try_get("id").ok();
        let last_evaluated_at: Option<chrono::DateTime<chrono::Utc>> =
            r.try_get("last_evaluated_at").ok();
        let created_at: Option<chrono::DateTime<chrono::Utc>> = r.try_get("created_at").ok();
        let updated_at: Option<chrono::DateTime<chrono::Utc>> = r.try_get("updated_at").ok();
        let item = json!({
            "id": id,
            "scope": r.try_get::<String, _>("scope").unwrap_or_default(),
            "key": r.try_get::<String, _>("key").unwrap_or_default(),
            "name": r.try_get::<String, _>("name").unwrap_or_default(),
            "state": r.try_get::<String, _>("state").unwrap_or_default(),
            "direction": r.try_get::<String, _>("direction").unwrap_or_default(),
            "summary": r.try_get::<String, _>("summary").unwrap_or_default(),
            "core_claim": r.try_get::<String, _>("core_claim").unwrap_or_default(),
            "why_now": r.try_get::<Option<String>, _>("why_now").ok().flatten(),
            "evidence": r.try_get::<serde_json::Value, _>("evidence").unwrap_or_else(|_| json!([])),
            "invalidation_conditions": r.try_get::<serde_json::Value, _>("invalidation_conditions").unwrap_or_else(|_| json!([])),
            "beneficiaries": r.try_get::<serde_json::Value, _>("beneficiaries").unwrap_or_else(|_| json!([])),
            "losers": r.try_get::<serde_json::Value, _>("losers").unwrap_or_else(|_| json!([])),
            "open_questions": r.try_get::<serde_json::Value, _>("open_questions").unwrap_or_else(|_| json!([])),
            "missing_evidence": r.try_get::<serde_json::Value, _>("missing_evidence").unwrap_or_else(|_| json!([])),
            "source_ref": r.try_get::<serde_json::Value, _>("source_ref").unwrap_or_else(|_| json!({})),
            "freshness_target_minutes": r.try_get::<i32, _>("freshness_target_minutes").unwrap_or(720),
            "last_evaluated_at": last_evaluated_at,
            "version": r.try_get::<i32, _>("version").unwrap_or(1),
            "created_at": created_at,
            "updated_at": updated_at,
            "freshness": r.try_get::<String, _>("freshness").unwrap_or_else(|_| "missing".to_string()),
            "tickers": r.try_get::<serde_json::Value, _>("tickers").unwrap_or_else(|_| json!([])),
            "watchlists": r.try_get::<serde_json::Value, _>("watchlists").unwrap_or_else(|_| json!([])),
            "nominations": r.try_get::<serde_json::Value, _>("nominations").unwrap_or_else(|_| json!([])),
            "latest_changes": r.try_get::<serde_json::Value, _>("latest_changes").unwrap_or_else(|_| json!([])),
        });

        if item.get("scope").and_then(serde_json::Value::as_str) == Some("macro") {
            macro_thesis = Some(item);
        } else {
            sectors.push(item);
        }
    }

    let macro_direction = macro_thesis
        .as_ref()
        .and_then(|m| m.get("direction"))
        .and_then(serde_json::Value::as_str);
    let contradictions = sectors
        .iter()
        .filter_map(|s| {
            let direction = s.get("direction").and_then(serde_json::Value::as_str)?;
            let name = s.get("name").and_then(serde_json::Value::as_str)?;
            let mismatch = matches!((macro_direction, direction), (Some("risk_off"), "bullish") | (Some("risk_on"), "bearish"));
            mismatch.then(|| {
                json!({
                    "kind": "macro_sector_direction_conflict",
                    "summary": format!("Macro is {} while {name} is {direction}", macro_direction.unwrap_or("unknown")),
                    "brain_thesis_key": s.get("key").cloned().unwrap_or(json!(null)),
                })
            })
        })
        .collect::<Vec<_>>();

    let stale_count = macro_thesis
        .iter()
        .chain(sectors.iter())
        .filter(|s| s.get("freshness").and_then(serde_json::Value::as_str) != Some("fresh"))
        .count();
    let nominations_count = sectors
        .iter()
        .map(|s| {
            s.get("nominations")
                .and_then(serde_json::Value::as_array)
                .map_or(0, Vec::len)
        })
        .sum::<usize>();
    let active_theses = macro_thesis.iter().count() + sectors.len();

    let body = json!({
        "as_of": chrono::Utc::now(),
        "market_state": market_state,
        "macro": macro_thesis,
        "sectors": sectors,
        "contradictions": contradictions,
        "summary": {
            "active_theses": active_theses,
            "stale_or_missing": stale_count,
            "open_nominations": nominations_count,
        },
    });
    (StatusCode::OK, Json(body)).into_response()
}

#[derive(Debug, Deserialize)]
struct BrainStatusQuery {
    symbol: String,
}

#[derive(Debug, Clone)]
struct SourceHealthSnapshot {
    source: String,
    last_status: String,
    last_success_at: Option<chrono::DateTime<chrono::Utc>>,
    last_started_at: Option<chrono::DateTime<chrono::Utc>>,
    last_failure_kind: Option<String>,
    last_error: Option<String>,
    retry_after_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Clone)]
struct SourceTaskSnapshot {
    requirement_key: Option<String>,
    action: String,
    provider: String,
    state: String,
    result: Option<String>,
    priority: String,
    due_at: Option<chrono::DateTime<chrono::Utc>>,
    next_retry_at: Option<chrono::DateTime<chrono::Utc>>,
    attempts: i32,
    last_error: Option<String>,
    updated_at: Option<chrono::DateTime<chrono::Utc>>,
}

async fn get_brain_status(
    State(gw): State<Arc<Gateway>>,
    Query(q): Query<BrainStatusQuery>,
) -> impl IntoResponse {
    use crate::platform::brain::{BrainDecisionInput, age_freshness, decide};
    use chrono::Duration as ChronoDuration;
    use sqlx::Row;

    let symbol = q.symbol.trim().to_ascii_uppercase();
    if symbol.is_empty() || symbol.len() > 14 {
        return (StatusCode::BAD_REQUEST, "invalid symbol").into_response();
    }
    let pool = &gw.store.pool;
    let now = chrono::Utc::now();
    let expected_price_session =
        crate::platform::market_calendar::expected_latest_us_equity_session(now);

    let row = match sqlx::query(
        r#"SELECT
              EXISTS (
                SELECT 1 FROM ticker WHERE symbol = $1 AND status = 'active'
              ) AS active_ticker,
              (SELECT max(ts) FROM price_bar WHERE symbol = $1) AS price_at,
              (SELECT max(ts)::date FROM price_bar WHERE symbol = $1) AS price_session,
              (SELECT max(profile_at) FROM company_profile WHERE symbol = $1) AS profile_at,
              (SELECT company_name FROM company_profile WHERE symbol = $1) AS profile_company_name,
              (SELECT sector FROM company_profile WHERE symbol = $1) AS profile_sector,
              (SELECT industry FROM company_profile WHERE symbol = $1) AS profile_industry,
              (SELECT market_cap FROM company_profile WHERE symbol = $1) AS profile_market_cap,
              (SELECT count(*) FROM company_profile WHERE symbol = $1) AS company_profiles,
              (SELECT max(ingested_at) FROM news_article WHERE symbol = $1) AS news_at,
              (SELECT max(published_at) FROM news_article WHERE symbol = $1) AS news_published_at,
              (SELECT max(snapshot_at) FROM estimate_snapshot WHERE symbol = $1) AS estimates_at,
              (SELECT max(updated_at) FROM earnings_calendar_event WHERE symbol = $1) AS earnings_at,
              (SELECT count(*) FROM earnings_calendar_event
                WHERE symbol = $1
                  AND report_date >= current_date - 30
                  AND report_date <= current_date + 180) AS earnings_events,
              (SELECT min(report_date) FROM earnings_calendar_event
                WHERE symbol = $1
                  AND report_date >= current_date) AS next_earnings_date,
              (SELECT max(at) FROM (
                  SELECT max(snapshot_at) AS at
                    FROM analyst_price_target_snapshot WHERE symbol = $1
                  UNION ALL
                  SELECT max(snapshot_at) AS at
                    FROM analyst_recommendation_snapshot WHERE symbol = $1
                  UNION ALL
                  SELECT max(ingested_at) AS at
                    FROM analyst_price_target_event WHERE symbol = $1
                  UNION ALL
                  SELECT max(ingested_at) AS at
                    FROM analyst_rating_event WHERE symbol = $1
              ) opinion_dates) AS analyst_opinion_at,
              (SELECT count(*) FROM analyst_price_target_snapshot
                WHERE symbol = $1) AS price_target_snapshots,
              (SELECT count(*) FROM analyst_recommendation_snapshot
                WHERE symbol = $1) AS recommendation_snapshots,
              (SELECT count(*) FROM analyst_price_target_event
                WHERE symbol = $1
                  AND published_at > now() - interval '90 days') AS price_target_events,
              (SELECT count(*) FROM analyst_rating_event
                WHERE symbol = $1
                  AND published_at > now() - interval '90 days') AS rating_events,
              (SELECT max(retrieved_at) FROM research_evidence WHERE symbol = $1) AS research_at,
              (SELECT max(ingested_at) FROM company_fact WHERE symbol = $1) AS fundamentals_at,
              (SELECT max(ingested_at) FROM ingest_event
                WHERE source = 'edgar' AND symbol = $1) AS filings_at,
              (SELECT version FROM ticker_context
                WHERE symbol = $1 ORDER BY version DESC LIMIT 1) AS context_version,
              (SELECT created_at FROM ticker_context
                WHERE symbol = $1 ORDER BY version DESC LIMIT 1) AS context_at,
              (SELECT structural_as_of FROM ticker_context
                WHERE symbol = $1 ORDER BY version DESC LIMIT 1) AS structural_as_of,
              (SELECT narrative_as_of FROM ticker_context
                WHERE symbol = $1 ORDER BY version DESC LIMIT 1) AS narrative_as_of,
              (SELECT market_as_of FROM ticker_context
                WHERE symbol = $1 ORDER BY version DESC LIMIT 1) AS market_as_of,
              (SELECT thesis_id FROM thesis
                WHERE symbol = $1 AND state NOT IN ('closed', 'disqualified')
                ORDER BY updated_at DESC LIMIT 1) AS open_thesis_id,
              (SELECT state FROM thesis
                WHERE symbol = $1 AND state NOT IN ('closed', 'disqualified')
                ORDER BY updated_at DESC LIMIT 1) AS open_thesis_state,
              (SELECT forecast->>'direction' FROM thesis
                WHERE symbol = $1 AND state NOT IN ('closed', 'disqualified')
                ORDER BY updated_at DESC LIMIT 1) AS open_thesis_direction,
              (SELECT updated_at FROM thesis
                WHERE symbol = $1 AND state NOT IN ('closed', 'disqualified')
                ORDER BY updated_at DESC LIMIT 1) AS open_thesis_updated_at,
              (SELECT COALESCE(last_evaluated_at, updated_at) FROM thesis
                WHERE symbol = $1 AND state NOT IN ('closed', 'disqualified')
                ORDER BY updated_at DESC LIMIT 1) AS open_thesis_at,
              (SELECT max(created_at) FROM attention_item
                WHERE symbol = $1 AND kind = 'thesis_incomplete') AS latest_decline_at,
              (SELECT count(*) FROM evidence_requirement
                WHERE symbol = $1) AS evidence_rows,
              (SELECT count(*) FROM evidence_requirement
                WHERE symbol = $1 AND blocking_state <> 'satisfied') AS open_evidence,
              (SELECT count(*) FROM evidence_requirement
                WHERE symbol = $1 AND priority = 'blocking'
                  AND blocking_state <> 'satisfied') AS blocking_evidence,
              (SELECT count(*) FROM evidence_requirement
                WHERE symbol = $1 AND blocking_state <> 'satisfied'
                  AND (next_retry_at IS NULL OR next_retry_at <= now())) AS due_evidence,
              (SELECT max(updated_at) FROM evidence_item
                WHERE symbol = $1
                  AND NOT (
                    kind = 'product_research'
                    AND source = 'web_research'
                    AND COALESCE((source_ref->'relevance'->>'accepted')::boolean, false) = false
                  )) AS latest_evidence_item_at,
              (SELECT count(*) FROM evidence_item
                WHERE symbol = $1
                  AND NOT (
                    kind = 'product_research'
                    AND source = 'web_research'
                    AND COALESCE((source_ref->'relevance'->>'accepted')::boolean, false) = false
                  )) AS normalized_evidence_items,
              (SELECT count(*) FROM attention_item
                WHERE symbol = $1 AND status = 'open'
                  AND (
                    fsm_state <> 'operator_deferred'
                    OR (resurface_at IS NOT NULL AND resurface_at <= now())
                  )) AS open_attention
        "#,
    )
    .bind(&symbol)
    .fetch_one(pool)
    .await
    {
        Ok(row) => row,
        Err(e) => {
            warn!(symbol = %symbol, error = %e, "get_brain_status failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }
    };

    let health_rows = sqlx::query(
        r#"SELECT source, last_status, last_started_at, last_success_at,
                  last_failure_kind, last_error, retry_after_at
             FROM source_health
            WHERE source = ANY($1)
         ORDER BY source"#,
    )
    .bind(vec![
        "fmp_price".to_string(),
        "fmp_news".to_string(),
        "massive_news".to_string(),
        "fmp_estimates".to_string(),
        "fmp_analyst_opinion".to_string(),
        "fmp_profile_calendar".to_string(),
        "xbrl".to_string(),
        "edgar".to_string(),
        "web_research".to_string(),
    ])
    .fetch_all(pool)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|r| SourceHealthSnapshot {
        source: r.try_get("source").unwrap_or_default(),
        last_status: r.try_get("last_status").unwrap_or_default(),
        last_success_at: r.try_get("last_success_at").ok().flatten(),
        last_started_at: r.try_get("last_started_at").ok().flatten(),
        last_failure_kind: r.try_get("last_failure_kind").ok().flatten(),
        last_error: r.try_get("last_error").ok().flatten(),
        retry_after_at: r.try_get("retry_after_at").ok().flatten(),
    })
    .collect::<Vec<_>>();

    let task_rows = sqlx::query(
        r#"SELECT requirement_key, action, provider, state, source_ref->>'result' AS result,
                  priority, due_at,
                  next_retry_at, attempts, last_error, updated_at
             FROM source_task
            WHERE scope = 'symbol'
              AND target_id = $1
         ORDER BY
              CASE priority
                WHEN 'blocking' THEN 0
                WHEN 'high' THEN 1
                WHEN 'medium' THEN 2
                ELSE 3
              END,
              due_at ASC,
              action"#,
    )
    .bind(&symbol)
    .fetch_all(pool)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|r| SourceTaskSnapshot {
        requirement_key: r.try_get("requirement_key").ok().flatten(),
        action: r.try_get("action").unwrap_or_default(),
        provider: r.try_get("provider").unwrap_or_default(),
        state: r.try_get("state").unwrap_or_default(),
        result: r.try_get("result").ok().flatten(),
        priority: r.try_get("priority").unwrap_or_default(),
        due_at: r.try_get("due_at").ok().flatten(),
        next_retry_at: r.try_get("next_retry_at").ok().flatten(),
        attempts: r.try_get("attempts").unwrap_or(0),
        last_error: r.try_get("last_error").ok().flatten(),
        updated_at: r.try_get("updated_at").ok().flatten(),
    })
    .collect::<Vec<_>>();

    let health = |names: &[&str], max_age: ChronoDuration| {
        source_health_group(&health_rows, names, now, max_age)
    };
    let price_health = health(&["fmp_price"], ChronoDuration::minutes(30));
    let news_health = health(&["fmp_news", "massive_news"], ChronoDuration::minutes(30));
    let estimates_health = health(&["fmp_estimates"], ChronoDuration::minutes(30));
    let analyst_opinion_health = health(&["fmp_analyst_opinion"], ChronoDuration::minutes(30));
    let profile_health = health(&["fmp_profile_calendar"], ChronoDuration::minutes(30));
    let earnings_health = health(&["fmp_profile_calendar"], ChronoDuration::minutes(30));
    let research_health = health(&["web_research"], ChronoDuration::hours(24));
    let fundamentals_health = health(&["xbrl"], ChronoDuration::minutes(360));
    let filings_health = health(&["edgar"], ChronoDuration::minutes(30));
    let news_status = source_status(&news_health).to_string();
    let estimates_status = source_status(&estimates_health).to_string();
    let analyst_opinion_status = source_status(&analyst_opinion_health).to_string();
    let profile_status = source_status(&profile_health).to_string();
    let earnings_status = source_status(&earnings_health).to_string();
    let research_status = source_status(&research_health).to_string();
    let fundamentals_status = source_status(&fundamentals_health).to_string();
    let filings_status = source_status(&filings_health).to_string();

    let price_at: Option<chrono::DateTime<chrono::Utc>> = row.try_get("price_at").ok();
    let price_session: Option<chrono::NaiveDate> = row.try_get("price_session").ok();
    let profile_at: Option<chrono::DateTime<chrono::Utc>> = row.try_get("profile_at").ok();
    let news_at: Option<chrono::DateTime<chrono::Utc>> = row.try_get("news_at").ok();
    let news_published_at: Option<chrono::DateTime<chrono::Utc>> =
        row.try_get("news_published_at").ok();
    let estimates_at: Option<chrono::DateTime<chrono::Utc>> = row.try_get("estimates_at").ok();
    let earnings_at: Option<chrono::DateTime<chrono::Utc>> = row.try_get("earnings_at").ok();
    let analyst_opinion_at: Option<chrono::DateTime<chrono::Utc>> =
        row.try_get("analyst_opinion_at").ok();
    let research_at: Option<chrono::DateTime<chrono::Utc>> = row.try_get("research_at").ok();
    let fundamentals_at: Option<chrono::DateTime<chrono::Utc>> =
        row.try_get("fundamentals_at").ok();
    let filings_at: Option<chrono::DateTime<chrono::Utc>> = row.try_get("filings_at").ok();
    let context_at: Option<chrono::DateTime<chrono::Utc>> = row.try_get("context_at").ok();
    let thesis_updated_at: Option<chrono::DateTime<chrono::Utc>> =
        row.try_get("open_thesis_updated_at").ok();
    let thesis_at: Option<chrono::DateTime<chrono::Utc>> = row.try_get("open_thesis_at").ok();
    let latest_decline_at: Option<chrono::DateTime<chrono::Utc>> =
        row.try_get("latest_decline_at").ok();
    let latest_evidence_item_at: Option<chrono::DateTime<chrono::Utc>> =
        row.try_get("latest_evidence_item_at").ok();
    let evidence_rows: i64 = row.try_get("evidence_rows").unwrap_or(0);
    let open_evidence: i64 = row.try_get("open_evidence").unwrap_or(0);
    let blocking_evidence: i64 = row.try_get("blocking_evidence").unwrap_or(0);
    let due_evidence: i64 = row.try_get("due_evidence").unwrap_or(0);
    let normalized_evidence_items: i64 = row.try_get("normalized_evidence_items").unwrap_or(0);

    let price_status = match price_session {
        Some(d) if d >= expected_price_session => "fresh",
        Some(_) => "stale",
        None => "missing",
    };
    let context_freshness = age_freshness(now, context_at, ChronoDuration::hours(12));
    let thesis_freshness = age_freshness(now, thesis_at, ChronoDuration::minutes(30));
    let evidence_freshness =
        age_freshness(now, latest_evidence_item_at, ChronoDuration::minutes(30));
    let source_blocked = [
        &price_health,
        &news_health,
        &estimates_health,
        &analyst_opinion_health,
        &profile_health,
        &earnings_health,
        &research_health,
        &fundamentals_health,
        &filings_health,
    ]
    .iter()
    .any(|s| source_is_blocked(s));
    let any_source_stale = [
        price_status,
        news_status.as_str(),
        estimates_status.as_str(),
        analyst_opinion_status.as_str(),
        profile_status.as_str(),
        earnings_status.as_str(),
        research_status.as_str(),
        filings_status.as_str(),
    ]
    .iter()
    .any(|s| matches!(*s, "stale" | "missing" | "rate_limited" | "failed"));
    let latest_source_task_outcome_at = task_rows
        .iter()
        .filter(|task| source_task_is_material(task))
        .filter_map(|task| task.updated_at)
        .max();
    let source_task_delta = latest_source_task_outcome_at.is_some_and(|at| {
        if let Some(thesis_at) = thesis_at {
            return at > thesis_at;
        }
        match latest_decline_at {
            Some(decline_at) => at > decline_at,
            None => true,
        }
    });
    let evidence_delta = latest_evidence_item_at.is_some_and(|at| {
        if let Some(thesis_at) = thesis_at {
            return at > thesis_at;
        }
        match latest_decline_at {
            Some(decline_at) => at > decline_at,
            None => true,
        }
    });

    let decision = decide(BrainDecisionInput {
        evidence_rows,
        open_evidence,
        blocking_evidence,
        due_evidence,
        source_task_delta,
        evidence_delta,
        has_context: context_at.is_some(),
        context_stale: context_freshness.as_str() == "stale",
        has_open_thesis: thesis_at.is_some(),
        thesis_stale: thesis_freshness.as_str() == "stale",
        any_source_stale,
        source_blocked,
    });
    let active_ticker = row.try_get::<bool, _>("active_ticker").unwrap_or(false);
    let (status, next_action, reason) = if active_ticker {
        (decision.status, decision.next_action, decision.reason)
    } else {
        (
            "not_monitored",
            "add_to_universe",
            "symbol is not in the active universe, so the scheduled brain loop will not run until it is confirmed or added",
        )
    };

    let attention_kinds = sqlx::query(
        r#"SELECT kind, count(*) AS n
             FROM attention_item
            WHERE symbol = $1 AND status = 'open'
              AND (
                fsm_state <> 'operator_deferred'
                OR (resurface_at IS NOT NULL AND resurface_at <= now())
              )
         GROUP BY kind
         ORDER BY n DESC, kind"#,
    )
    .bind(&symbol)
    .fetch_all(pool)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|r| {
        json!({
            "kind": r.try_get::<String, _>("kind").unwrap_or_default(),
            "count": r.try_get::<i64, _>("n").unwrap_or(0),
        })
    })
    .collect::<Vec<_>>();

    let cognition_runs = sqlx::query(
        r#"SELECT id, symbol, trigger, sweep_reason, status, reason,
                  context_version, thesis_id, thesis_classification,
                  evidence_open_count, evidence_blocking_count,
                  started_at, finished_at, next_retry_at, error, source_ref
             FROM cognition_run
            WHERE symbol = $1
         ORDER BY started_at DESC
            LIMIT 5"#,
    )
    .bind(&symbol)
    .fetch_all(pool)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(cognition_run_json)
    .collect::<Vec<_>>();
    let last_cognition_run = cognition_runs.first().cloned();

    let body = json!({
        "symbol": symbol,
        "as_of": now,
        "active_ticker": active_ticker,
        "status": status,
        "next_action": next_action,
        "reason": reason,
        "freshness_target_minutes": 30,
        "sources": [
            source_json("price", price_status, price_at, price_health, source_tasks_for(
                &task_rows,
                &["price_history"],
                &["fmp_price_backfill", "twse_price_backfill"],
            ), json!({
                "expected_latest_session": expected_price_session,
                "actual_latest_session": price_session,
            })),
            source_json("profile", &profile_status, profile_at, profile_health, source_tasks_for(
                &task_rows,
                &["company_profile"],
                &["fmp_company_profile"],
            ), json!({
                "company_profiles": row.try_get::<i64, _>("company_profiles").unwrap_or(0),
                "company_name": row.try_get::<Option<String>, _>("profile_company_name").ok().flatten(),
                "sector": row.try_get::<Option<String>, _>("profile_sector").ok().flatten(),
                "industry": row.try_get::<Option<String>, _>("profile_industry").ok().flatten(),
                "market_cap": row.try_get::<Option<f64>, _>("profile_market_cap").ok().flatten(),
            })),
            source_json("news", &news_status, news_at, news_health, source_tasks_for(
                &task_rows,
                &["recent_news"],
                &["fmp_news", "massive_news", "llm_sentiment_scoring"],
            ), json!({
                "latest_published_at": news_published_at,
            })),
            source_json("estimates", &estimates_status, estimates_at, estimates_health, source_tasks_for(
                &task_rows,
                &["analyst_estimates"],
                &["fmp_analyst_estimates"],
            ), json!({})),
            source_json("earnings", &earnings_status, earnings_at, earnings_health, source_tasks_for(
                &task_rows,
                &["earnings_calendar"],
                &["fmp_earnings_calendar"],
            ), json!({
                "earnings_events": row.try_get::<i64, _>("earnings_events").unwrap_or(0),
                "next_earnings_date": row.try_get::<Option<chrono::NaiveDate>, _>("next_earnings_date").ok().flatten(),
            })),
            source_json("analyst_opinion", &analyst_opinion_status, analyst_opinion_at, analyst_opinion_health, source_tasks_for(
                &task_rows,
                &["analyst_opinion"],
                &["fmp_price_target_consensus", "fmp_grades_historical", "fmp_price_target_news", "fmp_grades_latest_news"],
            ), json!({
                "price_target_snapshots": row.try_get::<i64, _>("price_target_snapshots").unwrap_or(0),
                "recommendation_snapshots": row.try_get::<i64, _>("recommendation_snapshots").unwrap_or(0),
                "price_target_events": row.try_get::<i64, _>("price_target_events").unwrap_or(0),
                "rating_events": row.try_get::<i64, _>("rating_events").unwrap_or(0),
            })),
            source_json("research", &research_status, research_at, research_health, source_tasks_for(
                &task_rows,
                &["product_research"],
                &["gdelt_doc_search", "bing_news_rss_search"],
            ), json!({})),
            source_json("fundamentals", &fundamentals_status, fundamentals_at, fundamentals_health, source_tasks_for(
                &task_rows,
                &["company_facts"],
                &["sec_company_tickers_cik_lookup", "sec_companyfacts_xbrl"],
            ), json!({})),
            source_json("filings", &filings_status, filings_at, filings_health, source_tasks_for(
                &task_rows,
                &["filing_metadata"],
                &["sec_edgar_submissions"],
            ), json!({})),
            json!({
                "source": "evidence",
                "status": evidence_freshness.as_str(),
                "last_changed_at": latest_evidence_item_at,
                "last_checked_at": latest_evidence_item_at,
                "max_age_minutes": 30,
                "detail": {
                    "normalized_items": normalized_evidence_items,
                    "evidence_delta": evidence_delta,
                    "latest_item_at": latest_evidence_item_at,
                },
            }),
            json!({
                "source": "context",
                "status": context_freshness.as_str(),
                "last_changed_at": context_at,
                "last_checked_at": context_at,
                "max_age_minutes": 720,
                "version": row.try_get::<Option<i32>, _>("context_version").ok().flatten(),
                "structural_as_of": row.try_get::<Option<chrono::DateTime<chrono::Utc>>, _>("structural_as_of").ok().flatten(),
                "narrative_as_of": row.try_get::<Option<chrono::DateTime<chrono::Utc>>, _>("narrative_as_of").ok().flatten(),
                "market_as_of": row.try_get::<Option<chrono::DateTime<chrono::Utc>>, _>("market_as_of").ok().flatten(),
            }),
            json!({
                "source": "thesis",
                "status": thesis_freshness.as_str(),
                "last_changed_at": thesis_updated_at,
                "last_checked_at": thesis_at,
                "max_age_minutes": 30,
                "thesis_id": row.try_get::<Option<uuid::Uuid>, _>("open_thesis_id").ok().flatten(),
                "state": row.try_get::<Option<String>, _>("open_thesis_state").ok().flatten(),
                "direction": row.try_get::<Option<String>, _>("open_thesis_direction").ok().flatten(),
            }),
        ],
        "evidence": {
            "rows": evidence_rows,
            "open": open_evidence,
            "blocking": blocking_evidence,
            "due": due_evidence,
            "items": normalized_evidence_items,
            "latest_item_at": latest_evidence_item_at,
            "delta": evidence_delta,
        },
        "attention": {
            "open": row.try_get::<i64, _>("open_attention").unwrap_or(0),
            "by_kind": attention_kinds,
        },
        "cognition": {
            "last_run": last_cognition_run,
            "recent_runs": cognition_runs,
        },
    });
    (StatusCode::OK, Json(body)).into_response()
}

fn cognition_run_json(r: sqlx::postgres::PgRow) -> serde_json::Value {
    let started_at: Option<chrono::DateTime<chrono::Utc>> = r.try_get("started_at").ok();
    let finished_at: Option<chrono::DateTime<chrono::Utc>> = r.try_get("finished_at").ok();
    let next_retry_at: Option<chrono::DateTime<chrono::Utc>> = r.try_get("next_retry_at").ok();
    json!({
        "id": r.try_get::<i64, _>("id").unwrap_or(0),
        "symbol": r.try_get::<String, _>("symbol").unwrap_or_default(),
        "trigger": r.try_get::<String, _>("trigger").unwrap_or_default(),
        "sweep_reason": r.try_get::<Option<String>, _>("sweep_reason").ok().flatten(),
        "status": r.try_get::<String, _>("status").unwrap_or_default(),
        "reason": r.try_get::<Option<String>, _>("reason").ok().flatten(),
        "context_version": r.try_get::<Option<i32>, _>("context_version").ok().flatten(),
        "thesis_id": r.try_get::<Option<uuid::Uuid>, _>("thesis_id").ok().flatten(),
        "thesis_classification": r.try_get::<Option<String>, _>("thesis_classification").ok().flatten(),
        "evidence_open_count": r.try_get::<i32, _>("evidence_open_count").unwrap_or(0),
        "evidence_blocking_count": r.try_get::<i32, _>("evidence_blocking_count").unwrap_or(0),
        "started_at": started_at,
        "finished_at": finished_at,
        "next_retry_at": next_retry_at,
        "error": r.try_get::<Option<String>, _>("error").ok().flatten(),
        "source_ref": r.try_get::<serde_json::Value, _>("source_ref").unwrap_or_else(|_| json!({})),
    })
}

#[derive(Debug, Deserialize)]
struct BrainJournalQuery {
    date: Option<NaiveDate>,
    page: Option<i64>,
    per_page: Option<i64>,
}

async fn get_brain_journal(
    State(gw): State<Arc<Gateway>>,
    Query(q): Query<BrainJournalQuery>,
) -> impl IntoResponse {
    let day = q.date.unwrap_or_else(|| chrono::Utc::now().date_naive());
    if let Err(e) = gw.store.refresh_brain_journal_entries(day).await {
        warn!(date = %day, error = %e, "refresh_brain_journal_entries failed");
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }
    let page = q.page.unwrap_or(1);
    let per_page = q.per_page.unwrap_or(50);
    match gw.store.brain_journal_for_date(day, page, per_page).await {
        Ok(body) => (StatusCode::OK, Json(body)).into_response(),
        Err(e) => {
            warn!(date = %day, error = %e, "get_brain_journal failed");
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
    }
}

fn source_health_effective_status(
    last_status: &str,
    last_started_at: Option<chrono::DateTime<chrono::Utc>>,
    now: chrono::DateTime<chrono::Utc>,
) -> (String, bool, Option<i64>) {
    let running_age_minutes = if last_status == "running" {
        last_started_at.map(|at| now.signed_duration_since(at).num_minutes().max(0))
    } else {
        None
    };
    let stale_running = running_age_minutes.is_some_and(|minutes| minutes > 15);
    let effective_status = if stale_running {
        "stale_running"
    } else {
        last_status
    };
    (
        effective_status.to_string(),
        stale_running,
        running_age_minutes,
    )
}

fn source_health_group(
    rows: &[SourceHealthSnapshot],
    names: &[&str],
    now: chrono::DateTime<chrono::Utc>,
    max_age: chrono::Duration,
) -> serde_json::Value {
    let matching = rows
        .iter()
        .filter(|r| names.contains(&r.source.as_str()))
        .collect::<Vec<_>>();
    let last_checked_at = matching
        .iter()
        .flat_map(|r| [r.last_success_at, r.last_started_at])
        .flatten()
        .max();
    let retry_after_at = matching.iter().filter_map(|r| r.retry_after_at).max();
    let last_error = matching.iter().find_map(|r| r.last_error.clone());
    let failure_kind = matching.iter().find_map(|r| r.last_failure_kind.clone());
    let has_fresh_running = matching.iter().any(|r| {
        r.last_status == "running"
            && !source_health_effective_status(&r.last_status, r.last_started_at, now).1
    });
    let status = if matching.is_empty() {
        "missing"
    } else if matching
        .iter()
        .any(|r| r.last_failure_kind.as_deref() == Some("rate_limited"))
    {
        "rate_limited"
    } else if matching.iter().any(|r| r.last_status == "failed") {
        "failed"
    } else if has_fresh_running {
        "running"
    } else {
        crate::platform::brain::age_freshness(now, last_checked_at, max_age).as_str()
    };
    json!({
        "status": status,
        "last_checked_at": last_checked_at,
        "retry_after_at": retry_after_at,
        "failure_kind": failure_kind,
        "last_error": last_error,
        "sources": names,
        "max_age_minutes": max_age.num_minutes(),
    })
}

fn source_status(health: &serde_json::Value) -> &str {
    health
        .get("status")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("missing")
}

fn source_is_blocked(health: &serde_json::Value) -> bool {
    matches!(source_status(health), "rate_limited" | "failed")
}

fn source_task_is_material(task: &SourceTaskSnapshot) -> bool {
    task.state == "satisfied" && task.result.as_deref() == Some("rows_seen")
}

fn source_tasks_for(
    rows: &[SourceTaskSnapshot],
    requirement_keys: &[&str],
    actions: &[&str],
) -> serde_json::Value {
    let tasks = rows
        .iter()
        .filter(|r| {
            r.requirement_key
                .as_deref()
                .is_some_and(|k| requirement_keys.contains(&k))
                || actions.contains(&r.action.as_str())
        })
        .map(|r| {
            json!({
                "requirement_key": r.requirement_key,
                "action": r.action,
                "provider": r.provider,
                "state": r.state,
                "result": r.result,
                "priority": r.priority,
                "due_at": r.due_at,
                "next_retry_at": r.next_retry_at,
                "attempts": r.attempts,
                "last_error": r.last_error,
                "updated_at": r.updated_at,
            })
        })
        .collect::<Vec<_>>();
    serde_json::Value::Array(tasks)
}

fn source_json(
    source: &str,
    status: &str,
    last_changed_at: Option<chrono::DateTime<chrono::Utc>>,
    health: serde_json::Value,
    source_tasks: serde_json::Value,
    detail: serde_json::Value,
) -> serde_json::Value {
    json!({
        "source": source,
        "status": status,
        "last_changed_at": last_changed_at,
        "last_checked_at": health.get("last_checked_at").cloned().unwrap_or(serde_json::Value::Null),
        "retry_after_at": health.get("retry_after_at").cloned().unwrap_or(serde_json::Value::Null),
        "failure_kind": health.get("failure_kind").cloned().unwrap_or(serde_json::Value::Null),
        "last_error": health.get("last_error").cloned().unwrap_or(serde_json::Value::Null),
        "max_age_minutes": health.get("max_age_minutes").cloned().unwrap_or(serde_json::Value::Null),
        "source_health": health,
        "source_tasks": source_tasks,
        "detail": detail,
    })
}

async fn list_attention_items(
    State(gw): State<Arc<Gateway>>,
    Query(q): Query<AttentionQuery>,
) -> impl IntoResponse {
    let status = q.status.unwrap_or_else(|| "open".to_string());
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    match gw.store.list_attention(&status, limit).await {
        Ok(rows) => (StatusCode::OK, Json(rows)).into_response(),
        Err(e) => {
            warn!(error = %e, "list_attention failed");
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
    }
}

#[derive(Debug, Deserialize, Default)]
struct DismissReq {
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct AttentionTransitionReq {
    to_state: String,
    #[serde(default)]
    owner: Option<String>,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    next_retry_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    resurface_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    source_ref: Option<serde_json::Value>,
}

async fn dismiss_attention_item(
    State(gw): State<Arc<Gateway>>,
    Path(id): Path<i64>,
    Json(req): Json<DismissReq>,
) -> impl IntoResponse {
    match gw.store.dismiss_attention(id, req.reason.as_deref()).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "not open").into_response(),
        Err(e) => {
            warn!(id, error = %e, "dismiss_attention failed");
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
    }
}

async fn transition_attention_item(
    State(gw): State<Arc<Gateway>>,
    Path(id): Path<i64>,
    Json(req): Json<AttentionTransitionReq>,
) -> impl IntoResponse {
    let to_state = req.to_state.trim();
    if !crate::attention::is_valid_fsm_state(to_state) {
        return (StatusCode::BAD_REQUEST, "invalid attention state").into_response();
    }
    let owner = req
        .owner
        .as_deref()
        .unwrap_or_else(|| crate::attention::default_owner_for_state(to_state));
    if !crate::attention::is_valid_owner(owner) {
        return (StatusCode::BAD_REQUEST, "invalid attention owner").into_response();
    }
    let resurface_at =
        if to_state == crate::attention::fsm::OPERATOR_DEFERRED && req.resurface_at.is_none() {
            Some(chrono::Utc::now() + chrono::Duration::days(7))
        } else {
            req.resurface_at
        };
    let reason = req.reason.as_deref().unwrap_or(to_state).trim().to_string();
    let source_ref = req.source_ref.unwrap_or_else(|| json!({ "source": "api" }));
    match gw
        .store
        .transition_attention(
            id,
            to_state,
            owner,
            &reason,
            req.next_retry_at,
            resurface_at,
            source_ref,
        )
        .await
    {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "not open").into_response(),
        Err(e) => {
            warn!(id, error = %e, "transition_attention failed");
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
    }
}

#[derive(Debug, Clone)]
struct DecisionThesisMeta {
    thesis_id: uuid::Uuid,
    symbol: String,
    cluster_id: String,
    state: crate::platform::domain::ThesisState,
    instrument: Option<String>,
    intended_size: serde_json::Value,
}

async fn load_decision_thesis(
    gw: &Gateway,
    thesis_id: uuid::Uuid,
) -> Result<Option<DecisionThesisMeta>, anyhow::Error> {
    let row = sqlx::query(
        r#"SELECT thesis_id, symbol, COALESCE(cluster_id, '') AS cluster_id,
                  state, instrument, COALESCE(intended_size, '{}'::jsonb) AS intended_size
             FROM thesis
            WHERE thesis_id = $1"#,
    )
    .bind(thesis_id)
    .fetch_optional(&gw.store.pool)
    .await?;

    let Some(row) = row else {
        return Ok(None);
    };
    let state_s: String = row.try_get("state")?;
    let state = serde_json::from_value(serde_json::Value::String(state_s))
        .map_err(|e| anyhow::anyhow!("decode ThesisState: {e}"))?;
    Ok(Some(DecisionThesisMeta {
        thesis_id: row.try_get("thesis_id")?,
        symbol: row.try_get("symbol")?,
        cluster_id: row.try_get("cluster_id")?,
        state,
        instrument: row.try_get("instrument").ok(),
        intended_size: row.try_get("intended_size")?,
    }))
}

fn sizing_object(
    sizing: Option<serde_json::Value>,
) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    match sizing.unwrap_or_else(|| json!({})) {
        serde_json::Value::Null => Ok(serde_json::Map::new()),
        serde_json::Value::Object(map) => Ok(map),
        _ => Err("sizing must be a JSON object".into()),
    }
}

fn sizing_string(map: &serde_json::Map<String, serde_json::Value>, key: &str) -> Option<String> {
    map.get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
}

fn sizing_f64(map: &serde_json::Map<String, serde_json::Value>, key: &str) -> Option<f64> {
    map.get(key).and_then(|v| v.as_f64())
}

fn validate_manual_fill(fill: &ManualFillReq) -> Result<(), String> {
    if !fill.qty.is_finite() || fill.qty <= 0.0 {
        return Err("manual fill qty must be positive".into());
    }
    if !fill.price.is_finite() || fill.price <= 0.0 {
        return Err("manual fill price must be positive".into());
    }
    let fees = fill.fees.unwrap_or(0.0);
    if !fees.is_finite() || fees < 0.0 {
        return Err("manual fill fees must be non-negative".into());
    }
    Ok(())
}

async fn risk_result_for_intent(
    gw: &Gateway,
    meta: &DecisionThesisMeta,
    instrument: &str,
    delta_notional: f64,
    premium_at_risk: f64,
) -> serde_json::Value {
    let Ok((cfg_json, cfg_ver)) = gw.store.active_config("risk").await else {
        return json!({"status": "unavailable", "reason": "risk config unavailable"});
    };
    let Ok(cfg) = serde_json::from_value::<crate::risk::Config>(cfg_json) else {
        return json!({"status": "unavailable", "reason": "risk config invalid"});
    };
    let positions = gw.store.open_positions_for_risk().await.unwrap_or_default();
    let settings = gw.store.portfolio_settings().await.unwrap_or_default();
    let realized_pnl = gw.store.realized_pnl_total().await.unwrap_or(0.0);
    let (portfolio, portfolio_demo) =
        match crate::risk::derive_portfolio(settings, &positions, realized_pnl) {
            Some(p) => (p, false),
            None => (
                crate::risk::Portfolio {
                    total_value: 100_000.0,
                    cash_pct: 50.0,
                    drawdown_pct: 0.0,
                },
                true,
            ),
        };
    let decision = crate::risk::evaluate(
        &crate::risk::Intent {
            symbol: meta.symbol.clone(),
            cluster: meta.cluster_id.clone(),
            instrument: instrument.to_owned(),
            delta_notional,
            premium_at_risk,
        },
        &positions,
        portfolio,
        &cfg,
    );
    json!({
        "status": if decision.veto { "veto" } else if decision.warnings.is_empty() { "pass" } else { "warning" },
        "veto": decision.veto,
        "reasons": decision.reasons,
        "warnings": decision.warnings,
        "size_mult": decision.size_mult,
        "config_version": cfg_ver,
        "portfolio_demo": portfolio_demo,
        "portfolio": {
            "total_value": portfolio.total_value,
            "cash_pct": portfolio.cash_pct,
            "drawdown_pct": portfolio.drawdown_pct,
        },
    })
}

async fn insert_decision_replay(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    decision_id: uuid::Uuid,
    meta: &DecisionThesisMeta,
    risk_result: &serde_json::Value,
    chart_range_seen: Option<&str>,
) -> Result<(), sqlx::Error> {
    let risk_verdict = if risk_result.is_null() {
        json!({})
    } else {
        risk_result.clone()
    };
    sqlx::query(
        r#"INSERT INTO decision_replay
                (decision_id, symbol, thesis_id, context_version, thesis_snapshot,
                 consensus_score, risk_verdict, evidence_ids, evidence_snapshot,
                 system_confidence, chart_range_seen, captured_at)
           SELECT $1,
                  $3,
                  $2,
                  (SELECT tc.version
                     FROM ticker_context tc
                    WHERE tc.symbol = $3
                 ORDER BY tc.version DESC
                    LIMIT 1),
                  COALESCE((SELECT to_jsonb(t) FROM thesis t WHERE t.thesis_id = $2), '{}'::jsonb),
                  (SELECT cs.score
                     FROM consensus_score cs
                    WHERE cs.symbol = $3
                 ORDER BY cs.computed_at DESC
                    LIMIT 1),
                  $4::jsonb,
                  COALESCE((
                    SELECT array_agg(ei.id ORDER BY COALESCE(te.weight, 0) DESC, ei.observed_at DESC, ei.id DESC)
                      FROM thesis_evidence te
                      JOIN evidence_item ei ON ei.id = te.evidence_id
                     WHERE te.thesis_id = $2
                  ), ARRAY[]::bigint[]),
                  COALESCE((
                    SELECT jsonb_agg(
                             jsonb_build_object(
                               'id', ei.id,
                               'symbol', ei.symbol,
                               'kind', ei.kind,
                               'observed_at', ei.observed_at,
                               'source', ei.source,
                               'source_id', ei.source_id,
                               'source_ref', ei.source_ref,
                               'summary', ei.summary,
                               'strength', ei.strength,
                               'polarity', ei.polarity,
                               'url', ei.url,
                               'created_at', ei.created_at,
                               'updated_at', ei.updated_at,
                               'weight', te.weight,
                               'added_by', te.added_by
                             )
                             ORDER BY COALESCE(te.weight, 0) DESC, ei.observed_at DESC, ei.id DESC
                           )
                      FROM thesis_evidence te
                      JOIN evidence_item ei ON ei.id = te.evidence_id
                     WHERE te.thesis_id = $2
                  ), '[]'::jsonb),
                  COALESCE(
                    NULLIF((SELECT t.system_confidence FROM thesis t WHERE t.thesis_id = $2), ''),
                    NULLIF((SELECT t.forecast->>'system_confidence' FROM thesis t WHERE t.thesis_id = $2), ''),
                    NULLIF((SELECT t.forecast->>'confidence' FROM thesis t WHERE t.thesis_id = $2), ''),
                    NULLIF((SELECT t.conviction_tier FROM thesis t WHERE t.thesis_id = $2), '')
                  ),
                  $5,
                  now()
        ON CONFLICT (decision_id) DO NOTHING"#,
    )
    .bind(decision_id)
    .bind(meta.thesis_id)
    .bind(&meta.symbol)
    .bind(risk_verdict)
    .bind(chart_range_seen)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn record_decision(
    State(gw): State<Arc<Gateway>>,
    Json(req): Json<DecisionReq>,
) -> impl IntoResponse {
    match record_decision_inner(&gw, req).await {
        Ok(body) => (StatusCode::OK, Json(body)).into_response(),
        Err((status, msg)) => (status, msg).into_response(),
    }
}

async fn record_decision_inner(
    gw: &Gateway,
    req: DecisionReq,
) -> Result<serde_json::Value, (StatusCode, String)> {
    use crate::platform::domain::ThesisState;

    let action = req.action.trim().to_ascii_lowercase();
    if !matches!(action.as_str(), "enter" | "exit" | "skip" | "resize") {
        return Err((StatusCode::BAD_REQUEST, "invalid decision action".into()));
    }

    let mut sizing_map =
        sizing_object(req.sizing.clone()).map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    let thesis_uuid: Option<uuid::Uuid> = if req.thesis_id.trim().is_empty() {
        None
    } else {
        Some(
            uuid::Uuid::parse_str(req.thesis_id.trim())
                .map_err(|_| (StatusCode::BAD_REQUEST, "invalid thesis_id".to_string()))?,
        )
    };

    let thesis_meta = if let Some(tid) = thesis_uuid {
        Some(
            load_decision_thesis(gw, tid)
                .await
                .map_err(|e| {
                    warn!(error = %e, "load decision thesis failed");
                    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
                })?
                .ok_or_else(|| (StatusCode::NOT_FOUND, format!("thesis {tid} not found")))?,
        )
    } else {
        None
    };

    if let Some(fill) = req.manual_fill.as_ref() {
        validate_manual_fill(fill).map_err(|e| (StatusCode::BAD_REQUEST, e))?;
        let Some(meta) = thesis_meta.as_ref() else {
            return Err((
                StatusCode::BAD_REQUEST,
                "manual fills must be linked to a thesis".into(),
            ));
        };
        match (action.as_str(), meta.state) {
            ("enter", ThesisState::Actionable) => {}
            ("resize", ThesisState::PositionOpen) => {}
            ("exit", ThesisState::PositionOpen | ThesisState::Exiting) => {}
            ("enter", _) => {
                return Err((
                    StatusCode::BAD_REQUEST,
                    format!(
                        "manual enter fill requires actionable thesis, got {}",
                        meta.state.as_str()
                    ),
                ));
            }
            ("resize", _) => {
                return Err((
                    StatusCode::BAD_REQUEST,
                    format!(
                        "manual resize fill requires position_open thesis, got {}",
                        meta.state.as_str()
                    ),
                ));
            }
            ("exit", _) => {
                return Err((
                    StatusCode::BAD_REQUEST,
                    format!(
                        "manual exit fill requires position_open/exiting thesis, got {}",
                        meta.state.as_str()
                    ),
                ));
            }
            _ => {
                return Err((
                    StatusCode::BAD_REQUEST,
                    "manual fills are only supported for enter, resize, and exit decisions".into(),
                ));
            }
        }
    }

    let sizing_side = sizing_string(&sizing_map, "side");
    let sizing_instrument = sizing_string(&sizing_map, "instrument");
    let fill_side = req.manual_fill.as_ref().and_then(|f| f.side.clone());
    let fill_instrument = req.manual_fill.as_ref().and_then(|f| f.instrument.clone());
    let side = fill_side.or(sizing_side).filter(|s| s != "none");
    let instrument = fill_instrument
        .or(sizing_instrument)
        .or_else(|| thesis_meta.as_ref().and_then(|m| m.instrument.clone()))
        .unwrap_or_else(|| "equity".to_string());

    if let Some(side) = side.as_deref() {
        if !crate::execution::allowed_side(side) {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("invalid trade side: {side}"),
            ));
        }
        sizing_map.insert("side".into(), json!(side));
    }
    if !crate::execution::allowed_instrument(&instrument) {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("invalid trade instrument: {instrument}"),
        ));
    }
    if matches!(action.as_str(), "enter" | "resize") {
        sizing_map.insert("instrument".into(), json!(instrument.clone()));
    }

    let exposure = if let Some(fill) = req.manual_fill.as_ref() {
        let fill_side = side.clone().ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                "manual enter/resize/exit fill needs side".to_string(),
            )
        })?;
        Some(crate::execution::default_exposure(
            &fill_side,
            &instrument,
            fill.qty,
            fill.price,
            fill.delta_notional,
            fill.premium_at_risk,
        ))
    } else {
        let delta = sizing_f64(&sizing_map, "delta_notional").unwrap_or(0.0);
        let premium = sizing_f64(&sizing_map, "premium_at_risk").unwrap_or(0.0);
        if delta > 0.0 || premium > 0.0 {
            Some(crate::execution::FillExposure {
                delta_notional: delta,
                premium_at_risk: premium,
                multiplier: crate::execution::contract_multiplier(&instrument),
            })
        } else {
            None
        }
    };

    let should_create_ticket =
        thesis_meta.is_some() && matches!(action.as_str(), "enter" | "exit" | "resize");
    let mut risk_result = serde_json::Value::Null;
    if matches!(action.as_str(), "enter" | "resize") {
        if let (Some(meta), Some(exposure)) = (thesis_meta.as_ref(), exposure) {
            risk_result = risk_result_for_intent(
                gw,
                meta,
                &instrument,
                exposure.delta_notional,
                exposure.premium_at_risk,
            )
            .await;
            sizing_map.insert("risk_result".into(), risk_result.clone());
            sizing_map.insert("delta_notional".into(), json!(exposure.delta_notional));
            sizing_map.insert("premium_at_risk".into(), json!(exposure.premium_at_risk));
        }
    }

    let sizing_value = if sizing_map.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::Value::Object(sizing_map.clone())
    };
    let user_choice = req.user_choice.trim().to_ascii_lowercase();
    let user_choice_db = if user_choice.is_empty() {
        None
    } else {
        Some(user_choice.as_str())
    };
    let (disagreement_reason, disagreement_detail) = normalize_disagreement(
        &action,
        &user_choice,
        &req.disagreement_reason,
        &req.disagreement_detail,
    )
    .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    let human_conviction = normalize_human_conviction(&req.human_conviction)
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    let decision_reason = req.reason.trim().to_string();
    let decision_reason_db = if decision_reason.is_empty() {
        None
    } else {
        Some(decision_reason.as_str())
    };

    let mut tx = gw.store.pool.begin().await.map_err(|e| {
        warn!(error = %e, "record_decision begin failed");
        (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
    })?;

    let decision_id: uuid::Uuid = sqlx::query_scalar(
        r#"INSERT INTO decision
                (thesis_id, action, user_choice, sizing,
                 disagreement_reason, disagreement_detail,
                 human_conviction, reason)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
       RETURNING decision_id"#,
    )
    .bind(thesis_uuid)
    .bind(&action)
    .bind(user_choice_db)
    .bind(&sizing_value)
    .bind(disagreement_reason.as_deref())
    .bind(disagreement_detail.as_deref())
    .bind(&human_conviction)
    .bind(decision_reason_db)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| {
        warn!(error = %e, "record_decision insert failed");
        (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
    })?;

    if let Some(meta) = thesis_meta.as_ref() {
        let chart_range_seen = req
            .chart_range_seen
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        insert_decision_replay(&mut tx, decision_id, meta, &risk_result, chart_range_seen)
            .await
            .map_err(|e| {
                warn!(error = %e, "decision_replay insert failed");
                (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
            })?;
    }

    let mut ticket_id: Option<uuid::Uuid> = None;
    if should_create_ticket {
        let meta = thesis_meta.as_ref().expect("checked above");
        let status = if req.manual_fill.is_some() {
            "filled"
        } else if user_choice == "rejected" {
            "rejected"
        } else if user_choice == "confirmed" {
            "accepted"
        } else {
            "proposed"
        };
        let ticket: uuid::Uuid = sqlx::query_scalar(
            r#"INSERT INTO trade_ticket
                    (thesis_id, decision_id, symbol, action, side, instrument,
                     intended_size, risk_result, status)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
           RETURNING ticket_id"#,
        )
        .bind(meta.thesis_id)
        .bind(decision_id)
        .bind(&meta.symbol)
        .bind(&action)
        .bind(side.as_deref())
        .bind(Some(instrument.as_str()))
        .bind(&meta.intended_size)
        .bind(if risk_result.is_null() {
            json!({})
        } else {
            risk_result.clone()
        })
        .bind(status)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| {
            warn!(error = %e, "trade_ticket insert failed");
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        })?;
        ticket_id = Some(ticket);
    }

    let mut position_id: Option<uuid::Uuid> = None;
    let mut fill_id: Option<uuid::Uuid> = None;
    let mut transitioned_to: Option<&'static str> = None;
    if let Some(fill) = req.manual_fill.as_ref() {
        let meta = thesis_meta.as_ref().expect("manual fill requires thesis");
        let fill_side = side.as_deref().expect("manual fill side validated");
        let fees = fill.fees.unwrap_or(0.0);
        let filled_at = fill.filled_at.unwrap_or_else(chrono::Utc::now);
        let exposure = exposure.expect("manual fill exposure exists");

        match action.as_str() {
            "enter" => {
                let pos_id: uuid::Uuid = sqlx::query_scalar(
                    r#"INSERT INTO position
                            (thesis_id, symbol, side, instrument, qty, avg_price,
                             delta_notional, premium_at_risk, opened_at)
                       VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
                   RETURNING position_id"#,
                )
                .bind(meta.thesis_id)
                .bind(&meta.symbol)
                .bind(fill_side)
                .bind(&instrument)
                .bind(fill.qty)
                .bind(fill.price)
                .bind(exposure.delta_notional)
                .bind(exposure.premium_at_risk)
                .bind(filled_at)
                .fetch_one(&mut *tx)
                .await
                .map_err(|e| {
                    warn!(error = %e, "position insert failed");
                    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
                })?;
                position_id = Some(pos_id);
            }
            "resize" | "exit" => {
                let explicit_position = fill
                    .position_id
                    .as_deref()
                    .map(uuid::Uuid::parse_str)
                    .transpose()
                    .map_err(|_| (StatusCode::BAD_REQUEST, "invalid position_id".to_string()))?;
                let row = if let Some(pid) = explicit_position {
                    sqlx::query(
                        r#"SELECT position_id, side, instrument, qty::float8 AS qty,
                                  avg_price::float8 AS avg_price,
                                  COALESCE(delta_notional, 0)::float8 AS delta_notional,
                                  COALESCE(premium_at_risk, 0)::float8 AS premium_at_risk
                             FROM position
                            WHERE position_id = $1
                              AND thesis_id = $2
                              AND closed_at IS NULL
                            FOR UPDATE"#,
                    )
                    .bind(pid)
                    .bind(meta.thesis_id)
                    .fetch_optional(&mut *tx)
                    .await
                } else {
                    sqlx::query(
                        r#"SELECT position_id, side, instrument, qty::float8 AS qty,
                                  avg_price::float8 AS avg_price,
                                  COALESCE(delta_notional, 0)::float8 AS delta_notional,
                                  COALESCE(premium_at_risk, 0)::float8 AS premium_at_risk
                             FROM position
                            WHERE thesis_id = $1
                              AND closed_at IS NULL
                         ORDER BY opened_at DESC
                            LIMIT 1
                            FOR UPDATE"#,
                    )
                    .bind(meta.thesis_id)
                    .fetch_optional(&mut *tx)
                    .await
                }
                .map_err(|e| {
                    warn!(error = %e, "open position lookup failed");
                    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
                })?;

                let Some(row) = row else {
                    return Err((
                        StatusCode::BAD_REQUEST,
                        "no open position found for thesis".into(),
                    ));
                };
                let pos_id: uuid::Uuid = row
                    .try_get("position_id")
                    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
                let existing_side: String = row.try_get("side").unwrap_or_else(|_| "long".into());
                let existing_instrument: String = row
                    .try_get("instrument")
                    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
                if fill_side != existing_side || instrument != existing_instrument {
                    return Err((
                        StatusCode::BAD_REQUEST,
                        "manual fill side/instrument must match the open position".into(),
                    ));
                }
                position_id = Some(pos_id);

                let open_qty: f64 = row.try_get("qty").unwrap_or(0.0);
                let avg_price: f64 = row.try_get("avg_price").unwrap_or(0.0);
                let old_delta: f64 = row.try_get("delta_notional").unwrap_or(0.0);
                let old_premium: f64 = row.try_get("premium_at_risk").unwrap_or(0.0);
                if action == "resize" {
                    let new_qty = open_qty + fill.qty;
                    let new_avg = ((open_qty * avg_price) + (fill.qty * fill.price)) / new_qty;
                    sqlx::query(
                        r#"UPDATE position
                              SET qty = $2,
                                  avg_price = $3,
                                  delta_notional = $4,
                                  premium_at_risk = $5
                            WHERE position_id = $1"#,
                    )
                    .bind(pos_id)
                    .bind(new_qty)
                    .bind(new_avg)
                    .bind(old_delta + exposure.delta_notional)
                    .bind(old_premium + exposure.premium_at_risk)
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| {
                        warn!(error = %e, "position resize update failed");
                        (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
                    })?;
                } else {
                    if fill.qty + f64::EPSILON < open_qty {
                        return Err((
                            StatusCode::BAD_REQUEST,
                            "partial exits are not supported yet; fill qty must close the position"
                                .into(),
                        ));
                    }
                    let pnl = crate::execution::realized_pnl(
                        &existing_side,
                        open_qty,
                        avg_price,
                        fill.price,
                        fees,
                        crate::execution::contract_multiplier(&existing_instrument),
                    );
                    sqlx::query(
                        r#"UPDATE position
                              SET closed_at = $2,
                                  realized_pnl = $3
                            WHERE position_id = $1"#,
                    )
                    .bind(pos_id)
                    .bind(filled_at)
                    .bind(pnl)
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| {
                        warn!(error = %e, "position close update failed");
                        (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
                    })?;
                }
            }
            _ => unreachable!("manual fill actions validated above"),
        }

        let pos_id = position_id.expect("position_id set for manual fill");
        let inserted_fill: uuid::Uuid = sqlx::query_scalar(
            r#"INSERT INTO position_fill
                    (position_id, ticket_id, decision_id, thesis_id, symbol, side,
                     instrument, qty, price, fees, filled_at, source, notes, raw)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, 'manual', $12, $13)
           RETURNING fill_id"#,
        )
        .bind(pos_id)
        .bind(ticket_id)
        .bind(decision_id)
        .bind(meta.thesis_id)
        .bind(&meta.symbol)
        .bind(fill_side)
        .bind(&instrument)
        .bind(fill.qty)
        .bind(fill.price)
        .bind(fees)
        .bind(filled_at)
        .bind(fill.notes.as_deref())
        .bind(json!({
            "delta_notional": exposure.delta_notional,
            "premium_at_risk": exposure.premium_at_risk,
            "multiplier": exposure.multiplier,
        }))
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| {
            warn!(error = %e, "position_fill insert failed");
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        })?;
        fill_id = Some(inserted_fill);
    }

    tx.commit().await.map_err(|e| {
        warn!(error = %e, "record_decision commit failed");
        (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
    })?;

    if let Some(tid) = thesis_uuid {
        if let Err(e) = gw
            .store
            .resolve_attention(
                "thesis_actionable",
                Some(tid),
                None,
                &format!("decision_recorded:{action}"),
                json!({
                    "action": action,
                    "user_choice": user_choice,
                    "disagreement_reason": disagreement_reason.clone(),
                    "human_conviction": human_conviction.clone(),
                }),
            )
            .await
        {
            warn!(error = %e, "attention resolve failed (non-fatal)");
        }
    }

    if action == "enter" && req.manual_fill.is_some() {
        if let Some(meta) = thesis_meta.as_ref() {
            if meta.state == ThesisState::Actionable {
                gw.store
                    .apply_state_transition(
                        meta.thesis_id,
                        ThesisState::Actionable,
                        ThesisState::PositionOpen,
                        "manual fill recorded",
                    )
                    .await
                    .map_err(|e| {
                        warn!(error = %e, "position_open transition failed");
                        (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
                    })?;
                transitioned_to = Some("position_open");
                let payload = json!({
                    "thesis_id": meta.thesis_id,
                    "symbol": meta.symbol.clone(),
                    "from": "actionable",
                    "to": "position_open",
                    "rationale": "manual fill recorded",
                    "at": chrono::Utc::now(),
                });
                if let Err(e) = gw
                    .bus
                    .publish(subjects::THESIS_UPDATED, payload.to_string().as_bytes())
                    .await
                {
                    warn!(error = %e, "thesis position_open publish failed (best-effort)");
                }
            }
        }
    }

    let env = json!({
        "thesis_id": thesis_uuid,
        "decision_id": decision_id,
        "ticket_id": ticket_id,
        "position_id": position_id,
        "fill_id": fill_id,
        "action": action,
        "user_choice": user_choice,
        "disagreement_reason": disagreement_reason.clone(),
        "disagreement_detail": disagreement_detail.clone(),
        "human_conviction": human_conviction.clone(),
        "reason": decision_reason_db,
        "sizing": sizing_value,
    });
    if let Err(e) = gw
        .bus
        .publish(subjects::DECISION_RECORDED, env.to_string().as_bytes())
        .await
    {
        warn!(error = %e, "decision publish failed (best-effort)");
    }
    Ok(json!({
        "decision_id": decision_id,
        "ticket_id": ticket_id,
        "position_id": position_id,
        "fill_id": fill_id,
        "risk_result": risk_result,
        "transitioned_to": transitioned_to,
        "disagreement_reason": disagreement_reason,
        "disagreement_detail": disagreement_detail,
        "human_conviction": human_conviction,
        "reason": decision_reason_db,
    }))
}

async fn spa_handler(State(gw): State<Arc<Gateway>>, uri: axum::http::Uri) -> impl IntoResponse {
    // Dev mode: anything that ISN'T an /api route lands here. Bounce the
    // browser to the live Vite dev server so :8080 stops competing with :5173.
    // API paths reach their dedicated handlers and never hit this fallback.
    if let Some(target) = gw.dev_redirect.as_deref() {
        let path = uri.path();
        let dest = if path == "/" || path.is_empty() {
            target.to_string()
        } else {
            format!("{}{}", target.trim_end_matches('/'), path)
        };
        return (
            StatusCode::FOUND,
            [(header::LOCATION, dest)],
            "SPA served by Vite dev server in dev mode — redirecting.",
        )
            .into_response();
    }

    let path = uri.path().trim_start_matches('/');
    let asset_path = if path.is_empty() { "index.html" } else { path };
    if let Some(file) = Dist::get(asset_path) {
        return (
            StatusCode::OK,
            [(header::CONTENT_TYPE, file.metadata.mimetype().to_string())],
            file.data,
        )
            .into_response();
    }
    // Client-routing fallback: serve index.html for unknown paths.
    if let Some(index) = Dist::get("index.html") {
        return (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/html; charset=utf-8".to_string())],
            index.data,
        )
            .into_response();
    }
    (StatusCode::NOT_FOUND, "not built").into_response()
}

// ---------- watchlists (#54) ----------

async fn list_watchlists(State(gw): State<Arc<Gateway>>) -> impl IntoResponse {
    match gw.store.list_watchlists().await {
        Ok(v) => (StatusCode::OK, Json(v)).into_response(),
        Err(e) => {
            warn!(error = %e, "list_watchlists failed");
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
    }
}

#[derive(Debug, Deserialize)]
struct CreateWatchlistReq {
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    color: Option<String>,
}

async fn create_watchlist(
    State(gw): State<Arc<Gateway>>,
    Json(req): Json<CreateWatchlistReq>,
) -> impl IntoResponse {
    if req.name.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "name required").into_response();
    }
    match gw
        .store
        .create_watchlist(
            req.name.trim(),
            req.description.as_deref(),
            req.color.as_deref(),
        )
        .await
    {
        Ok(id) => (StatusCode::CREATED, Json(serde_json::json!({"id": id}))).into_response(),
        Err(e) => {
            warn!(name = %req.name, error = %e, "create_watchlist failed");
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
    }
}

async fn delete_watchlist(
    State(gw): State<Arc<Gateway>>,
    Path(id): Path<uuid::Uuid>,
) -> impl IntoResponse {
    match gw.store.delete_watchlist(id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "no such non-system watchlist").into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn list_watchlist_members(
    State(gw): State<Arc<Gateway>>,
    Path(id): Path<uuid::Uuid>,
) -> impl IntoResponse {
    match gw.store.list_watchlist_members(id).await {
        Ok(v) => (StatusCode::OK, Json(v)).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[derive(Debug, Deserialize)]
struct AddMemberReq {
    symbol: String,
    #[serde(default = "default_added_by")]
    added_by: String,
}

fn default_added_by() -> String {
    "user".to_string()
}

async fn add_watchlist_member(
    State(gw): State<Arc<Gateway>>,
    Path(id): Path<uuid::Uuid>,
    Json(req): Json<AddMemberReq>,
) -> impl IntoResponse {
    let symbol = req.symbol.trim().to_uppercase();
    if symbol.is_empty() {
        return (StatusCode::BAD_REQUEST, "symbol required").into_response();
    }
    match gw.store.add_to_watchlist(id, &symbol, &req.added_by).await {
        Ok(()) => {
            let payload = serde_json::json!({
                "symbol": symbol,
                "watchlist_id": id,
                "source": "watchlist.added",
            });
            if let Err(e) = gw
                .bus
                .publish(
                    subjects::DISCOVERY_CONFIRMED,
                    payload.to_string().as_bytes(),
                )
                .await
            {
                warn!(id = %id, symbol = %symbol, error = %e, "publish watchlist cognition kickoff failed (non-fatal)");
            }
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => {
            warn!(id = %id, symbol = %symbol, error = %e, "add_watchlist_member failed");
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
    }
}

async fn remove_watchlist_member(
    State(gw): State<Arc<Gateway>>,
    Path((id, symbol)): Path<(uuid::Uuid, String)>,
) -> impl IntoResponse {
    match gw
        .store
        .remove_from_watchlist(id, &symbol.to_uppercase())
        .await
    {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "not in this list").into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ---------- portfolio settings (#26) ----------

async fn get_portfolio(State(gw): State<Arc<Gateway>>) -> impl IntoResponse {
    let settings = match gw.store.portfolio_settings().await {
        Ok(s) => s,
        Err(e) => {
            warn!(error = %e, "get_portfolio failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }
    };
    let realized = gw.store.realized_pnl_total().await.unwrap_or(0.0);
    let open = gw.store.open_positions_for_risk().await.unwrap_or_default();
    let derived = match settings.account_size_usd {
        Some(_) => crate::risk::derive_portfolio(settings, &open, realized),
        None => None,
    };
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "account_size_usd":    settings.account_size_usd,
            "high_water_mark_usd": settings.high_water_mark_usd,
            "realized_pnl_total":  realized,
            "configured":          settings.account_size_usd.is_some(),
            "derived":             derived.map(|p| serde_json::json!({
                "total_value":  p.total_value,
                "cash_pct":     p.cash_pct,
                "drawdown_pct": p.drawdown_pct,
            })),
        })),
    )
        .into_response()
}

#[derive(Debug, Deserialize)]
struct PutPortfolioReq {
    #[serde(default)]
    account_size_usd: Option<f64>,
    #[serde(default)]
    high_water_mark_usd: Option<f64>,
}

async fn put_portfolio(
    State(gw): State<Arc<Gateway>>,
    Json(req): Json<PutPortfolioReq>,
) -> impl IntoResponse {
    if req.account_size_usd.is_none() && req.high_water_mark_usd.is_none() {
        return (
            StatusCode::BAD_REQUEST,
            "at least one of account_size_usd / high_water_mark_usd required",
        )
            .into_response();
    }
    if let Some(v) = req.account_size_usd
        && v <= 0.0
    {
        return (StatusCode::BAD_REQUEST, "account_size_usd must be > 0").into_response();
    }
    if let Some(v) = req.high_water_mark_usd
        && v <= 0.0
    {
        return (StatusCode::BAD_REQUEST, "high_water_mark_usd must be > 0").into_response();
    }
    match gw
        .store
        .upsert_portfolio_settings(req.account_size_usd, req.high_water_mark_usd, "user")
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            warn!(error = %e, "put_portfolio failed");
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
    }
}

// ---------- discovery review (#54 phase B / #55) ----------

async fn list_pending_candidates(State(gw): State<Arc<Gateway>>) -> impl IntoResponse {
    match gw.store.pending_discovery_candidates().await {
        Ok(v) => (StatusCode::OK, Json(v)).into_response(),
        Err(e) => {
            warn!(error = %e, "list_pending_candidates failed");
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
    }
}

#[derive(Debug, Deserialize)]
struct ConfirmCandidateReq {
    #[serde(default)]
    watchlist_ids: Vec<uuid::Uuid>,
}

async fn confirm_candidate(
    State(gw): State<Arc<Gateway>>,
    Path(id): Path<i64>,
    Json(req): Json<ConfirmCandidateReq>,
) -> impl IntoResponse {
    // Look up the candidate's symbol so we can fire discovery.confirmed.
    let symbol: Option<String> =
        sqlx::query_scalar("SELECT symbol FROM discovery_candidate WHERE id = $1")
            .bind(id)
            .fetch_optional(&gw.store.pool)
            .await
            .ok()
            .flatten();
    if let Err(e) = gw
        .store
        .confirm_discovery_candidate(id, &req.watchlist_ids)
        .await
    {
        warn!(id, error = %e, "confirm_candidate failed");
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }
    // Auto-kick cognition pipeline (#100) by publishing discovery.confirmed.
    // The cognition service consumes this and runs context+thesis for the
    // newly-promoted ticker — no manual `make refresh-context` step needed.
    if let Some(sym) = symbol {
        let payload = serde_json::json!({
            "candidate_id": id,
            "symbol": sym,
            "watchlist_ids": req.watchlist_ids,
        });
        if let Err(e) = gw
            .bus
            .publish(
                subjects::DISCOVERY_CONFIRMED,
                payload.to_string().as_bytes(),
            )
            .await
        {
            warn!(error = %e, "publish discovery.confirmed failed (non-fatal)");
        }
    }
    StatusCode::NO_CONTENT.into_response()
}

/// Operator-triggered: re-run the cognition pipeline for SYMBOL. Used when
/// the UI opens a ticker that has no `ticker_context` yet, or when the
/// operator wants to refresh stale context. Reuses the `discovery.confirmed`
/// subject so the existing cognition consumer handles it — `candidate_id` is
/// optional in the consumer's payload schema.
async fn trigger_refresh_context(
    State(gw): State<Arc<Gateway>>,
    Path(symbol): Path<String>,
) -> impl IntoResponse {
    let sym = symbol.to_ascii_uppercase();
    if sym.is_empty() || sym.len() > 14 {
        return (StatusCode::BAD_REQUEST, "invalid symbol").into_response();
    }
    let payload = serde_json::json!({"symbol": sym, "source": "ui-refresh"});
    if let Err(e) = gw
        .bus
        .publish(
            subjects::DISCOVERY_CONFIRMED,
            payload.to_string().as_bytes(),
        )
        .await
    {
        warn!(symbol = %sym, error = %e, "publish refresh-context failed");
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }
    StatusCode::ACCEPTED.into_response()
}

async fn reject_candidate(
    State(gw): State<Arc<Gateway>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match gw.store.reject_discovery_candidate(id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "no such candidate").into_response(),
        Err(e) => {
            warn!(id, error = %e, "reject_candidate failed");
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::domain::{ThesisDetail, ThesisState};
    use chrono::{TimeZone, Utc};
    use serde_json::json;

    fn thesis_fixture(thesis_id: uuid::Uuid) -> ThesisDetail {
        ThesisDetail {
            thesis_id,
            symbol: "NVDA".to_string(),
            cluster_id: Some("ai".to_string()),
            cluster_thesis: None,
            parent_themes: json!([]),
            state: ThesisState::Armed,
            edge_rationale: "edge".to_string(),
            bull_case: None,
            bear_case: None,
            forecast: json!({
                "direction": "up",
                "target": 220,
                "horizon_days": 180
            }),
            conviction_conditions: json!([]),
            trigger_conditions: json!([]),
            invalidation_conditions: json!([]),
            fulfillment_conditions: json!([]),
            known_unknowns: json!([]),
            conviction_tier: Some("high".to_string()),
            system_confidence: Some("high".to_string()),
            system_confidence_components: json!({
                "evidence_strength": "strong"
            }),
            instrument: Some("LEAPS".to_string()),
            intended_size: json!({ "pct": 0.04 }),
            version: 3,
            immutable_original: json!({}),
            created_at: Utc.with_ymd_and_hms(2026, 6, 1, 12, 0, 0).unwrap(),
            updated_at: Utc.with_ymd_and_hms(2026, 6, 1, 12, 0, 0).unwrap(),
            last_evaluated_at: None,
            history: vec![],
            evidence_items: vec![],
            substance: None,
        }
    }

    #[test]
    fn disagreement_reason_is_required_for_skip_or_reject() {
        let skip = normalize_disagreement("skip", "deferred", "", "");
        assert!(skip.is_err());

        let rejected = normalize_disagreement("enter", "rejected", "", "");
        assert!(rejected.is_err());

        let accepted = normalize_disagreement("enter", "confirmed", "", "").unwrap();
        assert_eq!(accepted, (None, None));
    }

    #[test]
    fn disagreement_reason_validates_reason_and_other_detail() {
        let ok = normalize_disagreement(
            "skip",
            "deferred",
            "valuation_priced",
            "story is true but already reflected",
        )
        .unwrap();
        assert_eq!(
            ok,
            (
                Some("valuation_priced".to_string()),
                Some("story is true but already reflected".to_string())
            )
        );

        assert!(normalize_disagreement("skip", "deferred", "other", "").is_err());
        assert!(normalize_disagreement("skip", "deferred", "not_a_reason", "").is_err());
    }

    #[test]
    fn transition_payload_includes_prediction_context() {
        let thesis_id = uuid::Uuid::parse_str("00000000-0000-0000-0000-000000000123").unwrap();
        let thesis = thesis_fixture(thesis_id);
        let at = Utc.with_ymd_and_hms(2026, 6, 1, 13, 0, 0).unwrap();

        let payload = thesis_transition_event_payload(
            thesis_id,
            &thesis,
            ThesisState::Actionable,
            "ready",
            at,
        );

        assert_eq!(payload["thesis_id"], json!(thesis_id));
        assert_eq!(payload["symbol"], "NVDA");
        assert_eq!(payload["cluster_id"], "ai");
        assert_eq!(payload["from"], "armed");
        assert_eq!(payload["to"], "actionable");
        assert_eq!(payload["forecast"]["direction"], "up");
        assert_eq!(payload["forecast"]["horizon_days"], 180);
        assert_eq!(payload["conviction_tier"], "high");
        assert_eq!(payload["system_confidence"], "high");
        assert_eq!(
            payload["system_confidence_components"]["evidence_strength"],
            "strong"
        );
        assert_eq!(payload["instrument"], "LEAPS");
        assert_eq!(payload["intended_size"]["pct"], 0.04);
    }

    #[test]
    fn human_conviction_is_required_and_validated() {
        assert_eq!(
            normalize_human_conviction(" Medium ").unwrap(),
            "medium".to_string()
        );
        assert!(normalize_human_conviction("").is_err());
        assert!(normalize_human_conviction("very_high").is_err());
    }

    #[test]
    fn chat_scope_classifies_technical_questions() {
        assert_eq!(
            classify_chat_scope("Is ENTG overextended above the 200-day SMA?", Some("ENTG")),
            "technical"
        );
        assert_eq!(
            classify_chat_scope("Should I resize this position?", Some("ENTG")),
            "decision"
        );
        assert_eq!(classify_chat_scope("What do rates imply?", None), "macro");
    }

    #[test]
    fn chat_requested_evidence_is_canonicalized() {
        let req = RequestedEvidence {
            requirement_key: "customer_adoption_research".to_string(),
            source_type: "whatever".to_string(),
            priority: "urgent".to_string(),
            reason: "".to_string(),
        };
        let out = canonical_requested_evidence(&req);

        assert_eq!(out.requirement_key, "product_research");
        assert_eq!(out.source_type, "web_research");
        assert_eq!(out.priority, "medium");
        assert!(out.reason.contains("product_research"));
        assert_eq!(
            actions_for_requirement(&out.requirement_key),
            ["gdelt_doc_search", "bing_news_rss_search"]
        );

        let catalyst = canonical_requested_evidence(&RequestedEvidence {
            requirement_key: "earnings_calendar".to_string(),
            source_type: "whatever".to_string(),
            priority: "high".to_string(),
            reason: "Need next earnings date".to_string(),
        });
        assert_eq!(catalyst.requirement_key, "earnings_calendar");
        assert_eq!(catalyst.source_type, "catalysts");
        assert_eq!(
            actions_for_requirement(&catalyst.requirement_key),
            ["fmp_earnings_calendar"]
        );
    }

    #[test]
    fn chat_fallback_requests_research_for_missing_evidence_questions() {
        let package = json!({
            "technical_state": {
                "state": "extended",
                "summary": "technical state is extended; +25.0% vs 200-day SMA"
            },
            "evidence_items": []
        });

        let answer = fallback_chat_answer(
            "Search current product evidence and tell me what is missing",
            "technical",
            Some("ENTG"),
            &package,
        );

        assert_eq!(answer.technical_read.state.as_deref(), Some("extended"));
        assert_eq!(answer.requested_evidence.len(), 1);
        assert_eq!(
            answer.requested_evidence[0].requirement_key,
            "product_research"
        );
    }

    #[test]
    fn brain_source_tasks_are_grouped_by_requirement_or_action() {
        let at = Utc.with_ymd_and_hms(2026, 6, 1, 12, 0, 0).unwrap();
        let rows = vec![
            SourceTaskSnapshot {
                requirement_key: Some("analyst_opinion".to_string()),
                action: "fmp_price_target_news".to_string(),
                provider: "fmp".to_string(),
                state: "queued".to_string(),
                result: None,
                priority: "medium".to_string(),
                due_at: Some(at),
                next_retry_at: None,
                attempts: 2,
                last_error: None,
                updated_at: Some(at),
            },
            SourceTaskSnapshot {
                requirement_key: Some("recent_news".to_string()),
                action: "fmp_news".to_string(),
                provider: "fmp".to_string(),
                state: "satisfied".to_string(),
                result: Some("rows_seen".to_string()),
                priority: "high".to_string(),
                due_at: Some(at),
                next_retry_at: None,
                attempts: 1,
                last_error: None,
                updated_at: Some(at),
            },
            SourceTaskSnapshot {
                requirement_key: None,
                action: "twse_price_backfill".to_string(),
                provider: "twse".to_string(),
                state: "satisfied".to_string(),
                result: Some("rows_seen".to_string()),
                priority: "blocking".to_string(),
                due_at: Some(at),
                next_retry_at: None,
                attempts: 1,
                last_error: None,
                updated_at: Some(at),
            },
        ];

        let opinion = source_tasks_for(
            &rows,
            &["analyst_opinion"],
            &[
                "fmp_price_target_consensus",
                "fmp_grades_historical",
                "fmp_price_target_news",
                "fmp_grades_latest_news",
            ],
        );
        let price = source_tasks_for(&rows, &["price_history"], &["twse_price_backfill"]);

        assert_eq!(opinion.as_array().unwrap().len(), 1);
        assert_eq!(opinion[0]["action"], "fmp_price_target_news");
        assert_eq!(price.as_array().unwrap().len(), 1);
        assert_eq!(price[0]["provider"], "twse");
    }

    #[test]
    fn source_task_materiality_requires_rows_seen() {
        let base = SourceTaskSnapshot {
            requirement_key: Some("recent_news".to_string()),
            action: "fmp_news".to_string(),
            provider: "fmp".to_string(),
            state: "satisfied".to_string(),
            result: Some("rows_seen".to_string()),
            priority: "high".to_string(),
            due_at: None,
            next_retry_at: None,
            attempts: 1,
            last_error: None,
            updated_at: None,
        };

        assert!(source_task_is_material(&base));

        let no_rows = SourceTaskSnapshot {
            result: Some("no_rows".to_string()),
            ..base.clone()
        };
        assert!(!source_task_is_material(&no_rows));

        let recurring_freshness_sync = SourceTaskSnapshot {
            result: None,
            ..base.clone()
        };
        assert!(!source_task_is_material(&recurring_freshness_sync));

        let failed = SourceTaskSnapshot {
            state: "failed".to_string(),
            ..base
        };
        assert!(!source_task_is_material(&failed));
    }

    #[test]
    fn source_health_effective_status_marks_stale_running() {
        let now = Utc.with_ymd_and_hms(2026, 6, 1, 12, 30, 0).unwrap();
        let started = Utc.with_ymd_and_hms(2026, 6, 1, 12, 0, 0).unwrap();

        let (status, stale, age) = source_health_effective_status("running", Some(started), now);

        assert_eq!(status, "stale_running");
        assert!(stale);
        assert_eq!(age, Some(30));
    }

    #[test]
    fn source_health_effective_status_keeps_fresh_running() {
        let now = Utc.with_ymd_and_hms(2026, 6, 1, 12, 10, 0).unwrap();
        let started = Utc.with_ymd_and_hms(2026, 6, 1, 12, 0, 0).unwrap();

        let (status, stale, age) = source_health_effective_status("running", Some(started), now);

        assert_eq!(status, "running");
        assert!(!stale);
        assert_eq!(age, Some(10));
    }

    #[test]
    fn source_health_effective_status_leaves_finished_statuses() {
        let now = Utc.with_ymd_and_hms(2026, 6, 1, 12, 30, 0).unwrap();
        let started = Utc.with_ymd_and_hms(2026, 6, 1, 12, 0, 0).unwrap();

        let (status, stale, age) = source_health_effective_status("ok", Some(started), now);

        assert_eq!(status, "ok");
        assert!(!stale);
        assert_eq!(age, None);
    }

    #[test]
    fn source_health_group_treats_stale_running_as_stale() {
        let now = Utc.with_ymd_and_hms(2026, 6, 1, 12, 30, 0).unwrap();
        let started = Utc.with_ymd_and_hms(2026, 6, 1, 12, 0, 0).unwrap();
        let rows = vec![SourceHealthSnapshot {
            source: "xbrl".to_string(),
            last_status: "running".to_string(),
            last_success_at: None,
            last_started_at: Some(started),
            last_failure_kind: None,
            last_error: None,
            retry_after_at: None,
        }];

        let out = source_health_group(&rows, &["xbrl"], now, chrono::Duration::minutes(15));

        assert_eq!(out["status"], "stale");
    }

    #[test]
    fn source_health_group_keeps_fresh_running_active() {
        let now = Utc.with_ymd_and_hms(2026, 6, 1, 12, 10, 0).unwrap();
        let started = Utc.with_ymd_and_hms(2026, 6, 1, 12, 0, 0).unwrap();
        let rows = vec![SourceHealthSnapshot {
            source: "xbrl".to_string(),
            last_status: "running".to_string(),
            last_success_at: None,
            last_started_at: Some(started),
            last_failure_kind: None,
            last_error: None,
            retry_after_at: None,
        }];

        let out = source_health_group(&rows, &["xbrl"], now, chrono::Duration::minutes(15));

        assert_eq!(out["status"], "running");
    }

    #[test]
    fn source_health_group_uses_newest_source_activity_time() {
        let now = Utc.with_ymd_and_hms(2026, 6, 1, 12, 10, 0).unwrap();
        let started = Utc.with_ymd_and_hms(2026, 6, 1, 12, 9, 0).unwrap();
        let success = Utc.with_ymd_and_hms(2026, 6, 1, 11, 0, 0).unwrap();
        let rows = vec![SourceHealthSnapshot {
            source: "xbrl".to_string(),
            last_status: "running".to_string(),
            last_success_at: Some(success),
            last_started_at: Some(started),
            last_failure_kind: None,
            last_error: None,
            retry_after_at: None,
        }];

        let out = source_health_group(&rows, &["xbrl"], now, chrono::Duration::minutes(15));

        assert_eq!(out["status"], "running");
        assert_eq!(out["last_checked_at"], serde_json::json!(started));
    }

    #[test]
    fn chat_missing_evidence_questions_are_detected_deterministically() {
        let package = json!({
            "evidence_items": [{
                "source": "ticker_context",
                "summary": "existing context"
            }]
        });

        assert!(fallback_should_request_evidence(
            "Search current MI325X articles and source gaps",
            &package
        ));

        let req = default_product_research_request("Search current MI325X articles", Some("AMD"));
        assert_eq!(req.requirement_key, "product_research");
        assert_eq!(req.source_type, "web_research");
        assert_eq!(req.priority, "high");
        assert!(req.reason.contains("AMD"));
    }

    #[test]
    fn intraday_history_windows_backfill_from_oldest_bar() {
        let target = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        let oldest = NaiveDate::from_ymd_opt(2026, 5, 4).unwrap();

        let windows = intraday_history_windows(target, oldest, 45, 3);

        assert_eq!(
            windows,
            vec![
                IntradayFetchWindow {
                    from: NaiveDate::from_ymd_opt(2026, 3, 20).unwrap(),
                    to: NaiveDate::from_ymd_opt(2026, 5, 3).unwrap(),
                },
                IntradayFetchWindow {
                    from: NaiveDate::from_ymd_opt(2026, 2, 3).unwrap(),
                    to: NaiveDate::from_ymd_opt(2026, 3, 19).unwrap(),
                },
                IntradayFetchWindow {
                    from: target,
                    to: NaiveDate::from_ymd_opt(2026, 2, 2).unwrap(),
                },
            ]
        );
    }

    #[test]
    fn intraday_history_windows_do_not_fetch_when_covered() {
        let target = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        let oldest = NaiveDate::from_ymd_opt(2025, 12, 15).unwrap();

        assert!(intraday_history_windows(target, oldest, 45, 4).is_empty());
    }
}
