//! HTTP routes — REST + SSE + SPA fallback.

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Sse, sse::Event},
    routing::{get, post},
};
use futures::stream::Stream;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio_stream::wrappers::BroadcastStream;
use tracing::warn;

use super::Gateway;
use crate::platform::subjects;
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
        .route("/api/research-evidence", get(list_research_evidence))
        .route(
            "/api/theses/{thesis_id}/transition",
            post(transition_thesis),
        )
        .route("/api/ticker-context", get(get_ticker_context))
        .route("/api/candles", get(get_candles))
        .route("/api/symbol-events", get(get_symbol_events))
        .route("/api/decisions", get(get_decisions).post(record_decision))
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
        .route("/api/brain-status", get(get_brain_status))
        .route("/api/attention", get(list_attention_items))
        .route("/api/attention/{id}/dismiss", post(dismiss_attention_item))
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
    let payload = serde_json::json!({
        "thesis_id": thesis_id,
        "symbol": t.symbol,
        "from": t.state.as_str(),
        "to": req.to.as_str(),
        "rationale": req.rationale,
        "at": chrono::Utc::now(),
    });
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
        let mut fetch_error: Option<String> = None;
        match gw.store.latest_intraday_bar_ts(&sym, native).await {
            Ok(latest) => {
                let stale = latest
                    .map(|ts| chrono::Utc::now() - ts > chrono::Duration::minutes(30))
                    .unwrap_or(true);
                if stale {
                    match gw.fmp_intraday.fetch_one(&sym, native, lookback_days).await {
                        Ok(rows) => {
                            if let Err(e) = gw.store.upsert_intraday_price_bars(&rows).await {
                                warn!(symbol = %sym, interval = native, error = %e, "persist intraday bars failed");
                            }
                        }
                        Err(e) => {
                            warn!(symbol = %sym, interval = native, error = %e, "fetch intraday bars failed");
                            fetch_error = Some(e.to_string());
                        }
                    }
                }
            }
            Err(e) => {
                warn!(symbol = %sym, interval = native, error = %e, "latest_intraday_bar_ts failed")
            }
        }
        match gw
            .store
            .intraday_candles_for(&sym, native, lookback_days, interval.bucket_minutes)
            .await
        {
            Ok(rows) => {
                if rows.is_empty() {
                    if let Some(e) = fetch_error {
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
                return (StatusCode::OK, Json(rows)).into_response();
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
        Ok(rows) => (StatusCode::OK, Json(rows)).into_response(),
        Err(e) => {
            warn!(symbol = %sym, error = %e, "get_candles failed");
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
    }
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
    let stream = BroadcastStream::new(rx).filter_map(|res| async move {
        match res {
            Ok(s) => Some(Ok(Event::default().data(s))),
            Err(_) => None, // Lagged → drop
        }
    });
    use futures::StreamExt;
    let stream = stream.boxed();
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
    sizing: Option<serde_json::Value>,
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
        r#"SELECT symbol, company_name, sector, industry, market_cap, first_seen_at
             FROM discovery_pool
            WHERE dropped_at IS NULL
         ORDER BY market_cap DESC NULLS LAST, symbol"#,
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
        let last_started_at: Option<chrono::DateTime<chrono::Utc>> =
            r.try_get("last_started_at").ok();
        let last_success_at: Option<chrono::DateTime<chrono::Utc>> =
            r.try_get("last_success_at").ok();
        let last_failure_at: Option<chrono::DateTime<chrono::Utc>> =
            r.try_get("last_failure_at").ok();
        let retry_after_at: Option<chrono::DateTime<chrono::Utc>> =
            r.try_get("retry_after_at").ok();
        let updated_at: Option<chrono::DateTime<chrono::Utc>> = r.try_get("updated_at").ok();
        json!({
            "source": r.try_get::<String, _>("source").unwrap_or_default(),
            "last_status": r.try_get::<String, _>("last_status").unwrap_or_default(),
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
        json!({
            "contexts_24h": ctx_24h,
            "contexts_total_symbols": ctx_total,
            "thesis_by_state": by_state,
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
        json!({
            "open_requirements": open,
            "by_state": by_state,
            "by_reason": by_reason,
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

async fn get_brain_status(
    State(gw): State<Arc<Gateway>>,
    Query(q): Query<BrainStatusQuery>,
) -> impl IntoResponse {
    use crate::platform::brain::{BrainDecisionInput, age_freshness, decide};
    use chrono::Duration as ChronoDuration;
    use sqlx::Row;

    let symbol = q.symbol.trim().to_ascii_uppercase();
    if symbol.is_empty() || symbol.len() > 10 {
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
              (SELECT max(ingested_at) FROM news_article WHERE symbol = $1) AS news_at,
              (SELECT max(published_at) FROM news_article WHERE symbol = $1) AS news_published_at,
              (SELECT max(snapshot_at) FROM estimate_snapshot WHERE symbol = $1) AS estimates_at,
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

    let health = |names: &[&str], max_age: ChronoDuration| {
        source_health_group(&health_rows, names, now, max_age)
    };
    let price_health = health(&["fmp_price"], ChronoDuration::minutes(30));
    let news_health = health(&["fmp_news", "massive_news"], ChronoDuration::minutes(30));
    let estimates_health = health(&["fmp_estimates"], ChronoDuration::minutes(30));
    let research_health = health(&["web_research"], ChronoDuration::hours(24));
    let fundamentals_health = health(&["xbrl"], ChronoDuration::minutes(360));
    let filings_health = health(&["edgar"], ChronoDuration::minutes(30));
    let news_status = source_status(&news_health).to_string();
    let estimates_status = source_status(&estimates_health).to_string();
    let research_status = source_status(&research_health).to_string();
    let fundamentals_status = source_status(&fundamentals_health).to_string();
    let filings_status = source_status(&filings_health).to_string();

    let price_at: Option<chrono::DateTime<chrono::Utc>> = row.try_get("price_at").ok();
    let price_session: Option<chrono::NaiveDate> = row.try_get("price_session").ok();
    let news_at: Option<chrono::DateTime<chrono::Utc>> = row.try_get("news_at").ok();
    let news_published_at: Option<chrono::DateTime<chrono::Utc>> =
        row.try_get("news_published_at").ok();
    let estimates_at: Option<chrono::DateTime<chrono::Utc>> = row.try_get("estimates_at").ok();
    let research_at: Option<chrono::DateTime<chrono::Utc>> = row.try_get("research_at").ok();
    let fundamentals_at: Option<chrono::DateTime<chrono::Utc>> =
        row.try_get("fundamentals_at").ok();
    let filings_at: Option<chrono::DateTime<chrono::Utc>> = row.try_get("filings_at").ok();
    let context_at: Option<chrono::DateTime<chrono::Utc>> = row.try_get("context_at").ok();
    let thesis_updated_at: Option<chrono::DateTime<chrono::Utc>> =
        row.try_get("open_thesis_updated_at").ok();
    let thesis_at: Option<chrono::DateTime<chrono::Utc>> = row.try_get("open_thesis_at").ok();
    let evidence_rows: i64 = row.try_get("evidence_rows").unwrap_or(0);
    let open_evidence: i64 = row.try_get("open_evidence").unwrap_or(0);
    let blocking_evidence: i64 = row.try_get("blocking_evidence").unwrap_or(0);
    let due_evidence: i64 = row.try_get("due_evidence").unwrap_or(0);

    let price_status = match price_session {
        Some(d) if d >= expected_price_session => "fresh",
        Some(_) => "stale",
        None => "missing",
    };
    let context_freshness = age_freshness(now, context_at, ChronoDuration::hours(12));
    let thesis_freshness = age_freshness(now, thesis_at, ChronoDuration::minutes(30));
    let source_blocked = [
        &price_health,
        &news_health,
        &estimates_health,
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
        research_status.as_str(),
        filings_status.as_str(),
    ]
    .iter()
    .any(|s| matches!(*s, "stale" | "missing" | "rate_limited" | "failed"));

    let decision = decide(BrainDecisionInput {
        evidence_rows,
        open_evidence,
        blocking_evidence,
        due_evidence,
        has_context: context_at.is_some(),
        context_stale: context_freshness.as_str() == "stale",
        has_open_thesis: thesis_at.is_some(),
        thesis_stale: thesis_freshness.as_str() == "stale",
        any_source_stale,
        source_blocked,
    });

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

    let body = json!({
        "symbol": symbol,
        "as_of": now,
        "active_ticker": row.try_get::<bool, _>("active_ticker").unwrap_or(false),
        "status": decision.status,
        "next_action": decision.next_action,
        "reason": decision.reason,
        "freshness_target_minutes": 30,
        "sources": [
            source_json("price", price_status, price_at, price_health, json!({
                "expected_latest_session": expected_price_session,
                "actual_latest_session": price_session,
            })),
            source_json("news", &news_status, news_at, news_health, json!({
                "latest_published_at": news_published_at,
            })),
            source_json("estimates", &estimates_status, estimates_at, estimates_health, json!({})),
            source_json("research", &research_status, research_at, research_health, json!({})),
            source_json("fundamentals", &fundamentals_status, fundamentals_at, fundamentals_health, json!({})),
            source_json("filings", &filings_status, filings_at, filings_health, json!({})),
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
        },
        "attention": {
            "open": row.try_get::<i64, _>("open_attention").unwrap_or(0),
            "by_kind": attention_kinds,
        },
    });
    (StatusCode::OK, Json(body)).into_response()
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
        .filter_map(|r| r.last_success_at.or(r.last_started_at))
        .max();
    let retry_after_at = matching.iter().filter_map(|r| r.retry_after_at).max();
    let last_error = matching.iter().find_map(|r| r.last_error.clone());
    let failure_kind = matching.iter().find_map(|r| r.last_failure_kind.clone());
    let status = if matching.is_empty() {
        "missing"
    } else if matching
        .iter()
        .any(|r| r.last_failure_kind.as_deref() == Some("rate_limited"))
    {
        "rate_limited"
    } else if matching.iter().any(|r| r.last_status == "failed") {
        "failed"
    } else if matching.iter().any(|r| r.last_status == "running") {
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

fn source_json(
    source: &str,
    status: &str,
    last_changed_at: Option<chrono::DateTime<chrono::Utc>>,
    health: serde_json::Value,
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

async fn record_decision(
    State(gw): State<Arc<Gateway>>,
    Json(req): Json<DecisionReq>,
) -> impl IntoResponse {
    let sizing = req.sizing.clone().unwrap_or(serde_json::Value::Null);
    let thesis_uuid: Option<uuid::Uuid> = if req.thesis_id.is_empty() {
        None
    } else {
        uuid::Uuid::parse_str(&req.thesis_id).ok()
    };

    let result = sqlx::query(
        r#"INSERT INTO decision (thesis_id, action, user_choice, sizing)
           VALUES ($1, $2, $3, $4)"#,
    )
    .bind(thesis_uuid)
    .bind(&req.action)
    .bind(if req.user_choice.is_empty() {
        None
    } else {
        Some(&req.user_choice)
    })
    .bind(sizing)
    .execute(&gw.store.pool)
    .await;

    if let Err(e) = result {
        warn!(error = %e, "record_decision failed");
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }

    // Resolve any open thesis_actionable attention item for this thesis (#86).
    if let Some(tid) = thesis_uuid {
        if let Err(e) = gw
            .store
            .resolve_attention(
                "thesis_actionable",
                Some(tid),
                None,
                &format!("decision_recorded:{}", req.action),
                serde_json::json!({"action": req.action, "user_choice": req.user_choice}),
            )
            .await
        {
            warn!(error = %e, "attention resolve failed (non-fatal)");
        }
    }

    let env = json!({
        "thesis_id": req.thesis_id,
        "action":    req.action,
        "user_choice": req.user_choice,
        "sizing":    req.sizing,
    });
    if let Err(e) = gw
        .bus
        .publish(subjects::DECISION_RECORDED, env.to_string().as_bytes())
        .await
    {
        warn!(error = %e, "decision publish failed (best-effort)");
    }
    StatusCode::NO_CONTENT.into_response()
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
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
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
    if sym.is_empty() || sym.len() > 10 {
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
