//! Generic, read-only "recent log output for a service" endpoint.
//!
//! `GET /api/logs/{service}?limit=<lines>` returns the tail of the combined
//! server log (`<home>/server.log`) filtered down to the lines that belong to
//! one *service*. A "service" is an abstract log producer — today the data
//! collectors (`reddit`, `hackernews`, `github`), tomorrow the AI filter
//! (`ai_filter`/`llm`). The frontend `LogViewer` component is generic over the
//! same identifier, so a channel detail page and a future AI-filter page share
//! one component and one endpoint.
//!
//! ## Why filter one combined log instead of per-service files
//!
//! The backend writes a single `tracing` stream to `<home>/server.log` (see
//! `cli::init_serve_logging`); there are no dedicated per-service log files.
//! Each record carries its module path as the `tracing` target (e.g.
//! `pulp::collectors::reddit`, `pulp::ai_filter`), which the default `fmt`
//! layer prints into the line. So "logs for service X" is "lines of the
//! combined log that belong to X", which we resolve by matching a small set of
//! substrings (targets + a couple of message keywords) per service. Adding a
//! new service is one entry in [`resolve_service`].

use std::io::{Read, Seek, SeekFrom};
use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    Json,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::error::AppError;
use crate::state::AppState;

/// Default number of log lines returned when `limit` is omitted.
const DEFAULT_LIMIT: usize = 200;
/// Hard cap on returned lines so a huge log can't blow up the response.
const MAX_LIMIT: usize = 2000;
/// How many bytes to read off the tail of `server.log`, regardless of how
/// large the file has grown. `server.log` accumulates for the whole lifetime
/// of a `serve` instance with no rotation, so reading it whole (as this
/// endpoint used to) is unbounded memory use per request. 64 KiB comfortably
/// covers `MAX_LIMIT` lines of typical `tracing` output.
const TAIL_BYTES: u64 = 64 * 1024;

/// How a service's log lines are recognised inside the combined log: a line
/// belongs to the service if it contains ANY of these (case-insensitive)
/// substrings. Targets (`pulp::collectors::reddit`) are the primary signal;
/// extra keywords catch lines logged from a shared module (e.g.
/// `pulp::collectors` with `channel: reddit` in the message).
struct LogService {
    matchers: &'static [&'static str],
}

/// Map a service id → its log source. **To add a new service (e.g. `llm`),
/// add one arm here.** Channel names resolve to their collector targets; the
/// AI filter resolves to its worker/judge targets.
fn resolve_service(service: &str) -> Option<LogService> {
    let matchers: &'static [&'static str] = match service {
        "reddit" => &["pulp::collectors::reddit", "channel: reddit"],
        "hackernews" => &["pulp::collectors::hackernews", "channel: hackernews"],
        "github" => &["pulp::collectors::github", "channel: github"],
        // Future services — already wired so the frontend can point at them.
        "ai_filter" | "llm" => &["pulp::ai_filter", "pulp::ai"],
        _ => return None,
    };
    Some(LogService { matchers })
}

#[derive(Deserialize)]
pub struct LogQuery {
    /// Max number of lines to return (most recent last). Clamped to `MAX_LIMIT`.
    pub limit: Option<usize>,
}

/// Response body: the resolved service, the lines (oldest → newest), and
/// whether the underlying log file existed.
#[derive(Serialize, Deserialize, ToSchema)]
pub struct LogResponse {
    pub service: String,
    pub lines: Vec<String>,
    pub exists: bool,
}

/// `GET /api/logs/{service}` — last N log lines for `service`.
///
/// `service` accepts any channel name (`reddit`, `hackernews`, `github`) and
/// future services (`ai_filter`/`llm`). Unknown services → 404. The number of
/// lines is capped (default 200, max 2000).
#[utoipa::path(
    get,
    path = "/api/logs/{service}",
    tag = "logs",
    operation_id = "getServiceLogs",
    params(
        ("service" = String, Path, description = "Service id: a channel name or ai_filter/llm"),
        ("limit" = Option<usize>, Query, description = "Max lines to return (default 200, max 2000)")
    ),
    responses(
        (status = 200, body = LogResponse),
        (status = 404, description = "Unknown service")
    )
)]
pub async fn get_logs(
    State(state): State<Arc<AppState>>,
    Path(service): Path<String>,
    Query(q): Query<LogQuery>,
) -> Result<Json<LogResponse>, AppError> {
    let svc = resolve_service(&service).ok_or(AppError::NotFound)?;
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);

    let path = state.config.home.join("server.log");
    let exists = path.is_file();

    let lines = if exists {
        let content =
            read_tail(&path, TAIL_BYTES).map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
        filter_tail(&content, svc.matchers, limit)
    } else {
        Vec::new()
    };

    Ok(Json(LogResponse {
        service,
        lines,
        exists,
    }))
}

