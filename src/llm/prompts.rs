//! Prompt registry — `prompts/*.md` loaded at startup, hashed, callable by
//! name (#6). Every LLM call routed through here records an audit row in
//! `llm_invocation`. Same pattern as `config_version` on `market_state`:
//! every output attributable to the input prompt that produced it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

use super::{Message, Provider, Request, Response};

/// A loaded prompt file. The whole markdown body IS the template; we do
/// `{{var}}` substitution at render time. No frontmatter, no parsing burden —
/// filename stem is the name, sha256 of bytes is the version identity.
#[derive(Debug, Clone)]
pub struct Prompt {
    pub name: String,
    pub hash: String,
    template: String,
}

impl Prompt {
    /// Renders the template by substituting `{{key}}` placeholders.
    /// Unknown placeholders pass through untouched (so they're visible in the
    /// final prompt — easier to spot during prompt iteration than silent drops).
    #[must_use]
    pub fn render(&self, vars: &HashMap<&str, String>) -> String {
        let mut out = self.template.clone();
        for (k, v) in vars {
            out = out.replace(&format!("{{{{{k}}}}}"), v);
        }
        out
    }
}

/// Registry indexed by name. Built once at service startup via [`load`].
#[derive(Debug, Clone, Default)]
pub struct Registry {
    by_name: HashMap<String, Prompt>,
}

impl Registry {
    pub fn get(&self, name: &str) -> Option<&Prompt> {
        self.by_name.get(name)
    }
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.by_name.keys().map(String::as_str)
    }
    pub fn len(&self) -> usize {
        self.by_name.len()
    }
    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }
}

/// Loads every `*.md` file in `dir` into a [`Registry`]. Hash = sha256 of
/// file content (so editing the prompt changes the hash; tooling can detect
/// prompt drift across deploys).
pub fn load(dir: impl AsRef<Path>) -> Result<Registry> {
    let dir = dir.as_ref();
    let mut by_name = HashMap::new();
    let entries = std::fs::read_dir(dir).with_context(|| format!("read_dir {dir:?}"))?;
    for entry in entries {
        let entry = entry?;
        let path: PathBuf = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| anyhow::anyhow!("bad filename: {path:?}"))?
            .to_string();
        let template = std::fs::read_to_string(&path).with_context(|| format!("read {path:?}"))?;
        let mut hasher = Sha256::new();
        hasher.update(template.as_bytes());
        let hash = hex::encode(hasher.finalize());
        by_name.insert(
            name.clone(),
            Prompt {
                name,
                hash,
                template,
            },
        );
    }
    Ok(Registry { by_name })
}

/// Sink for recording an LLM invocation. The platform store implements this;
/// tests can pass `&NoopRecorder` (or `()` via the blanket impl) to skip
/// persistence.
#[async_trait::async_trait]
pub trait InvocationRecorder: Send + Sync {
    async fn record(&self, row: InvocationRow<'_>) -> Result<()>;
}

#[derive(Debug, Clone, Copy)]
pub struct InvocationRow<'a> {
    pub prompt_name: &'a str,
    pub prompt_hash: &'a str,
    pub provider: &'a str,
    pub model: &'a str,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub latency_ms: u32,
    pub request_summary: &'a str,
    pub response_summary: &'a str,
}

/// Calls the provider with a rendered prompt, optionally recording the
/// invocation. Returns the model's response unchanged. This is the *one*
/// entry point every cognition-layer service should use — keeps the audit
/// trail honest.
pub async fn invoke(
    provider: &dyn Provider,
    recorder: Option<&dyn InvocationRecorder>,
    prompt: &Prompt,
    vars: &HashMap<&str, String>,
    user_message: &str,
    provider_name: &str,
    model_override: Option<&str>,
) -> Result<Response> {
    let system = prompt.render(vars);
    let started = Instant::now();
    let resp = provider
        .complete(Request {
            model: model_override.unwrap_or_default().to_string(),
            system: system.clone(),
            messages: vec![Message {
                role: "user".to_string(),
                content: user_message.to_string(),
            }],
            ..Default::default()
        })
        .await?;
    let elapsed_ms = u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX);

    if let Some(rec) = recorder {
        let req_summary = summary(&system, 200);
        let resp_summary = summary(&resp.content, 200);
        rec.record(InvocationRow {
            prompt_name: &prompt.name,
            prompt_hash: &prompt.hash,
            provider: provider_name,
            model: &resp.model,
            input_tokens: resp.usage.input_tokens,
            output_tokens: resp.usage.output_tokens,
            latency_ms: elapsed_ms,
            request_summary: &req_summary,
            response_summary: &resp_summary,
        })
        .await?;
    }
    Ok(resp)
}

fn summary(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let cut: String = s.chars().take(n).collect();
        format!("{cut}…")
    }
}

// ---------- typed invocation with auto-retry (#28) ----------

use serde::de::DeserializeOwned;

