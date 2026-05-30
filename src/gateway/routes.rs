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
        .route("/api/regime", get(get_regime))
        .route("/api/tickers", get(list_tickers))
        .route("/api/theses", get(list_theses))
        .route("/api/theses/{thesis_id}/transition", post(transition_thesis))
        .route("/api/ticker-context", get(get_ticker_context))
        .route("/api/stream", get(stream))
        .route("/api/decisions", post(record_decision))
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
        return (StatusCode::NOT_FOUND, format!("thesis {thesis_id} not found")).into_response();
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
            error: if missing.first().is_some_and(|s| s.starts_with("illegal transition")) {
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
        crate::platform::domain::ThesisState::Actionable => crate::platform::subjects::THESIS_ACTIONABLE,
        crate::platform::domain::ThesisState::Disqualified => crate::platform::subjects::THESIS_INVALIDATED,
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

    (StatusCode::OK, Json(serde_json::json!({
        "thesis_id": thesis_id,
        "from": t.state,
        "to": req.to,
    })))
        .into_response()
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

async fn list_alerts(State(gw): State<Arc<Gateway>>) -> impl IntoResponse {
    match gw.store.recent_alerts(100).await {
        Ok(alerts) => (StatusCode::OK, Json(alerts)).into_response(),
        Err(e) => {
            warn!(error = %e, "list_alerts failed");
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
    .bind(if req.user_choice.is_empty() { None } else { Some(&req.user_choice) })
    .bind(sizing)
    .execute(&gw.store.pool)
    .await;

    if let Err(e) = result {
        warn!(error = %e, "record_decision failed");
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
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

async fn spa_handler(
    State(gw): State<Arc<Gateway>>,
    uri: axum::http::Uri,
) -> impl IntoResponse {
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
