//! AI relevance judge and shared prompt/verdict plumbing.
//!
//! The single production judge, [`local::OpenAiCompatJudge`], implements
//! [`AiJudge`] by calling any user-supplied OpenAI-compatible chat-completions
//! endpoint (local Ollama/LM Studio/llama-server, or a hosted provider). Pulp
//! bundles no model; AI filtering is always optional and activates only when
//! configured (see [`judge_from_config`]).
//!
//! The shared prompt construction lives here so verdicts stay byte-identical to
//! the eval suite.

use std::sync::Arc;

use crate::config::AiFilterSettings;

pub mod judge;
pub mod local;

pub use judge::{AiJudge, AiVerdict};

/// Build the relevance judge from merged config, or `None` when AI filtering is
/// disabled or incompletely configured (no endpoint / no model). Callers treat
/// `None` as "AI disabled" and fail closed. Cheap to call — the judge is just a
/// stateless HTTP client — so the settings API rebuilds it on every change.
pub fn judge_from_config(cfg: &AiFilterSettings) -> Option<Arc<dyn AiJudge>> {
    if !cfg.enabled || cfg.base_url.trim().is_empty() || cfg.model.trim().is_empty() {
        return None;
    }
    Some(Arc::new(local::OpenAiCompatJudge::new(
        cfg.base_url.clone(),
        cfg.model.clone(),
        cfg.api_key.clone(),
    )))
}

/// Mention text beyond this many characters is truncated before being sent to
/// the model (keeps the request inside the 4096-token context).
pub const MAX_TEXT_CHARS: usize = 4000;

/// System prompt framing the task as a relevance filter. The user-configured
/// watch `prompt` is embedded as the product/context section; the verdict
/// contract mirrors the prompt shape validated in `eval/PROMPT.md`. Shared by
/// the Ollama judge and the embedded llama.cpp judge so both build identical
/// prompts.
pub fn judge_system_prompt(watch_prompt: &str) -> String {
    format!(
        "You are a relevance filter for a social-listening tool.\n\n\
         Product / watch context (what the user is listening for):\n{}\n\n\
         Given a social-media mention, decide whether surfacing it to the user would be \
         genuinely relevant per the context above (\"include\") or off-topic/noise \
         (\"exclude\"). When the context describes include/exclude rules, follow them \
         strictly; when in doubt, prefer \"exclude\".\n\n\
         Respond with JSON only: \
         {{\"verdict\": \"include\" | \"exclude\", \"reason\": \"<one short sentence>\"}}",
        watch_prompt.trim()
    )
}

/// User message: the mention text, truncated to [`MAX_TEXT_CHARS`].
pub fn judge_user_message(text: &str) -> String {
    truncate_chars(text, MAX_TEXT_CHARS)
}

/// Char-boundary-safe truncation (mention text can contain multi-byte UTF-8).
pub fn truncate_chars(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((idx, _)) => s[..idx].to_string(),
        None => s.to_string(),
    }
}

/// The verdict JSON the model must emit.
#[derive(serde::Deserialize)]
struct VerdictJson {
    verdict: String,
    #[serde(default)]
    reason: Option<String>,
}

/// Robustly map model output to a full verdict: `include` -> score 1.0,
/// `exclude` -> 0.0, anything else -> `None` (fail closed). The model's
/// `reason` string is carried through when present.
///
/// Accepts the expected JSON object anywhere in the output (first `{` to last
/// `}`, tolerating chatter/code fences around it) and, as a fallback, a bare
/// `include` / `exclude` word as the entire output.
pub fn parse_verdict_full(content: &str) -> Option<crate::ai::AiVerdict> {
    let content = content.trim();

    // Primary: extract the first {...} span and parse it as the verdict JSON.
    if let (Some(start), Some(end)) = (content.find('{'), content.rfind('}')) {
        if start < end {
            if let Ok(v) = serde_json::from_str::<VerdictJson>(&content[start..=end]) {
                return verdict_to_score(&v.verdict).map(|score| crate::ai::AiVerdict {
                    score,
                    reason: v.reason.filter(|r| !r.trim().is_empty()),
                });
            }
        }
    }

    // Fallback: the whole output is just the verdict word (possibly quoted).
    let bare = content
        .trim_matches(|c: char| c.is_whitespace() || "\"'`.,:".contains(c))
        .to_lowercase();
    verdict_to_score(&bare).map(|score| crate::ai::AiVerdict {
        score,
        reason: None,
    })
}

/// Score-only variant of [`parse_verdict_full`].
pub fn parse_verdict(content: &str) -> Option<f64> {
    parse_verdict_full(content).map(|v| v.score)
}

fn verdict_to_score(verdict: &str) -> Option<f64> {
    match verdict.trim().to_lowercase().as_str() {
        "include" => Some(1.0),
        "exclude" => Some(0.0),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_is_noop_under_limit() {
        assert_eq!(truncate_chars("short", 4000), "short");
    }

    #[test]
    fn truncate_cuts_at_char_boundary() {
        let s = "é".repeat(10);
        let cut = truncate_chars(&s, 4);
        assert_eq!(cut.chars().count(), 4);
        assert_eq!(cut, "éééé");
    }

    #[test]
    fn system_prompt_embeds_watch_context() {
        let p = judge_system_prompt("  watching for desktop UI testing  ");
        assert!(p.contains("relevance filter"));
        assert!(p.contains("watching for desktop UI testing"));
        assert!(p.contains(r#""verdict": "include" | "exclude""#));
    }

    #[test]
    fn user_message_truncates() {
        let long = "x".repeat(MAX_TEXT_CHARS + 500);
        assert_eq!(judge_user_message(&long).chars().count(), MAX_TEXT_CHARS);
    }

    #[test]
    fn parse_verdict_clean_json() {
        assert_eq!(
            parse_verdict(r#"{"verdict":"include","reason":"on topic"}"#),
            Some(1.0)
        );
        assert_eq!(
            parse_verdict(r#"{"verdict":"exclude","reason":"noise"}"#),
            Some(0.0)
        );
    }

    #[test]
    fn parse_verdict_json_with_surrounding_chatter() {
        assert_eq!(
            parse_verdict(
                "Sure! Here is my verdict:\n```json\n{\"verdict\": \"include\", \"reason\": \"asks for a tool\"}\n```"
            ),
            Some(1.0)
        );
    }

    #[test]
    fn parse_verdict_bare_word() {
        assert_eq!(parse_verdict("include"), Some(1.0));
        assert_eq!(parse_verdict(" \"Exclude\". "), Some(0.0));
    }

    #[test]
    fn parse_verdict_garbage_fails_closed() {
        assert_eq!(parse_verdict("sure, that looks relevant to me!"), None);
        assert_eq!(
            parse_verdict(r#"{"verdict":"maybe","reason":"unsure"}"#),
            None
        );
        assert_eq!(parse_verdict(""), None);
        // A truncated JSON object must not pass.
        assert_eq!(
            parse_verdict(r#"{"verdict":"include","reason":"cut of"#),
            None
        );
    }
}
