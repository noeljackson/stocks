//! Swappable LLM provider abstraction (SPEC §3 invariant).
//!
//! Two real transports plus a deterministic mock:
//!
//! - `anthropic`     — Messages API shape. Works with Anthropic direct,
//!   z.ai (set `ANTHROPIC_BASE_URL=https://api.z.ai/api/anthropic`),
//!   Bedrock/Vertex proxies.
//! - `openai_compat` — `/v1/chat/completions` shape. DeepSeek, Together,
//!   OpenRouter, vLLM, Groq.
//! - `mock`          — fixed JSON; default for dev/tests.

mod anthropic;
mod openai;
#[cfg(test)]
mod tests;

pub use anthropic::AnthropicProvider;
pub use openai::OpenAiCompatProvider;

use serde::{Deserialize, Serialize};

use crate::platform::config::LlmTransport;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String, // "user" | "assistant"
    pub content: String,
}

#[derive(Debug, Clone, Default)]
pub struct Request {
    pub model: String,
    pub system: String,
    pub messages: Vec<Message>,
    pub max_tokens: u32,
    /// When non-empty, appended to the system prompt asking for JSON
    /// matching the schema. Provider-agnostic structured output.
    pub json_schema: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Response {
    pub content: String,
    pub usage: Usage,
    pub model: String,
}

#[async_trait::async_trait]
pub trait Provider: Send + Sync {
    async fn complete(&self, req: Request) -> anyhow::Result<Response>;
}

/// Returns a provider configured from `cfg`. Unknown providers and missing
/// required config both fall back to [`MockProvider`] — never panic — so a
/// misconfigured env var doesn't crash boot.
#[must_use]
pub fn new(cfg: &LlmTransport) -> Box<dyn Provider> {
    match cfg.provider.as_str() {
        "anthropic" => match AnthropicProvider::try_new(cfg) {
            Some(p) => Box::new(p),
            None => Box::new(MockProvider),
        },
        "openai_compat" | "openai" => match OpenAiCompatProvider::try_new(cfg) {
            Some(p) => Box::new(p),
            None => Box::new(MockProvider),
        },
        _ => Box::new(MockProvider),
    }
}

pub struct MockProvider;

#[async_trait::async_trait]
impl Provider for MockProvider {
    async fn complete(&self, _req: Request) -> anyhow::Result<Response> {
        Ok(Response {
            content: r#"{"mock":true}"#.into(),
            model: "mock".into(),
            usage: Usage::default(),
        })
    }
}

/// Appends a JSON-schema instruction to the system prompt. Same approach
/// works for both Anthropic-shape and OpenAI-shape transports.
pub(crate) fn append_schema_to_system(system: &str, schema: Option<&serde_json::Value>) -> String {
    let Some(schema) = schema else {
        return system.to_string();
    };
    let suffix = format!(
        "\n\nRespond ONLY with JSON matching this schema (no prose, no markdown fences):\n{schema}"
    );
    if system.is_empty() {
        format!("Respond with JSON only.{suffix}")
    } else {
        format!("{system}{suffix}")
    }
}

pub(crate) fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let cut: String = s.chars().take(n).collect();
        format!("{cut}…")
    }
}
