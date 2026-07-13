//! Relevance judge backed by any OpenAI-compatible chat-completions endpoint.
//!
//! Implements [`AiJudge`] by calling `POST {base_url}/chat/completions` with a
//! JSON-object response format (`{"verdict": "include"|"exclude", "reason":
//! string}`). This is the de-facto universal LLM API: a local Ollama
//! (`/v1`), LM Studio, `llama-server`, or vLLM all speak it, as do hosted
//! providers (OpenAI, OpenRouter, …) — the latter via the optional bearer
//! `api_key`.
//!
//! The per-watch `prompt` supplies the product/context section of the system
//! prompt; the mention text is the user message (shared with no other runtime
//! now, but kept in [`super`] for prompt/verdict parity with the eval suite).
//! `include` maps to `1.0` and `exclude` to `0.0`, so the existing `AiLeaf`
//! threshold semantics (default 0.7) work unchanged. Any failure — connection
//! refused, timeout, non-2xx, malformed body, unexpected verdict — returns
//! `None` and the AI leaf fails closed.

use std::time::Duration;

use serde::Deserialize;

use crate::ai::{AiJudge, AiVerdict};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

/// Judge that talks to an OpenAI-compatible chat-completions endpoint.
pub struct OpenAiCompatJudge {
    base_url: String,
    model: String,
    api_key: Option<String>,
    timeout: Duration,
}

