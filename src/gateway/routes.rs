//! HTTP routes — REST + SSE + SPA fallback.

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::{
    Json, Router,
    extract::State,
    http::{StatusCode, header},
    response::{IntoResponse, Sse, sse::Event},
    routing::{get, post},
};
use futures::stream::Stream;
use serde::Deserialize;
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
        .route("/api/stream", get(stream))
        .route("/api/decisions", post(record_decision))
        .fallback(spa_handler)
        .with_state(gw)
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

async fn spa_handler(uri: axum::http::Uri) -> impl IntoResponse {
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
