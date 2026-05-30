//! Runtime configuration loaded from the environment.
//!
//! Zero-config LLM: just drop a key in. If `LLM_PROVIDER` is unset,
//! [`crate::llm::new`] auto-detects the transport from the credentials
//! present (`ANTHROPIC_API_KEY` → Anthropic-shape; `OPENAI_API_KEY` +
//! `OPENAI_BASE_URL` → OpenAI-shape; nothing → mock).

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
    pub anthropic_api_key: String,
    pub anthropic_version: String,
    pub openai_base_url: String,
    pub openai_api_key: String,

    /// When true (set by `make dev`), the gateway's SPA fallback returns a
    /// 302 to `dev_ui_url` instead of serving the rust-embed'd snapshot.
    /// Stops the stale-SPA-at-:8080 footgun (#38).
    pub dev_mode: bool,
    pub dev_ui_url: String,
}

#[derive(Debug, Clone, Default)]
pub struct LlmTransport {
    /// Empty string → auto-detect from credentials. Otherwise one of:
    /// `"anthropic"`, `"openai_compat"`, `"openai"`, `"mock"`.
    pub provider: String,
    pub model: String,
    pub anthropic_base_url: String,
    pub anthropic_api_key: String,
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
    /// LLM examples:
    ///
    /// z.ai (auto-detected the moment `ANTHROPIC_API_KEY` is set):
    ///
    /// ```text
    /// ANTHROPIC_BASE_URL=https://api.z.ai/api/anthropic
    /// ANTHROPIC_API_KEY=<your z.ai key>
    /// ```
    ///
    /// DeepSeek / OpenAI-compatible (auto-detected when both are set):
    ///
    /// ```text
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
            // Empty default → auto-detect.
            llm_provider: get("LLM_PROVIDER", ""),
            model_deep: get("LLM_MODEL_DEEP", "glm-5.1"),
            model_routine: get("LLM_MODEL_ROUTINE", "glm-5.1"),
            model_triage: get("LLM_MODEL_TRIAGE", "glm-5-turbo"),
            sec_user_agent: get("SEC_EDGAR_UA", "stocks-research n@noeljackson.com"),
            fred_api_key: get("FRED_API_KEY", ""),
            anthropic_base_url: get("ANTHROPIC_BASE_URL", "https://api.z.ai/api/anthropic"),
            anthropic_api_key: get("ANTHROPIC_API_KEY", ""),
            anthropic_version: get("ANTHROPIC_VERSION", "2023-06-01"),
            openai_base_url: get("OPENAI_BASE_URL", ""),
            openai_api_key: get("OPENAI_API_KEY", ""),
            dev_mode: matches!(get("STOCKS_DEV_MODE", "").as_str(), "1" | "true" | "yes"),
            dev_ui_url: get("STOCKS_DEV_UI_URL", "http://localhost:5173"),
        }
    }

    /// Returns the LLM-transport subset, ready to pass to [`crate::llm::new`].
    #[must_use]
    pub fn llm(&self) -> LlmTransport {
        LlmTransport {
            provider: self.llm_provider.clone(),
            model: self.model_routine.clone(),
            anthropic_base_url: self.anthropic_base_url.clone(),
            anthropic_api_key: self.anthropic_api_key.clone(),
            anthropic_version: self.anthropic_version.clone(),
            openai_base_url: self.openai_base_url.clone(),
            openai_api_key: self.openai_api_key.clone(),
        }
    }
}