/// Read at most the last `max_bytes` of `path` — never the whole file — so
/// memory use per request stays bounded no matter how large `server.log` has
/// grown. Drops a possibly-truncated first line (the byte offset we seek to
/// almost never lands on a line boundary) and lossily re-decodes UTF-8 for
/// the same reason.
fn read_tail(path: &std::path::Path, max_bytes: u64) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    let start = len.saturating_sub(max_bytes);

    let mut buf = Vec::with_capacity((len - start) as usize);
    if start > 0 {
        file.seek(SeekFrom::Start(start))?;
    }
    file.read_to_end(&mut buf)?;

    let text = String::from_utf8_lossy(&buf);
    if start > 0 {
        // We likely seeked into the middle of a line; drop the partial
        // fragment before the first newline so every remaining line is whole.
        match text.find('\n') {
            Some(idx) => Ok(text[idx + 1..].to_string()),
            None => Ok(String::new()),
        }
    } else {
        Ok(text.into_owned())
    }
}

/// Keep the last `limit` lines of `content` that match any of `matchers`
/// (case-insensitive), stripping ANSI escape codes so the text is clean. Pure
/// (no I/O) so it's unit-testable.
fn filter_tail(content: &str, matchers: &[&str], limit: usize) -> Vec<String> {
    let mut out: Vec<String> = content
        .lines()
        .map(strip_ansi)
        .filter(|line| {
            let hay = line.to_ascii_lowercase();
            matchers
                .iter()
                .any(|m| hay.contains(&m.to_ascii_lowercase()))
        })
        .collect();
    if out.len() > limit {
        out.drain(0..out.len() - limit);
    }
    out
}

/// Remove ANSI/VT escape sequences (`ESC [ … m` etc.) from a line. The file
/// can contain ANSI-coloured copies when stdout was redirected into it.
fn strip_ansi(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // Skip an escape sequence: ESC [ ... <final byte 0x40..=0x7e>.
            if chars.peek() == Some(&'[') {
                chars.next();
                for d in chars.by_ref() {
                    if ('@'..='~').contains(&d) {
                        break;
                    }
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_tail_bounds_memory_and_drops_partial_first_line() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("pulp_logs_test_{}.log", std::process::id()));

        // 100 short numbered lines, each "line NNN\n" (9 bytes) — 900 bytes total.
        let mut content = String::new();
        for i in 0..100 {
            content.push_str(&format!("line {i:03}\n"));
        }
        std::fs::write(&path, &content).unwrap();

        // A max_bytes smaller than the file forces a mid-file seek; the
        // (possibly partial) first line in the window must be dropped, and
        // every full line after it must survive intact.
        let tail = read_tail(&path, 95).unwrap();
        assert!(
            !tail.contains("line 089"),
            "the byte window is expected to start mid-line at ~line 090"
        );
        assert!(tail.ends_with("line 099\n"), "tail = {tail:?}");
        for line in tail.lines() {
            assert!(line.starts_with("line "), "corrupted line: {line:?}");
        }

        // A max_bytes larger than the file returns the whole thing unmodified.
        let whole = read_tail(&path, 10_000).unwrap();
        assert_eq!(whole, content);

        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn unknown_service_is_none() {
        assert!(resolve_service("twitter").is_none());
        assert!(resolve_service("reddit").is_some());
        assert!(resolve_service("ai_filter").is_some());
        assert!(resolve_service("llm").is_some());
    }

    #[test]
    fn filters_by_target_and_respects_limit() {
        let log = "\
2026-06-17T06:19:35Z  INFO pulp::collectors: Starting collector for channel: reddit
2026-06-17T06:19:36Z  INFO pulp::collectors::reddit: fetched feed A
2026-06-17T06:19:37Z  INFO pulp::collectors::github: fetched repo X
2026-06-17T06:19:38Z  WARN pulp::collectors::reddit: rate limited
2026-06-17T06:19:39Z  INFO pulp::ai_filter: judged mention";

        let reddit = filter_tail(log, resolve_service("reddit").unwrap().matchers, 100);
        assert_eq!(
            reddit.len(),
            3,
            "two reddit-target lines + the channel line"
        );
        assert!(reddit.iter().all(|l| l.to_lowercase().contains("reddit")));

        // limit keeps only the most recent matches.
        let limited = filter_tail(log, resolve_service("reddit").unwrap().matchers, 1);
        assert_eq!(limited.len(), 1);
        assert!(limited[0].contains("rate limited"));

        let ai = filter_tail(log, resolve_service("ai_filter").unwrap().matchers, 100);
        assert_eq!(ai.len(), 1);
    }

    #[test]
    fn strips_ansi_escapes() {
        let line =
            "\u{1b}[2m2026-06-17\u{1b}[0m \u{1b}[32m INFO\u{1b}[0m pulp::collectors::reddit: hi";
        let cleaned = strip_ansi(line);
        assert_eq!(cleaned, "2026-06-17  INFO pulp::collectors::reddit: hi");
        assert!(!cleaned.contains('\u{1b}'));
    }
}
