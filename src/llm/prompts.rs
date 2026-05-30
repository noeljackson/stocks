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
        by_name.insert(name.clone(), Prompt { name, hash, template });
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
}
