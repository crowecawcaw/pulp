//! `pulp config` — view and edit runtime configuration.
//!
//! Today this covers exactly the optional AI relevance filter (bring-your-own
//! OpenAI-compatible LLM endpoint); it replaces the old `pulp llm` command.
//! Like that command it talks to a running server's `/api/config/ai` endpoints,
//! so a `set` persists to `~/.pulp/config.json` *and* hot-swaps the live
//! judge without a restart.
//!
//! The interface is a flat key/value store over those settings. Valid keys:
//!
//! | key        | type   | meaning                                         |
//! |------------|--------|-------------------------------------------------|
//! | `enabled`  | bool   | turn the AI filter on/off                       |
//! | `base_url` | string | OpenAI-compatible base URL                       |
//! | `model`    | string | model name to request                           |
//! | `api_key`  | string | bearer token for hosted endpoints (`""` clears) |

use anyhow::bail;
use clap::Subcommand;
use std::io::Write;

use crate::api::config::{AiConfigUpdate, AiConfigView, AiTestResult};
use crate::config::AiFilterSection;

use super::client::ApiClient;
use super::util;

pub const LONG_ABOUT: &str = "\
View and edit Pulp configuration. Currently this manages the optional AI
relevance filter — Pulp bundles no model, so you point it at any endpoint
that speaks the OpenAI Chat Completions API: a local server (Ollama at /v1, LM
Studio, llama-server, vLLM) or a hosted provider (OpenAI, OpenRouter, …). The
filter stays off until `enabled` is true with both `base_url` and `model` set.

Changes are saved to ~/.pulp/config.json and applied immediately — no
server restart needed.

