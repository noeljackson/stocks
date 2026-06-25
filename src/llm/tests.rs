//! Transport tests using wiremock. Mirrors the 12 Go tests.

use pretty_assertions::assert_eq;
use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::{AnthropicProvider, Message, MockProvider, OpenAiCompatProvider, Provider, Request};
use crate::platform::config::LlmTransport;

fn anthropic_happy() -> serde_json::Value {
    json!({
        "id": "msg_01",
        "type": "message",
        "role": "assistant",
        "content": [{"type": "text", "text": "hello world"}],
        "model": "glm-5.1",
        "stop_reason": "end_turn",
        "usage": {"input_tokens": 12, "output_tokens": 3}
    })
}

fn openai_happy() -> serde_json::Value {
    json!({
        "id": "cmpl_01",
        "choices": [{
            "message": {"role": "assistant", "content": "hi back"},
            "finish_reason": "stop",
            "index": 0
        }],
        "model": "deepseek-chat",
        "usage": {"prompt_tokens": 7, "completion_tokens": 2, "total_tokens": 9}
    })
}

fn anthropic_cfg(base_url: String, key: &str) -> LlmTransport {
    LlmTransport {
        provider: "anthropic".into(),
        model: "glm-5.1".into(),
        anthropic_base_url: base_url,
        anthropic_api_key: key.into(),
        anthropic_version: "2023-06-01".into(),
        ..Default::default()
    }
}

fn openai_cfg(base_url: String, key: &str) -> LlmTransport {
    LlmTransport {
        provider: "openai_compat".into(),
        model: "deepseek-chat".into(),
        openai_base_url: base_url,
        openai_api_key: key.into(),
        ..Default::default()
    }
}

// ---------- anthropic ----------

#[tokio::test]
async fn anthropic_happy_path() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(anthropic_happy()))
        .mount(&server)
        .await;

    let p = AnthropicProvider::try_new(&anthropic_cfg(server.uri(), "test-token")).unwrap();
    let r = p
        .complete(Request {
            system: "you are precise".into(),
            messages: vec![Message {
                role: "user".into(),
                content: "say hello".into(),
            }],
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(r.content, "hello world");
    assert_eq!(r.usage.input_tokens, 12);
    assert_eq!(r.usage.output_tokens, 3);

    let reqs = server.received_requests().await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&reqs[0].body).unwrap();
    assert_eq!(body["model"], "glm-5.1");
    assert_eq!(body["system"], "you are precise");
    assert!(
        body.get("max_tokens").is_some(),
        "anthropic requires max_tokens"
    );
}

#[tokio::test]
async fn anthropic_sends_api_key_and_version() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("x-api-key", "test-token"))
        .and(header("anthropic-version", "2023-06-01"))
        .respond_with(ResponseTemplate::new(200).set_body_json(anthropic_happy()))
        .mount(&server)
        .await;

    let p = AnthropicProvider::try_new(&anthropic_cfg(server.uri(), "test-token")).unwrap();
    p.complete(Request {
        messages: vec![Message {
            role: "user".into(),
            content: "x".into(),
        }],
        ..Default::default()
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn anthropic_http_error_propagates() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(401).set_body_json(
            json!({"error": {"type": "authentication_error", "message": "invalid key"}}),
        ))
        .mount(&server)
        .await;

    let p = AnthropicProvider::try_new(&anthropic_cfg(server.uri(), "x")).unwrap();
    let err = p
        .complete(Request {
            messages: vec![Message {
                role: "user".into(),
                content: "x".into(),
            }],
            ..Default::default()
        })
        .await
        .unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("401"), "error should include status: {msg}");
    assert!(
        msg.contains("invalid key"),
        "error should include body fragment: {msg}"
    );
}

#[tokio::test]
async fn anthropic_json_schema_appends_to_system() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(anthropic_happy()))
        .mount(&server)
        .await;

    let p = AnthropicProvider::try_new(&anthropic_cfg(server.uri(), "x")).unwrap();
    let schema = json!({"type": "object", "properties": {"x": {"type": "number"}}});
    p.complete(Request {
        system: "be helpful".into(),
        messages: vec![Message {
            role: "user".into(),
            content: "give me x".into(),
        }],
        json_schema: Some(schema),
        ..Default::default()
    })
    .await
    .unwrap();

    let reqs = server.received_requests().await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&reqs[0].body).unwrap();
    let sys = body["system"].as_str().unwrap();
    assert!(sys.contains("be helpful"), "must keep caller's text");
    assert!(
        sys.contains("JSON") && sys.contains("schema"),
        "must add schema directive: {sys}"
    );
}

