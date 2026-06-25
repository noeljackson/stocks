//! Anthropic Messages API transport. Works with Anthropic direct, z.ai,
//! Bedrock/Vertex proxies — anything that speaks the Messages shape.

use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::warn;

use super::{Provider, Request, Response, Usage, append_schema_to_system, truncate};
use crate::platform::config::LlmTransport;

const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
const DEFAULT_VERSION: &str = "2023-06-01";
const DEFAULT_MAX_TOKENS: u32 = 4096;

pub struct AnthropicProvider {
    base_url: String,
    api_key: String,
    version: String,
    model: String,
    client: Client,
}

impl AnthropicProvider {
    /// Returns `None` if required config is missing (caller falls back to mock).
    pub fn try_new(cfg: &LlmTransport) -> Option<Self> {
        if cfg.anthropic_api_key.is_empty() {
            warn!("llm anthropic: missing ANTHROPIC_API_KEY, using mock");
            return None;
        }
        let base_url = if cfg.anthropic_base_url.is_empty() {
            DEFAULT_BASE_URL.to_string()
        } else {
            cfg.anthropic_base_url.trim_end_matches('/').to_string()
        };
        let version = if cfg.anthropic_version.is_empty() {
            DEFAULT_VERSION.to_string()
        } else {
            cfg.anthropic_version.clone()
        };
        let client = Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .ok()?;
        Some(Self {
            base_url,
            api_key: cfg.anthropic_api_key.clone(),
            version,
            model: cfg.model.clone(),
            client,
        })
    }
}

#[derive(Serialize)]
struct ReqBody<'a> {
    model: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    system: &'a str,
    messages: &'a [super::Message],
    max_tokens: u32,
}

#[derive(Deserialize)]
struct RespBody {
    #[serde(default)]
    content: Vec<ContentBlock>,
    #[serde(default)]
    model: String,
    #[serde(default)]
    usage: UsageBody,
    #[serde(default)]
    error: Option<ErrorBody>,
}

#[derive(Deserialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    block_type: String,
    #[serde(default)]
    text: String,
}

#[derive(Deserialize, Default)]
struct UsageBody {
    #[serde(default)]
    input_tokens: u32,
    #[serde(default)]
    output_tokens: u32,
}

#[derive(Deserialize)]
struct ErrorBody {
    #[serde(default, rename = "type")]
    err_type: String,
    #[serde(default)]
    message: String,
}

#[async_trait]
impl Provider for AnthropicProvider {
    async fn complete(&self, req: Request) -> anyhow::Result<Response> {
        let system = append_schema_to_system(&req.system, req.json_schema.as_ref());
        let model = if req.model.is_empty() {
            &self.model
        } else {
            &req.model
        };
        let max_tokens = if req.max_tokens == 0 {
            DEFAULT_MAX_TOKENS
        } else {
            req.max_tokens
        };

        let body = ReqBody {
            model,
            system: &system,
            messages: &req.messages,
            max_tokens,
        };

        let url = format!("{}/v1/messages", self.base_url);
        let resp = self
            .client
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", &self.version)
            .json(&body)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("anthropic request: {e}"))?;

        let status = resp.status();
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| anyhow::anyhow!("anthropic body: {e}"))?;

        if !status.is_success() {
            let body_str = String::from_utf8_lossy(&bytes);
            return Err(anyhow::anyhow!(
                "anthropic {}: {}",
                status.as_u16(),
                truncate(&body_str, 512)
            ));
        }

        let parsed: RespBody =
            serde_json::from_slice(&bytes).map_err(|e| anyhow::anyhow!("anthropic decode: {e}"))?;
        if let Some(err) = parsed.error {
            return Err(anyhow::anyhow!(
                "anthropic {}: {}",
                err.err_type,
                err.message
            ));
        }
        let content: String = parsed
            .content
            .into_iter()
            .filter(|b| b.block_type == "text")
            .map(|b| b.text)
            .collect();
        Ok(Response {
            content,
            model: parsed.model,
            usage: Usage {
                input_tokens: parsed.usage.input_tokens,
                output_tokens: parsed.usage.output_tokens,
            },
        })
    }
}
