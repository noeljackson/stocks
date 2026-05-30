//! Event router: fans ingest.* events to per-ticker subjects so that
//! downstream services (Python context maintainer) can bind a single
//! durable consumer rather than juggling N subject patterns.

use std::sync::Arc;

use anyhow::Result;
use tracing::info;

use crate::platform::bus::{Bus, ConsumerHandle};
use crate::platform::subjects;

/// Pulls a `ticker` or `symbol` string field out of the JSON payload and
/// returns the uppercase + trimmed value. `None` for malformed/missing/empty —
/// those events are market-wide and not per-ticker routed.
pub fn extract_symbol(payload: &[u8]) -> Option<String> {
    let v: serde_json::Value = serde_json::from_slice(payload).ok()?;
    let obj = v.as_object()?;
    for key in ["ticker", "symbol"] {
        if let Some(s) = obj.get(key).and_then(serde_json::Value::as_str) {
            let trimmed = s.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_uppercase());
            }
        }
    }
    None
}

#[must_use]
pub fn route_subject(symbol: &str) -> String {
    format!("route.ticker.{symbol}")
}

/// Run the router service: ensures the TICKER stream, binds a durable
/// consumer on INGEST/ingest.*, and republishes events with a ticker to
/// route.ticker.<SYMBOL>. Market-wide events ack-and-drop.
pub async fn run(bus: Bus) -> Result<ConsumerHandle> {
    bus.ensure_stream(subjects::STREAM_TICKER, &["route.ticker.*"])
        .await?;
    let bus = Arc::new(bus);
    let publisher = bus.clone();
    let handle = bus
        .consume(subjects::STREAM_INGEST, "event-router", "ingest.*", move |msg| {
            let publisher = publisher.clone();
            async move {
                let Some(sym) = extract_symbol(&msg.payload) else {
                    return Ok(());
                };
                publisher.publish(&route_subject(&sym), &msg.payload).await
            }
        })
        .await?;
    info!(stream = subjects::STREAM_INGEST, filter = "ingest.*", "router consuming");
    Ok(handle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn extract_edgar() {
        assert_eq!(
            extract_symbol(br#"{"ticker":"NVDA","form":"10-K"}"#),
            Some("NVDA".into())
        );
    }

    #[test]
    fn extract_lowercase_normalizes() {
        assert_eq!(extract_symbol(br#"{"ticker":"nvda"}"#), Some("NVDA".into()));
    }

    #[test]
    fn extract_accepts_symbol_key() {
        assert_eq!(extract_symbol(br#"{"symbol":"MU"}"#), Some("MU".into()));
    }

    #[test]
    fn extract_market_wide_returns_none() {
        for body in [
            &b"{\"series\":\"VIXCLS\",\"value\":\"15\"}"[..],
            &b"{}"[..],
            &b"{\"ticker\":\"\"}"[..],
        ] {
            assert!(
                extract_symbol(body).is_none(),
                "market-wide should yield no symbol: {:?}",
                std::str::from_utf8(body)
            );
        }
    }

    #[test]
    fn extract_malformed_returns_none() {
        assert!(extract_symbol(b"not json").is_none());
    }

    #[test]
    fn extract_strips_whitespace() {
        assert_eq!(
            extract_symbol(br#"{"ticker":"  amd  "}"#),
            Some("AMD".into())
        );
    }

    #[test]
    fn route_subject_format() {
        assert_eq!(route_subject("NVDA"), "route.ticker.NVDA");
    }
}