#[test]
fn anthropic_missing_key_returns_mock() {
    let p = super::new(&LlmTransport {
        provider: "anthropic".into(),
        ..Default::default()
    });
    // Without a key we expect Mock; verify by content (Mock returns {"mock":true}).
    let fut = p.complete(Request::default());
    let r = futures::executor::block_on(fut).unwrap();
    assert_eq!(r.content, r#"{"mock":true}"#);
}

// ---------- auto-detect ----------

#[test]
fn detect_anthropic_when_key_present() {
    let cfg = LlmTransport {
        anthropic_api_key: "k".into(),
        ..Default::default()
    };
    assert_eq!(super::detect(&cfg), "anthropic");
}

#[test]
fn detect_openai_when_both_present() {
    let cfg = LlmTransport {
        openai_base_url: "https://x".into(),
        openai_api_key: "k".into(),
        ..Default::default()
    };
    assert_eq!(super::detect(&cfg), "openai_compat");
}

#[test]
fn detect_anthropic_wins_over_openai() {
    // Both creds present → anthropic wins (it's the project default).
    let cfg = LlmTransport {
        anthropic_api_key: "ak".into(),
        openai_base_url: "https://x".into(),
        openai_api_key: "ok".into(),
        ..Default::default()
    };
    assert_eq!(super::detect(&cfg), "anthropic");
}

#[test]
fn detect_openai_needs_both_base_and_key() {
    // Only base, no key → mock (don't pretend we can call).
    let cfg = LlmTransport {
        openai_base_url: "https://x".into(),
        ..Default::default()
    };
    assert_eq!(super::detect(&cfg), "mock");
    // Only key, no base → mock.
    let cfg = LlmTransport {
        openai_api_key: "k".into(),
        ..Default::default()
    };
    assert_eq!(super::detect(&cfg), "mock");
}

#[test]
fn detect_falls_back_to_mock_with_nothing_set() {
    assert_eq!(super::detect(&LlmTransport::default()), "mock");
}

#[test]
fn explicit_provider_overrides_detect() {
    // Anthropic key present but caller forces openai_compat → still goes to
    // openai branch (which then falls back to mock for missing creds).
    let cfg = LlmTransport {
        provider: "openai_compat".into(),
        anthropic_api_key: "would-have-picked-this".into(),
        ..Default::default()
    };
    // The factory chooses openai_compat → tries to build it → no openai creds
    // → returns Mock. We assert by observable behavior (Mock content).
    let p = super::new(&cfg);
    let r = futures::executor::block_on(p.complete(Request::default())).unwrap();
    assert_eq!(r.content, r#"{"mock":true}"#);
}

// ---------- openai-compat ----------

#[tokio::test]
async fn openai_happy_path() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(openai_happy()))
        .mount(&server)
        .await;

    let p = OpenAiCompatProvider::try_new(&openai_cfg(server.uri(), "sk-test")).unwrap();
    let r = p
        .complete(Request {
            system: "be terse".into(),
            messages: vec![Message {
                role: "user".into(),
                content: "say hi".into(),
            }],
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(r.content, "hi back");
    assert_eq!(r.usage.input_tokens, 7);
    assert_eq!(r.usage.output_tokens, 2);

    let reqs = server.received_requests().await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&reqs[0].body).unwrap();
    let msgs = body["messages"].as_array().unwrap();
    assert_eq!(msgs.len(), 2, "system must be flattened into messages[0]");
    assert_eq!(msgs[0]["role"], "system");
    assert_eq!(msgs[0]["content"], "be terse");
}

#[tokio::test]
async fn openai_sends_bearer() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(header("authorization", "Bearer sk-test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(openai_happy()))
        .mount(&server)
        .await;

    let p = OpenAiCompatProvider::try_new(&openai_cfg(server.uri(), "sk-test")).unwrap();
    p.complete(Request {
        messages: vec![Message {
            role: "user".into(),
            content: "x".into(),
        }],
        ..Default::default()
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn openai_strips_v1_suffix() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(openai_happy()))
        .mount(&server)
        .await;

    // Caller appended /v1 by mistake — final URL must still be /v1/chat/completions exactly once.
    let p =
        OpenAiCompatProvider::try_new(&openai_cfg(format!("{}/v1", server.uri()), "k")).unwrap();
    p.complete(Request {
        messages: vec![Message {
            role: "user".into(),
            content: "x".into(),
        }],
        ..Default::default()
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn openai_http_error_propagates() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(429).set_body_json(json!({"error": {"message": "rate limited"}})),
        )
        .mount(&server)
        .await;

    let p = OpenAiCompatProvider::try_new(&openai_cfg(server.uri(), "k")).unwrap();
    let err = p
        .complete(Request {
            messages: vec![Message {
                role: "user".into(),
                content: "x".into(),
            }],
            ..Default::default()
        })
        .await
        .unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("429"), "error should include status: {msg}");
    assert!(
        msg.contains("rate"),
        "error should include body fragment: {msg}"
    );
}

#[test]
fn openai_missing_base_returns_mock() {
    let p = super::new(&LlmTransport {
        provider: "openai_compat".into(),
        openai_api_key: "k".into(),
        ..Default::default()
    });
    let r = futures::executor::block_on(p.complete(Request::default())).unwrap();
    assert_eq!(r.content, r#"{"mock":true}"#);
}

#[test]
fn openai_missing_key_returns_mock() {
    let p = super::new(&LlmTransport {
        provider: "openai_compat".into(),
        openai_base_url: "https://x".into(),
        ..Default::default()
    });
    let r = futures::executor::block_on(p.complete(Request::default())).unwrap();
    assert_eq!(r.content, r#"{"mock":true}"#);
}

#[test]
fn factory_unknown_provider_returns_mock() {
    // Verify by observable behavior — Mock returns a known fixed payload.
    let p = super::new(&LlmTransport {
        provider: "???".into(),
        ..Default::default()
    });
    let r = futures::executor::block_on(p.complete(Request::default())).unwrap();
    assert_eq!(r.content, r#"{"mock":true}"#);
}

#[test]
fn factory_mock_provider_constructable() {
    // Pin the type so refactors don't silently change the default.
    let m = MockProvider;
    let r = futures::executor::block_on(m.complete(Request::default())).unwrap();
    assert_eq!(r.content, r#"{"mock":true}"#);
    assert_eq!(r.model, "mock");
}
