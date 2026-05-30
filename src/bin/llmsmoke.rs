//! One-shot LLM round-trip smoke. Loads the env, builds whichever transport
//! is auto-detected (or `LLM_PROVIDER`-overridden), runs a single completion,
//! prints the response. No NATS, no DB.

use anyhow::Result;
use stocks::llm::{self, Message, Request};
use stocks::platform::{config::Config, logging};

#[tokio::main]
async fn main() -> Result<()> {
    logging::init("llmsmoke");
    let cfg = Config::load();
    let provider = llm::new(&cfg.llm());

    let user_msg = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "Say hello in five words.".to_string());

    let resp = provider
        .complete(Request {
            model: cfg.model_routine.clone(),
            system: "You are a terse assistant. No preamble.".to_string(),
            messages: vec![Message {
                role: "user".to_string(),
                content: user_msg,
            }],
            max_tokens: 64,
            ..Default::default()
        })
        .await?;

    println!("--- response ---");
    println!("{}", resp.content);
    println!("--- meta ---");
    println!(
        "model={} input_tokens={} output_tokens={}",
        resp.model, resp.usage.input_tokens, resp.usage.output_tokens
    );
    Ok(())
}
