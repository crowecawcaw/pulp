//! The pluggable AI relevance judge: the `AiJudge` trait and its `AiVerdict`.
//!
//! This is the only external dependency of the monitor-level AI filter
//! (`crate::ai_filter`): production wires a real LLM here (see
//! [`crate::ai::local::OpenAiCompatJudge`]), tests inject a deterministic stub.
//! It lives in its own module so the AI filter does not depend on any alerting
//! machinery.

/// A full judgment: relevance score in `[0, 1]` plus the model's one-sentence
/// explanation when the runtime surfaces one.
#[derive(Debug, Clone)]
pub struct AiVerdict {
    pub score: f64,
    pub reason: Option<String>,
}

/// Pluggable relevance provider. `judge` returns `None` when no provider is
/// available / the call failed (callers fail closed).
pub trait AiJudge: Send + Sync {
    fn judge(&self, prompt: &str, text: &str) -> Option<AiVerdict>;

    /// Score-only convenience.
    fn score(&self, prompt: &str, text: &str) -> Option<f64> {
        self.judge(prompt, text).map(|v| v.score)
    }

    /// Whether the judge can currently produce verdicts at all (e.g. the
    /// managed runtime's model file has finished downloading). Callers that
    /// retry — like the ingest AI filter — use this to wait instead of
    /// burning attempts while the model is still being fetched.
    fn available(&self) -> bool {
        true
    }
}
