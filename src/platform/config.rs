//! Runtime configuration loaded from the environment.
//!
//! Same env var names as the Go version — drop-in compatible. The companion
//! [`LlmTransport`] subset is what gets passed to [`crate::llm::new`].

use std::env;

#[derive(Debug, Clone)]
pub struct Config {
    pub database_url: String,
    pub nats_url: String,
    pub gateway_addr: String,
    pub llm_provider: String,
    pub model_deep: String,
    pub model_routine: String,
    pub model_triage: String,
    pub sec_user_agent: String,
    pub fred_api_key: String,
    pub anthropic_base_url: String,
    pub anthropic_auth_token: String,
    pub anthropic_version: String,
    pub openai_base_url: String,
    pub openai_api_key: String,
}

#[derive(Debug, Clone, Default)]
pub struct LlmTransport {
    pub provider: String,
    pub model: String,
    pub anthropic_base_url: String,
    pub anthropic_auth_token: String,
    pub anthropic_version: String,
    pub openai_base_url: String,
    pub openai_api_key: String,
}

fn get(k: &str, default: &str) -> String {
    match env::var(k) {
        Ok(v) if !v.is_empty() => v,
        _ => default.to_string(),
    }
}

impl Config {
    /// Reads config from the environment, falling back to local-dev defaults.
    ///
    /// LLM transport defaults to "mock". To use z.ai (recommended):
    ///
    /// ```text
    /// LLM_PROVIDER=anthropic
    /// ANTHROPIC_BASE_URL=https://api.z.ai/api/anthropic
    /// ANTHROPIC_AUTH_TOKEN=<your z.ai key>
    /// ```
    ///
    /// For DeepSeek / OpenAI-compatible:
    ///
    /// ```text
    /// LLM_PROVIDER=openai_compat
    /// OPENAI_BASE_URL=https://api.deepseek.com
    /// OPENAI_API_KEY=<sk-...>
    /// ```
    #[must_use]
    pub fn load() -> Self {
        // Best-effort .env load — silently no-ops if absent. Real prod uses
        // the orchestrator's env injection.
        let _ = dotenvy::dotenv();
        Self {
            database_url: get(
                "DATABASE_URL",
                "postgres://stocks:stocks_dev_only@localhost:5432/stocks?sslmode=disable",
            ),
            nats_url: get("NATS_URL", "nats://localhost:4222"),
            gateway_addr: get("GATEWAY_ADDR", ":8080"),
            llm_provider: get("LLM_PROVIDER", "mock"),
            model_deep: get("LLM_MODEL_DEEP", "claude-opus-4-8"),
            model_routine: get("LLM_MODEL_ROUTINE", "glm-4.6"),
            model_triage: get("LLM_MODEL_TRIAGE", "glm-4.5-air"),
            sec_user_agent: get("SEC_EDGAR_UA", "stocks-research n@noeljackson.com"),
            fred_api_key: get("FRED_API_KEY", ""),
            anthropic_base_url: get("ANTHROPIC_BASE_URL", "https://api.anthropic.com"),
            anthropic_auth_token: get("ANTHROPIC_AUTH_TOKEN", ""),
            anthropic_version: get("ANTHROPIC_VERSION", "2023-06-01"),
            openai_base_url: get("OPENAI_BASE_URL", ""),
            openai_api_key: get("OPENAI_API_KEY", ""),
        }
    }

    /// Returns the LLM-transport subset, ready to pass to [`crate::llm::new`].
    #[must_use]
    pub fn llm(&self) -> LlmTransport {
        LlmTransport {
            provider: self.llm_provider.clone(),
            model: self.model_routine.clone(),
            anthropic_base_url: self.anthropic_base_url.clone(),
            anthropic_auth_token: self.anthropic_auth_token.clone(),
            anthropic_version: self.anthropic_version.clone(),
            openai_base_url: self.openai_base_url.clone(),
            openai_api_key: self.openai_api_key.clone(),
        }
    }
}