KEYS:
  enabled    bool    turn the AI relevance filter on/off
  base_url   string  OpenAI-compatible base URL, e.g. http://localhost:11434/v1
  model      string  model name to request, e.g. llama3.2 or gpt-4o-mini
  api_key    string  bearer token for hosted endpoints (set to \"\" to clear)

EXAMPLES:
  pulp config                                   # list every key, value + default
  pulp config get model
  pulp config set base_url http://localhost:11434/v1
  pulp config set model llama3.2
  pulp config set enabled true
  pulp config set api_key sk-...                # hosted endpoints
  pulp config test                              # verify connectivity";

/// Subcommand of `pulp config`. Omit it (`pulp config`) to list
/// everything.
#[derive(Subcommand, Debug)]
pub enum ConfigCmd {
    /// List every option with its current and default value (the default)
    List,
    /// Print one option's current value
    Get {
        /// One of: enabled, base_url, model, api_key
        key: String,
    },
    /// Set one option's value (persisted + applied live)
    Set {
        /// One of: enabled, base_url, model, api_key
        key: String,
        /// New value (`enabled` accepts true/false; `api_key ""` clears it)
        value: String,
    },
    /// Send a sample mention to the configured endpoint to verify connectivity
    Test,
}

/// Keys this command exposes, in display order. Kept in one place so `list`,
/// `get`, and `set` can't drift on the spelling.
const KEYS: [&str; 4] = ["enabled", "base_url", "model", "api_key"];

pub async fn run(
    cmd: Option<ConfigCmd>,
    client: &ApiClient,
    json: bool,
    out: &mut dyn Write,
) -> anyhow::Result<()> {
    match cmd.unwrap_or(ConfigCmd::List) {
        ConfigCmd::List => list(client, json, out).await,
        ConfigCmd::Get { key } => get(&key, client, json, out).await,
        ConfigCmd::Set { key, value } => set(&key, &value, client, json, out).await,
        ConfigCmd::Test => test(client, json, out).await,
    }
}

async fn list(client: &ApiClient, json: bool, out: &mut dyn Write) -> anyhow::Result<()> {
    let cur: AiConfigView = client.get("/api/config/ai").await?;
    let def = AiFilterSection::default();
    if json {
        util::print_json(
            out,
            &serde_json::json!({
                "current": {
                    "enabled": cur.enabled,
                    "base_url": cur.base_url,
                    "model": cur.model,
                    "api_key_set": cur.api_key_set,
                },
                "defaults": {
                    "enabled": def.enabled,
                    "base_url": def.base_url,
                    "model": def.model,
                    "api_key_set": def.api_key.is_some(),
                },
            }),
        )?;
        return Ok(());
    }
    writeln!(out, "AI relevance filter (bring-your-own LLM endpoint):")?;
    writeln!(out, "  {:<9} {:<34} DEFAULT", "KEY", "VALUE")?;
    let rows = [
        ("enabled", cur.enabled.to_string(), def.enabled.to_string()),
        ("base_url", or_unset(&cur.base_url), or_unset(&def.base_url)),
        ("model", or_unset(&cur.model), or_unset(&def.model)),
        (
            "api_key",
            secret_state(cur.api_key_set),
            secret_state(def.api_key.is_some()),
        ),
    ];
    for (key, value, default) in rows {
        writeln!(out, "  {:<9} {:<34} {}", key, value, default)?;
    }
    Ok(())
}

async fn get(key: &str, client: &ApiClient, json: bool, out: &mut dyn Write) -> anyhow::Result<()> {
    ensure_known_key(key)?;
    let cur: AiConfigView = client.get("/api/config/ai").await?;
    // The API never returns the stored api_key; report only whether one is set.
    let value = match key {
        "enabled" => serde_json::Value::Bool(cur.enabled),
        "base_url" => serde_json::Value::String(cur.base_url),
        "model" => serde_json::Value::String(cur.model),
        "api_key" => serde_json::Value::Bool(cur.api_key_set),
        _ => unreachable!("validated by ensure_known_key"),
    };
    if json {
        util::print_json(out, &serde_json::json!({ "key": key, "value": value }))?;
    } else if key == "api_key" {
        writeln!(out, "{}", secret_state(cur.api_key_set))?;
    } else if let serde_json::Value::String(s) = &value {
        writeln!(out, "{}", or_unset(s))?;
    } else {
        writeln!(out, "{}", value)?;
    }
    Ok(())
}

async fn set(
    key: &str,
    value: &str,
    client: &ApiClient,
    json: bool,
    out: &mut dyn Write,
) -> anyhow::Result<()> {
    ensure_known_key(key)?;
    let mut body = AiConfigUpdate {
        enabled: None,
        base_url: None,
        model: None,
        api_key: None,
    };
    match key {
        "enabled" => body.enabled = Some(parse_bool(value)?),
        "base_url" => body.base_url = Some(value.to_string()),
        "model" => body.model = Some(value.to_string()),
        // An empty string clears the stored key (handled server-side).
        "api_key" => body.api_key = Some(value.to_string()),
        _ => unreachable!("validated by ensure_known_key"),
    }
    let cfg: AiConfigView = client.put("/api/config/ai", &body).await?;
    if json {
        util::print_json(out, &cfg)?;
    } else {
        writeln!(out, "set {} — current configuration:", key)?;
        print_view(out, &cfg)?;
    }
    Ok(())
}

async fn test(client: &ApiClient, json: bool, out: &mut dyn Write) -> anyhow::Result<()> {
    let res: AiTestResult = client
        .post("/api/config/ai/test", &serde_json::json!({}))
        .await?;
    if json {
        util::print_json(out, &res)?;
    } else if res.ok {
        let reason = res.reason.map(|r| format!(" ({})", r)).unwrap_or_default();
        writeln!(
            out,
            "OK — endpoint returned verdict '{}'{}",
            res.verdict.as_deref().unwrap_or("?"),
            reason
        )?;
    } else {
        writeln!(
            out,
            "FAILED — {}",
            res.error.as_deref().unwrap_or("unknown error")
        )?;
    }
    Ok(())
}

fn ensure_known_key(key: &str) -> anyhow::Result<()> {
    if KEYS.contains(&key) {
        Ok(())
    } else {
        bail!(
            "unknown config key '{}' — valid keys: {}",
            key,
            KEYS.join(", ")
        )
    }
}

/// Parse a permissive boolean for `config set enabled <value>`.
fn parse_bool(value: &str) -> anyhow::Result<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "on" | "yes" | "y" => Ok(true),
        "false" | "0" | "off" | "no" | "n" => Ok(false),
        other => bail!(
            "invalid boolean '{}' for 'enabled' — use true or false",
            other
        ),
    }
}

fn or_unset(s: &str) -> String {
    if s.is_empty() {
        "(unset)".to_string()
    } else {
        s.to_string()
    }
}

fn secret_state(set: bool) -> String {
    if set {
        "set".to_string()
    } else {
        "(unset)".to_string()
    }
}

fn print_view(out: &mut dyn Write, cfg: &AiConfigView) -> std::io::Result<()> {
    writeln!(out, "  enabled:  {}", cfg.enabled)?;
    writeln!(out, "  base_url: {}", or_unset(&cfg.base_url))?;
    writeln!(out, "  model:    {}", or_unset(&cfg.model))?;
    writeln!(out, "  api_key:  {}", secret_state(cfg.api_key_set))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bool_parsing_is_permissive_but_strict_on_garbage() {
        for t in ["true", "TRUE", "1", "on", "yes", "y"] {
            assert_eq!(parse_bool(t).unwrap(), true, "{t}");
        }
        for f in ["false", "False", "0", "off", "no", "n"] {
            assert_eq!(parse_bool(f).unwrap(), false, "{f}");
        }
        assert!(parse_bool("maybe").is_err());
    }

    #[test]
    fn unknown_keys_are_rejected_with_the_valid_list() {
        for k in KEYS {
            assert!(ensure_known_key(k).is_ok());
        }
        let err = ensure_known_key("temperature").unwrap_err().to_string();
        assert!(err.contains("temperature"));
        assert!(err.contains("base_url"));
    }
}
