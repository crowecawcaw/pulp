//! Shared CLI helpers: time parsing, text formatting, JSON loading.

use anyhow::{bail, Context};
use std::io::Write;

/// Parse a relative age like `7d` / `12h` / `30m` / `45s` / `2w` into seconds.
pub fn parse_duration_secs(s: &str) -> Option<i64> {
    // Split off the last CHAR, not the last byte: `s.split_at(s.len() - 1)`
    // indexes by byte length, which panics if the final char is multibyte
    // (e.g. a Cyrillic suffix like "7д") because the split point lands
    // mid-character. `chars().next_back()` + the remaining `as_str()` finds
    // the char boundary correctly regardless of encoding width.
    let mut chars = s.chars();
    let unit = chars.next_back()?;
    let num = chars.as_str();
    if num.is_empty() {
        return None;
    }
    let mult = match unit {
        's' => 1,
        'm' => 60,
        'h' => 3_600,
        'd' => 86_400,
        'w' => 604_800,
        _ => return None,
    };
    num.parse::<i64>()
        .ok()
        .filter(|n| *n >= 0)
        .map(|n| n * mult)
}

/// Resolve a user-supplied point in time to epoch seconds. Accepts a relative
/// age (`7d`, `12h`, `30m`), epoch seconds, or a `YYYY-MM-DD` date (UTC).
pub fn parse_since(s: &str) -> anyhow::Result<i64> {
    let s = s.trim();
    if let Some(secs) = parse_duration_secs(s) {
        return Ok(chrono::Utc::now().timestamp() - secs);
    }
    if let Ok(epoch) = s.parse::<i64>() {
        return Ok(epoch);
    }
    if let Ok(date) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return Ok(date.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp());
    }
    bail!(
        "invalid time '{}': use a relative age (7d, 12h, 30m), epoch seconds, or YYYY-MM-DD",
        s
    )
}

/// Epoch seconds -> `2026-06-09 14:02` (UTC), or `-` when absent.
pub fn fmt_ts(epoch: Option<i64>) -> String {
    match epoch.and_then(|e| chrono::DateTime::<chrono::Utc>::from_timestamp(e, 0)) {
        Some(dt) => dt.format("%Y-%m-%d %H:%M").to_string(),
        None => "-".to_string(),
    }
}

/// Collapse whitespace and truncate to `max` chars for single-line display.
pub fn snippet(text: &str, max: usize) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= max {
        collapsed
    } else {
        let cut: String = collapsed.chars().take(max).collect();
        format!("{}...", cut.trim_end())
    }
}

pub fn print_json<T: serde::Serialize>(out: &mut dyn Write, value: &T) -> anyhow::Result<()> {
    writeln!(out, "{}", serde_json::to_string_pretty(value)?)?;
    Ok(())
}

/// Load a JSON argument: inline JSON or `@path/to/file.json`. `what` names the
/// flag in error messages.
pub fn load_json(arg: &str, what: &str) -> anyhow::Result<serde_json::Value> {
    let text = match arg.strip_prefix('@') {
        Some(path) => std::fs::read_to_string(path)
            .with_context(|| format!("reading {} file {}", what, path))?,
        None => arg.to_string(),
    };
    serde_json::from_str(&text).with_context(|| format!("{} is not valid JSON", what))
}

/// Error early (with the full channel list) on a channel name no collector
/// exists for.
pub fn ensure_known_channel(channel: &str) -> anyhow::Result<()> {
    if crate::collectors::CHANNELS.contains(&channel) {
        Ok(())
    } else {
        bail!(
            "unknown channel '{}' — available channels: {}",
            channel,
            crate::collectors::CHANNELS.join(", ")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_parse() {
        assert_eq!(parse_duration_secs("45s"), Some(45));
        assert_eq!(parse_duration_secs("30m"), Some(1_800));
        assert_eq!(parse_duration_secs("12h"), Some(43_200));
        assert_eq!(parse_duration_secs("7d"), Some(604_800));
        assert_eq!(parse_duration_secs("2w"), Some(1_209_600));
        assert_eq!(parse_duration_secs("d"), None);
        assert_eq!(parse_duration_secs("7x"), None);
        assert_eq!(parse_duration_secs("7"), None);
        assert_eq!(parse_duration_secs("-7d"), None);
    }

    #[test]
    fn duration_with_multibyte_suffix_does_not_panic() {
        // `split_at(len - 1)` used to index by BYTE length, which panics when
        // the final char is multibyte (a Cyrillic "д" is 2 bytes) because the
        // split point lands mid-character. It should gracefully reject the
        // unrecognized unit instead of panicking.
        assert_eq!(parse_duration_secs("7д"), None);
        assert!(parse_since("7д").is_err());
        // A lone multibyte char (no numeric prefix) must not panic either.
        assert_eq!(parse_duration_secs("д"), None);
    }

    #[test]
    fn since_accepts_relative_epoch_and_date() {
        let now = chrono::Utc::now().timestamp();
        let week_ago = parse_since("7d").unwrap();
        assert!((now - 604_800 - week_ago).abs() < 5);

        assert_eq!(parse_since("1700000000").unwrap(), 1_700_000_000);
        // 2026-01-01T00:00:00Z
        assert_eq!(parse_since("2026-01-01").unwrap(), 1_767_225_600);
        assert!(parse_since("yesterday").is_err());
    }

    #[test]
    fn snippet_collapses_and_truncates() {
        assert_eq!(snippet("a  b\n\nc", 10), "a b c");
        assert_eq!(snippet("hello world", 5), "hello...");
    }

    #[test]
    fn channel_validation() {
        assert!(ensure_known_channel("reddit").is_ok());
        assert!(ensure_known_channel("twitter").is_err());
    }
}
