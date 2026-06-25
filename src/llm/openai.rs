//! OpenAI-compatible `/v1/chat/completions` transport.
//! DeepSeek, Together, OpenRouter, vLLM, Groq, Fireworks, etc.

use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::warn;

use super::{Message, Provider, Request, Response, Usage, append_schema_to_system, truncate};
use crate::platform::config::LlmTransport;

const DEFAULT_MAX_TOKENS: u32 = 4096;

pub struct OpenAiCompatProvider {
    base_url: String,
    api_key: String,
    model: String,
    client: Client,
}

impl OpenAiCompatProvider {
    /// Returns `None` if required config is missing (caller falls back to mock).
    pub fn try_new(cfg: &LlmTransport) -> Option<Self> {
        if cfg.openai_base_url.is_empty() {
            warn!("llm openai_compat: missing OPENAI_BASE_URL, using mock");
            return None;
        }
        if cfg.openai_api_key.is_empty() {
            warn!("llm openai_compat: missing OPENAI_API_KEY, using mock");
            return None;
        }
        // Strip a trailing /v1 (or /) so we can always append /v1/chat/completions.
        let mut base = cfg.openai_base_url.trim_end_matches('/').to_string();
        if let Some(stripped) = base.strip_suffix("/v1") {
            base = stripped.to_string();
        }
        let client = Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .ok()?;
        Some(Self {
            base_url: base,
            api_key: cfg.openai_api_key.clone(),
            model: cfg.model.clone(),
            client,
        })
    }
}

#[derive(Serialize)]
struct ReqBody<'a> {
    model: &'a str,
    messages: &'a [Message],
    #[serde(skip_serializing_if = "is_zero")]
    max_tokens: u32,
}

fn is_zero(v: &u32) -> bool {
    *v == 0
}

#[derive(Deserialize)]
struct RespBody {
    #[serde(default)]
    choices: Vec<Choice>,
    #[serde(default)]
    model: String,
    #[serde(default)]
    usage: UsageBody,
    #[serde(default)]
    error: Option<ErrorBody>,
}

#[derive(Deserialize)]
struct Choice {
    message: MessageBody,
}

#[derive(Deserialize)]
struct MessageBody {
    #[serde(default)]
    content: String,
}

#[derive(Deserialize, Default)]
struct UsageBody {
    #[serde(default)]
    prompt_tokens: u32,
    #[serde(default)]
    completion_tokens: u32,
}

#[derive(Deserialize)]
struct ErrorBody {
    #[serde(default, rename = "type")]
    err_type: String,
    #[serde(default)]
    message: String,
}

#[async_trait]
impl Provider for OpenAiCompatProvider {
    async fn complete(&self, req: Request) -> anyhow::Result<Response> {
        let system = append_schema_to_system(&req.system, req.json_schema.as_ref());
        let mut msgs: Vec<Message> = Vec::with_capacity(req.messages.len() + 1);
        if !system.is_empty() {
            msgs.push(Message {
                role: "system".into(),
                content: system,
            });
        }
        msgs.extend(req.messages.iter().cloned());

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
            messages: &msgs,
            max_tokens,
        };
        let url = format!("{}/v1/chat/completions", self.base_url);
        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("openai request: {e}"))?;

        let status = resp.status();
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| anyhow::anyhow!("openai body: {e}"))?;

        if !status.is_success() {
            let body_str = String::from_utf8_lossy(&bytes);
            return Err(anyhow::anyhow!(
                "openai {}: {}",
                status.as_u16(),
                truncate(&body_str, 512)
            ));
        }

        let parsed: RespBody =
            serde_json::from_slice(&bytes).map_err(|e| anyhow::anyhow!("openai decode: {e}"))?;
        if let Some(err) = parsed.error {
            return Err(anyhow::anyhow!("openai {}: {}", err.err_type, err.message));
        }
        let content = parsed
            .choices
            .into_iter()
            .next()
            .map(|c| c.message.content)
            .unwrap_or_default();
        Ok(Response {
            content,
            model: parsed.model,
            usage: Usage {
                input_tokens: parsed.usage.prompt_tokens,
                output_tokens: parsed.usage.completion_tokens,
            },
        })
    }
}