/// Strip markdown fences + grab the first {...} or [...] block. LLMs ignore
/// "no fences" instructions sometimes; this is the pragmatic recovery.
pub fn extract_json(content: &str) -> &str {
    let s = content.trim();
    // Strip ```json or ``` opening + ``` closing.
    let s = s.strip_prefix("```json").unwrap_or(s).trim_start();
    let s = s.strip_prefix("```").unwrap_or(s).trim_start();
    let s = s.strip_suffix("```").unwrap_or(s).trim_end();
    // If still not parseable, fall back to first balanced object/array.
    if serde_json::from_str::<serde_json::Value>(s).is_ok() {
        return s;
    }
    if let Some(start) = s.find(|c| c == '{' || c == '[') {
        // Find matching close — naive but works for well-formed JSON.
        if let Some(end) = s.rfind(|c| c == '}' || c == ']') {
            if end > start {
                return &s[start..=end];
            }
        }
    }
    s
}

/// Like [`invoke`], but parses the response into `T`. On parse failure
/// re-asks the model up to `max_retries` times, each time appending the
/// parse error to the user message. Same as the `instructor` Python pattern.
///
/// `schema_hint` is a free-form string appended to the prompt's system
/// message describing the expected shape. Use a JSON schema or a verbatim
/// example — the LLM treats whatever shape you give it as authority.
pub async fn complete_typed<T: DeserializeOwned>(
    provider: &dyn Provider,
    recorder: Option<&dyn InvocationRecorder>,
    prompt: &Prompt,
    vars: &HashMap<&str, String>,
    user_message: &str,
    provider_name: &str,
    model_override: Option<&str>,
    max_retries: u32,
) -> Result<T> {
    let mut current_user = user_message.to_string();
    let mut last_err = String::new();
    for attempt in 0..=max_retries {
        let resp = invoke(
            provider,
            recorder,
            prompt,
            vars,
            &current_user,
            provider_name,
            model_override,
        )
        .await?;
        let raw = extract_json(&resp.content);
        match serde_json::from_str::<T>(raw) {
            Ok(v) => {
                if attempt > 0 {
                    tracing::info!(attempt, "complete_typed succeeded after retry");
                }
                return Ok(v);
            }
            Err(e) => {
                last_err = e.to_string();
                if attempt == max_retries {
                    return Err(anyhow::anyhow!(
                        "complete_typed: schema parse failed after {} retries: {} (raw: {})",
                        max_retries,
                        last_err,
                        super::truncate(raw, 200)
                    ));
                }
                // Append the error to the user message and retry. The LLM gets
                // a chance to see what was wrong with its previous attempt.
                current_user = format!(
                    "{}\n\n[Previous attempt failed JSON-schema validation with error: \"{}\". \
                     Reply ONLY with valid JSON matching the schema; no prose, no markdown fences.]",
                    user_message, last_err
                );
                tracing::warn!(attempt, error = %last_err, "complete_typed parse failed; retrying");
            }
        }
    }
    Err(anyhow::anyhow!(
        "complete_typed: unreachable retry loop exit ({last_err})"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::io::Write;

    fn write_prompts(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (name, body) in files {
            let path = dir.path().join(name);
            let mut f = std::fs::File::create(&path).unwrap();
            f.write_all(body.as_bytes()).unwrap();
        }
        dir
    }

    #[test]
    fn load_indexes_by_filename_stem() {
        let dir = write_prompts(&[
            ("synthesize-context.md", "do context"),
            ("draft-thesis.md", "draft a thesis for {{symbol}}"),
            ("README.txt", "not a prompt"), // non-.md ignored
        ]);
        let reg = load(dir.path()).unwrap();
        assert_eq!(reg.len(), 2);
        assert!(reg.get("synthesize-context").is_some());
        assert!(reg.get("draft-thesis").is_some());
        assert!(reg.get("README").is_none(), "non-.md files must be skipped");
    }

    #[test]
    fn hash_is_stable_and_content_addressed() {
        let dir = write_prompts(&[("p.md", "abc")]);
        let h1 = load(dir.path()).unwrap().get("p").unwrap().hash.clone();
        let h2 = load(dir.path()).unwrap().get("p").unwrap().hash.clone();
        assert_eq!(h1, h2, "same content → same hash across loads");
    }

    #[test]
    fn hash_changes_when_content_changes() {
        let dir = write_prompts(&[("p.md", "abc")]);
        let h1 = load(dir.path()).unwrap().get("p").unwrap().hash.clone();
        std::fs::write(dir.path().join("p.md"), "abcd").unwrap();
        let h2 = load(dir.path()).unwrap().get("p").unwrap().hash.clone();
        assert_ne!(h1, h2, "different content → different hash");
    }

    #[test]
    fn render_substitutes_placeholders() {
        let dir = write_prompts(&[("p.md", "hello {{name}}, you are a {{role}}")]);
        let p = load(dir.path()).unwrap().get("p").unwrap().clone();
        let vars = HashMap::from([("name", "noel".to_string()), ("role", "trader".to_string())]);
        assert_eq!(p.render(&vars), "hello noel, you are a trader");
    }

    #[test]
    fn render_passes_unknown_placeholders_through() {
        // Unknown placeholders stay visible so prompt-writers can spot them.
        let dir = write_prompts(&[("p.md", "{{wanted}} and {{also_wanted}}")]);
        let p = load(dir.path()).unwrap().get("p").unwrap().clone();
        let vars = HashMap::from([("wanted", "ok".to_string())]);
        assert_eq!(p.render(&vars), "ok and {{also_wanted}}");
    }

    #[test]
    fn load_missing_dir_errors() {
        let r = load("/this/path/does/not/exist");
        assert!(r.is_err());
    }

    // ---------- extract_json + complete_typed (#28) ----------

    #[test]
    fn extract_json_passthrough_clean() {
        assert_eq!(extract_json(r#"{"a":1}"#), r#"{"a":1}"#);
    }

    #[test]
    fn extract_json_strips_fences() {
        assert_eq!(extract_json("```json\n{\"a\":1}\n```"), r#"{"a":1}"#);
        assert_eq!(extract_json("```\n{\"a\":1}\n```"), r#"{"a":1}"#);
    }

    #[test]
    fn extract_json_finds_first_object_in_prose() {
        let s = "Sure! Here's the data: {\"a\":1,\"b\":2} — let me know.";
        assert_eq!(extract_json(s), r#"{"a":1,"b":2}"#);
    }

    #[test]
    fn extract_json_handles_arrays() {
        assert_eq!(extract_json("```json\n[1,2,3]\n```"), "[1,2,3]");
    }

    // For the retry test we need a Provider that can return scripted
    // responses. The Mock variant in this crate returns a constant — define
    // a tiny scripted provider here.
    struct ScriptedProvider {
        responses: std::sync::Mutex<Vec<String>>,
    }
    #[async_trait::async_trait]
    impl Provider for ScriptedProvider {
        async fn complete(&self, _req: Request) -> Result<Response> {
            let next = self.responses.lock().unwrap().remove(0);
            Ok(Response {
                content: next,
                model: "scripted".into(),
                usage: super::super::Usage::default(),
            })
        }
    }

    #[derive(Debug, serde::Deserialize, PartialEq)]
    struct Demo {
        n: i32,
        s: String,
    }

    fn fixture_prompt() -> Prompt {
        Prompt {
            name: "demo".into(),
            hash: "h".into(),
            template: "demo".into(),
        }
    }

    #[tokio::test]
    async fn complete_typed_succeeds_first_try() {
        let provider = ScriptedProvider {
            responses: std::sync::Mutex::new(vec![r#"{"n":42,"s":"ok"}"#.into()]),
        };
        let vars: HashMap<&str, String> = HashMap::new();
        let out: Demo = complete_typed(
            &provider,
            None,
            &fixture_prompt(),
            &vars,
            "give me demo",
            "scripted",
            None,
            2,
        )
        .await
        .unwrap();
        assert_eq!(
            out,
            Demo {
                n: 42,
                s: "ok".into()
            }
        );
    }

    #[tokio::test]
    async fn complete_typed_retries_then_succeeds() {
        let provider = ScriptedProvider {
            // First response is garbage; second is valid → succeeds on retry.
            responses: std::sync::Mutex::new(vec![
                "not even json".into(),
                r#"{"n":7,"s":"second-try"}"#.into(),
            ]),
        };
        let vars: HashMap<&str, String> = HashMap::new();
        let out: Demo = complete_typed(
            &provider,
            None,
            &fixture_prompt(),
            &vars,
            "give me demo",
            "scripted",
            None,
            2,
        )
        .await
        .unwrap();
        assert_eq!(out.n, 7);
        assert_eq!(out.s, "second-try");
    }

    #[tokio::test]
    async fn complete_typed_gives_up_after_max_retries() {
        let provider = ScriptedProvider {
            responses: std::sync::Mutex::new(vec![
                "garbage1".into(),
                "garbage2".into(),
                "garbage3".into(),
            ]),
        };
        let vars: HashMap<&str, String> = HashMap::new();
        let err = complete_typed::<Demo>(
            &provider,
            None,
            &fixture_prompt(),
            &vars,
            "go",
            "scripted",
            None,
            2, // → 3 total attempts
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("schema parse failed"), "{err}");
    }

    #[tokio::test]
    async fn complete_typed_tolerates_fenced_json() {
        let provider = ScriptedProvider {
            responses: std::sync::Mutex::new(vec![
                "```json\n{\"n\":1,\"s\":\"fenced\"}\n```".into(),
            ]),
        };
        let vars: HashMap<&str, String> = HashMap::new();
        let out: Demo = complete_typed(
            &provider,
            None,
            &fixture_prompt(),
            &vars,
            "go",
            "scripted",
            None,
            0,
        )
        .await
        .unwrap();
        assert_eq!(out.s, "fenced");
    }
}