impl OpenAiCompatJudge {
    pub fn new(
        base_url: impl Into<String>,
        model: impl Into<String>,
        api_key: Option<String>,
    ) -> Self {
        Self {
            base_url: base_url.into(),
            model: model.into(),
            api_key: api_key.filter(|k| !k.trim().is_empty()),
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// Override the request timeout (tests use a short one).
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// `{base_url}/chat/completions`, tolerating a trailing slash on base_url.
    fn endpoint(&self) -> String {
        format!("{}/chat/completions", self.base_url.trim_end_matches('/'))
    }

    fn request_body(&self, prompt: &str, text: &str) -> serde_json::Value {
        // Prompt construction is shared via `ai::judge_system_prompt` /
        // `ai::judge_user_message` so prompts stay identical to the eval suite.
        serde_json::json!({
            "model": self.model,
            "messages": [
                { "role": "system", "content": super::judge_system_prompt(prompt) },
                { "role": "user", "content": super::judge_user_message(text) },
            ],
            "stream": false,
            "temperature": 0,
            // A verdict JSON object is tiny (~30-40 tokens). The budget is kept
            // generous as a safety net so that a server which insists on
            // emitting a reasoning preamble (see `reasoning_effort` below) still
            // has room to finish the JSON rather than getting cut off mid-token.
            "max_tokens": 200,
            // Turn OFF "thinking" for reasoning models (Qwen3.x, etc.). Without
            // this, such a model spends its whole token budget on hidden
            // reasoning and returns an EMPTY `content` with
            // `finish_reason: "length"`, so the judge would fail closed on every
            // call. `reasoning_effort` is a standard OpenAI Chat Completions
            // field; `"none"` is what a local Ollama (>=0.30) honors to skip
            // thinking entirely, and it's harmless on servers/models that don't
            // reason (verified ignored, not rejected, by non-reasoning models).
            "reasoning_effort": "none",
            // Ask for JSON when the server supports it; servers that ignore the
            // field still work because `parse_verdict_full` tolerates chatter.
            "response_format": { "type": "json_object" }
        })
    }
}

impl AiJudge for OpenAiCompatJudge {
    fn judge(&self, prompt: &str, text: &str) -> Option<AiVerdict> {
        // `AiJudge::judge` is sync but the notifier invokes it from a Tokio
        // worker thread, and reqwest's blocking client panics when driven from
        // inside an async runtime. Running each call on a dedicated plain OS
        // thread sidesteps that regardless of runtime flavor; the thread cost
        // is negligible next to LLM inference latency.
        let url = self.endpoint();
        let body = self.request_body(prompt, text);
        let api_key = self.api_key.clone();
        let timeout = self.timeout;
        std::thread::scope(|s| {
            s.spawn(move || call_endpoint(&url, &body, api_key.as_deref(), timeout))
                .join()
                .ok()
                .flatten()
        })
    }
}

/// Wire shape of an OpenAI chat-completions response (fields we use).
#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: ChatMessage,
}

#[derive(Deserialize)]
struct ChatMessage {
    #[serde(default)]
    content: String,
    /// Some reasoning-model servers (Ollama, vLLM) split hidden reasoning into a
    /// separate `reasoning` (or `reasoning_content`) field and leave `content`
    /// empty when the token budget is exhausted mid-thought. We send
    /// `reasoning_effort: "none"` to avoid that, but if a server ignores the
    /// hint we still try to recover a verdict the model emitted inside its
    /// reasoning text rather than failing closed.
    #[serde(default, alias = "reasoning_content")]
    reasoning: Option<String>,
}

/// Blocking call to the endpoint; every failure path logs and returns `None`
/// so the AI criteria leaf fails closed.
fn call_endpoint(
    url: &str,
    body: &serde_json::Value,
    api_key: Option<&str>,
    timeout: Duration,
) -> Option<AiVerdict> {
    let client = match reqwest::blocking::Client::builder()
        .timeout(timeout)
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("LLM judge: failed to build HTTP client: {:?}", e);
            return None;
        }
    };

    let mut req = client.post(url).json(body);
    if let Some(key) = api_key {
        req = req.bearer_auth(key);
    }

    let resp = match req.send() {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("LLM judge: request to {} failed: {:?}", url, e);
            return None;
        }
    };

    if !resp.status().is_success() {
        tracing::warn!("LLM judge: {} returned HTTP {}", url, resp.status());
        return None;
    }

    let chat: ChatResponse = match resp.json() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("LLM judge: unparseable response body: {:?}", e);
            return None;
        }
    };

    let message = match chat.choices.into_iter().next() {
        Some(c) => c.message,
        None => {
            tracing::warn!("LLM judge: response had no choices");
            return None;
        }
    };

    // Robust parse: tolerates code fences / chatter and bare verdict words.
    // Primary source is the assistant `content`. If that's empty or
    // non-conforming (e.g. a reasoning model ignored `reasoning_effort: "none"`
    // and spent its budget thinking, leaving `content` empty and the verdict —
    // if any — inside the separate `reasoning` field), fall back to scanning the
    // reasoning text so we recover a verdict instead of failing closed.
    if let Some(v) = super::parse_verdict_full(&message.content) {
        return Some(v);
    }
    if let Some(reasoning) = message.reasoning.as_deref() {
        if let Some(v) = super::parse_verdict_full(reasoning) {
            return Some(v);
        }
    }

    tracing::warn!(
        "LLM judge: model emitted non-conforming output (content={:?}, reasoning={:?})",
        message.content,
        message.reasoning,
    );
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::MAX_TEXT_CHARS;

    #[test]
    fn request_body_shape() {
        let judge = OpenAiCompatJudge::new("http://localhost:11434/v1", "llama3.2", None);
        let long_text = "x".repeat(MAX_TEXT_CHARS + 500);
        let body = judge.request_body("watching for desktop UI testing", &long_text);

        assert_eq!(body["model"], "llama3.2");
        assert_eq!(body["stream"], false);
        assert_eq!(body["temperature"], 0);
        assert_eq!(body["max_tokens"], 200);
        assert_eq!(body["response_format"]["type"], "json_object");
        // The no-think hint that lets reasoning models (Qwen3.x, …) answer
        // directly instead of burning the token budget on hidden reasoning.
        assert_eq!(body["reasoning_effort"], "none");

        let system = body["messages"][0]["content"].as_str().unwrap();
        assert!(system.contains("relevance filter"));
        assert!(system.contains("watching for desktop UI testing"));
        // Prompt parity with the shared helper.
        assert_eq!(
            system,
            crate::ai::judge_system_prompt("watching for desktop UI testing")
        );

        let user = body["messages"][1]["content"].as_str().unwrap();
        assert_eq!(user.chars().count(), MAX_TEXT_CHARS);
        assert_eq!(user, crate::ai::judge_user_message(&long_text));
    }

    #[test]
    fn endpoint_tolerates_trailing_slash() {
        let a = OpenAiCompatJudge::new("http://h/v1", "m", None);
        let b = OpenAiCompatJudge::new("http://h/v1/", "m", None);
        assert_eq!(a.endpoint(), "http://h/v1/chat/completions");
        assert_eq!(b.endpoint(), "http://h/v1/chat/completions");
    }

    #[test]
    fn blank_api_key_is_treated_as_absent() {
        let judge = OpenAiCompatJudge::new("http://h/v1", "m", Some("   ".to_string()));
        assert!(judge.api_key.is_none());
    }
}
