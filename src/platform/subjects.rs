//! Canonical NATS subject + stream names (SPEC §3).

// Ingestion (raw, normalized events from adapters).
pub const INGEST_FILING: &str = "ingest.filing";
pub const INGEST_PRICE: &str = "ingest.price";
pub const INGEST_MACRO: &str = "ingest.macro";
pub const INGEST_NEWS: &str = "ingest.news";

// Context layer.
pub const CONTEXT_UPDATED: &str = "context.updated";
pub const CONTEXT_SHIFT: &str = "context.shift";

// Market state / regime.
pub const REGIME_STATE: &str = "regime.state";
pub const REGIME_CAPITULATION: &str = "regime.capitulation";
pub const MARKET_BAR_FILTER: &str = "market.bar.>";

// Discovery.
pub const DISCOVERY_CANDIDATE: &str = "discovery.candidate";
pub const DISCOVERY_CONFIRMED: &str = "discovery.confirmed";

// Thesis lifecycle.
pub const THESIS_ACTIONABLE: &str = "thesis.actionable";
pub const THESIS_INVALIDATED: &str = "thesis.invalidated";
pub const THESIS_FULFILLED: &str = "thesis.fulfilled";
pub const THESIS_UPDATED: &str = "thesis.updated";

// Risk + decision.
pub const RISK_VETO: &str = "risk.veto";
pub const RISK_WARNING: &str = "risk.warning";
pub const DECISION_RECORDED: &str = "decision.recorded";

// JetStream stream names. Each is durable + replayable; consumers bind
// named durable cursors against them (see [`crate::platform::bus::Bus::consume`]).
pub const STREAM_INGEST: &str = "INGEST"; // ingest.*
pub const STREAM_CONTEXT: &str = "CONTEXT"; // context.*
pub const STREAM_THESIS: &str = "THESIS"; // thesis.*
pub const STREAM_MARKET: &str = "MARKET"; // regime.*, discovery.*, market.*
pub const STREAM_DECISIONS: &str = "DECISIONS"; // risk.*, decision.*
pub const STREAM_TICKER: &str = "TICKER"; // route.ticker.>
pub const TICKER_ROUTE_FILTER: &str = "route.ticker.>";
pub const MARKET_STREAM_SUBJECTS: &[&str] = &["regime.*", "discovery.*", "market.>"];

/// Per-ticker routed subject (event router → context maintainer).
#[must_use]
pub fn ticker_route(symbol: &str) -> String {
    format!("route.ticker.{symbol}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticker_route_format() {
        assert_eq!(ticker_route("NVDA"), "route.ticker.NVDA");
        assert_eq!(ticker_route("2454.TW"), "route.ticker.2454.TW");
        assert_eq!(TICKER_ROUTE_FILTER, "route.ticker.>");
    }

    #[test]
    fn market_stream_includes_bar_subjects() {
        assert_eq!(MARKET_BAR_FILTER, "market.bar.>");
        assert!(MARKET_STREAM_SUBJECTS.contains(&"market.>"));
    }
}
