//! Universal sentiment classifier (#19).
//!
//! Takes (ticker, headline, body) → a `SentimentScore` audited via
//! `llm_invocation`. Designed as a small reusable building block that any
//! news ingest (Massive, FMP, RSS, etc.) can call to score articles that
//! arrive without an upstream sentiment field.
//!
//! The actual classification logic lives in `prompts/score-sentiment.md`
//! (a first-class prompt registry entry), so behavior is tunable without
//! redeploying — just edit the prompt, the hash changes, audit rows
//! reflect the new version.

use anyhow::Result;
use serde::Deserialize;

use crate::llm::Provider;
use crate::llm::prompts::{InvocationRecorder, Prompt, complete_typed};

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct SentimentScore {
    pub sentiment: String,   // "positive" | "neutral" | "negative"
    pub polarity: f64,       // -1.0 .. 1.0
    pub confidence: String,  // "low" | "medium" | "high"
    pub rationale: String,
}

impl SentimentScore {
    /// Returns true if `sentiment` and `confidence` match the schema we
    /// promised. The validation happens at the `complete_typed` parse step,
    /// but production callers may want a belt-and-braces check before
    /// persisting (e.g. an LLM drift would manifest as a fresh label).
    #[must_use]
    pub fn is_valid(&self) -> bool {
        matches!(self.sentiment.as_str(), "positive" | "neutral" | "negative")
            && matches!(self.confidence.as_str(), "low" | "medium" | "high")
            && (-1.0..=1.0).contains(&self.polarity)
    }
}

/// Score one article. The LLM gets ticker + title + body; we return whatever
/// it produced (subject to JSON schema validation in `complete_typed`).
///
/// Body may be empty/short — the classifier handles either case.
pub async fn score_one(
    provider: &dyn Provider,
    recorder: Option<&dyn InvocationRecorder>,
    prompt: &Prompt,
    provider_name: &str,
    ticker: &str,
    title: &str,
    body: &str,
    model_override: Option<&str>,
) -> Result<SentimentScore> {
    let user_msg = serde_json::json!({
        "ticker": ticker,
        "title": title,
        "body": body,
    })
    .to_string();
    let score: SentimentScore = complete_typed(
        provider,
        recorder,
        prompt,
        &std::collections::HashMap::new(),
        &user_msg,
        provider_name,
        model_override,
        2, // retries
    )
    .await?;
    Ok(score)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_valid_accepts_well_formed() {
        let s = SentimentScore {
            sentiment: "positive".into(),
            polarity: 0.65,
            confidence: "high".into(),
            rationale: "Beat expectations".into(),
        };
        assert!(s.is_valid());
    }

    #[test]
    fn is_valid_rejects_unknown_label() {
        let s = SentimentScore {
            sentiment: "bullish".into(),
            polarity: 0.5,
            confidence: "high".into(),
            rationale: "x".into(),
        };
        assert!(!s.is_valid(), "drifted label must fail validation");
    }

    #[test]
    fn is_valid_rejects_polarity_out_of_range() {
        let s = SentimentScore {
            sentiment: "positive".into(),
            polarity: 1.5,
            confidence: "high".into(),
            rationale: "x".into(),
        };
        assert!(!s.is_valid());
        let s2 = SentimentScore { polarity: -2.0, ..s };
        assert!(!s2.is_valid());
    }

    #[test]
    fn is_valid_rejects_unknown_confidence() {
        let s = SentimentScore {
            sentiment: "neutral".into(),
            polarity: 0.0,
            confidence: "very-high".into(),
            rationale: "x".into(),
        };
        assert!(!s.is_valid());
    }

    #[test]
    fn deserializes_clean_llm_response() {
        let raw = r#"{"sentiment":"negative","polarity":-0.7,"confidence":"high","rationale":"earnings miss"}"#;
        let s: SentimentScore = serde_json::from_str(raw).unwrap();
        assert!(s.is_valid());
        assert_eq!(s.sentiment, "negative");
        assert_eq!(s.polarity, -0.7);
    }
}
