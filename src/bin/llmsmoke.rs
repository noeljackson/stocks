//! One-shot LLM round-trip smoke (#6 demo).
//!
//! Loads the prompt registry, picks `prompts/echo.md` by default, renders it
//! with `expected="..."`, calls the auto-detected provider, prints the
//! response. If `DATABASE_URL` is reachable, records an `llm_invocation` row
//! so we can verify prompt_name + prompt_hash are stamped on the audit trail.
//!
//! Usage: `llmsmoke [prompt_name] [user_message]`
//!   defaults: prompt_name=echo, user_message="PING"

use std::collections::HashMap;

use anyhow::Result;
use stocks::llm::{self, prompts};
use stocks::platform::{config::Config, logging, store::Store};

#[tokio::main]
async fn main() -> Result<()> {
    logging::init("llmsmoke");
    let cfg = Config::load();

    let args: Vec<String> = std::env::args().collect();
    let prompt_name = args.get(1).map(String::as_str).unwrap_or("echo");
    let user_msg = args.get(2).cloned().unwrap_or_else(|| "PING".to_string());

    let registry = prompts::load("prompts")?;
    let prompt = registry
        .get(prompt_name)
        .ok_or_else(|| anyhow::anyhow!("prompt {prompt_name} not in registry"))?;

    tracing::info!(
        prompt_name = %prompt.name,
        prompt_hash = %&prompt.hash[..12],   // short hash in logs
        "prompt loaded"
    );

    let provider = llm::new(&cfg.llm());
    // Best-effort DB connection — the audit recorder is optional so llmsmoke
    // still works without postgres reachable.
    let store = Store::connect(&cfg.database_url).await.ok();
    let recorder: Option<&dyn prompts::InvocationRecorder> =
        store.as_ref().map(|s| s as &dyn prompts::InvocationRecorder);

    // echo.md uses `{{expected}}`; the user message ALSO doubles as the
    // expected reply token so the smoke is a real round-trip check.
    let mut vars = HashMap::new();
    vars.insert("expected", user_msg.clone());

    let provider_name = if cfg.llm_provider.is_empty() {
        // Mirror the auto-detect for log attribution.
        if !cfg.anthropic_api_key.is_empty() {
            "anthropic"
        } else if !cfg.openai_base_url.is_empty() && !cfg.openai_api_key.is_empty() {
            "openai_compat"
        } else {
            "mock"
        }
    } else {
        cfg.llm_provider.as_str()
    };

    let resp = prompts::invoke(
        provider.as_ref(),
        recorder,
        prompt,
        &vars,
        &user_msg,
        provider_name,
        Some(&cfg.model_routine),
    )
    .await?;

    println!("--- response ---");
    println!("{}", resp.content);
    println!("--- meta ---");
    println!(
        "prompt={}@{} model={} provider={} input_tokens={} output_tokens={}",
        prompt.name,
        &prompt.hash[..12],
        resp.model,
        provider_name,
        resp.usage.input_tokens,
        resp.usage.output_tokens,
    );
    if recorder.is_some() {
        println!("audit: row written to llm_invocation");
    } else {
        println!("audit: DB unreachable, no row written");
    }
    Ok(())
}
