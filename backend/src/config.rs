//! Runtime configuration.
//!
//! Pulp keeps its persistent state in an app home directory:
//!
//! ```text
//! ~/.pulp/            (override with PULP_HOME)
//! ├── config.json         user-editable config; written with defaults on first run
//! ├── pulp.db         default SQLite database location
//! ├── vapid.json          Web Push VAPID keypair, generated on first run
//! ├── server.log          serve's log stream (also on stdout); `pulp logs`
//! └── certs/              HTTPS certs auto-provisioned via `tailscale cert`
//! ```
//!
//! Precedence (lowest to highest): built-in defaults < `config.json` <
//! environment variables. Env vars (`DATABASE_URL`, `BIND`,
//! `PULP_LLM_BASE_URL`, `PULP_LLM_MODEL`, `PULP_LLM_API_KEY`, ...)
//! always win over `config.json`.

use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};

/// Default base URL for the AI relevance filter's LLM endpoint. Points at a
/// local Ollama server's OpenAI-compatible API; override via `ai_filter.base_url`
/// in config.json or the `PULP_LLM_BASE_URL` env var. The filter stays off
/// until `ai_filter.enabled` is set and a `model` is chosen.
pub const DEFAULT_LLM_BASE_URL: &str = "http://localhost:11434/v1";

// ---------------------------------------------------------------------------
// App home resolution
// ---------------------------------------------------------------------------

/// Resolve the Pulp home directory: the `PULP_HOME` env var if set
/// (and non-empty), else `~/.pulp` (`USERPROFILE` on Windows, `$HOME` on
/// Unix).
pub fn resolve_home() -> anyhow::Result<PathBuf> {
    if let Ok(custom) = std::env::var("PULP_HOME") {
        if !custom.trim().is_empty() {
            return Ok(PathBuf::from(custom));
        }
    }
    let user_home = home::home_dir()
        .context("could not determine the user home directory; set PULP_HOME explicitly")?;
    Ok(user_home.join(".pulp"))
}

/// Read `<home>/config.json` if it exists, falling back to built-in defaults.
/// Unlike [`Config::load`] this never creates files or applies env overrides —
/// it's the cheap read used by the CLI client and `serve` control commands that
/// only need a couple of fields (server host/port) without standing the server
/// up. A missing or malformed file yields defaults.
pub fn load_file_config() -> FileConfig {
    resolve_home()
        .ok()
        .map(|home| home.join("config.json"))
        .filter(|p| p.exists())
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// The fixed `host:port` `pulp serve` binds (and CLI control commands probe).
/// Same precedence as [`Config::merge`]'s `bind`: `BIND` env > config.json
/// `server.host:port` > built-in default — so the value here always matches the
/// address the running server actually listens on.
pub fn resolve_bind() -> String {
    if let Ok(b) = std::env::var("BIND") {
        if !b.trim().is_empty() {
            return b;
        }
    }
    let server = load_file_config().server;
    format!("{}:{}", server.host, server.port)
}

/// Create the app home directory. Idempotent.
pub fn ensure_home_dirs(home: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(home)
        .with_context(|| format!("failed to create app directory {}", home.display()))?;
    Ok(())
}

/// Build a sqlx-compatible SQLite URL from a filesystem path. Backslashes are
/// normalized to forward slashes so Windows paths survive URL parsing.
fn sqlite_url(path: &Path) -> String {
    format!("sqlite:{}", path.display().to_string().replace('\\', "/"))
}

// ---------------------------------------------------------------------------
// config.json schema
// ---------------------------------------------------------------------------

/// On-disk schema of `<home>/config.json`. Every field is optional and falls
/// back to its default, so a partial file is valid. Unknown fields are
/// ignored (forward compatibility).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FileConfig {
    pub server: ServerSection,
    /// SQLite database file path. `null` => `<home>/pulp.db`.
    pub database_path: Option<String>,
    pub ai_filter: AiFilterSection,
    pub webpush: WebPushSection,
}

/// Web Push settings (`webpush` in config.json).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WebPushSection {
    /// VAPID `sub` contact embedded in push JWTs (RFC 8292) — a `mailto:` or
    /// `https:` URI. No mail is sent and it isn't verified, but it must be
    /// well-formed (Apple rejects malformed contacts with `BadJwtToken`).
    /// Env override: `PULP_VAPID_SUBJECT`.
    pub subject: String,
}

impl Default for WebPushSection {
    fn default() -> Self {
        Self {
            subject: crate::notifier::webpush::DEFAULT_VAPID_SUBJECT.to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerSection {
    pub host: String,
    pub port: u16,
    pub https: HttpsSection,
}

impl Default for ServerSection {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 3000,
            https: HttpsSection::default(),
        }
    }
}

/// HTTPS listener settings (`server.https` in config.json).
///
/// `mode`:
/// - `"auto"` (default) — serve HTTPS when certificates are resolvable:
///   explicit `cert_path`/`key_path`, else certs provisioned via the local
///   `tailscale` CLI for this machine's tailnet name. When neither works the
///   server stays HTTP-only and logs how to set HTTPS up.
/// - `"on"` — like auto, but failing to resolve certs is an error log (the
///   server still starts HTTP-only rather than dying).
/// - `"off"` — never serve HTTPS.
///
/// Env overrides: `PULP_HTTPS_MODE`, `PULP_HTTPS_PORT`,
/// `PULP_TLS_CERT`, `PULP_TLS_KEY`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HttpsSection {
    pub mode: String,
    pub port: u16,
    pub cert_path: Option<String>,
    pub key_path: Option<String>,
}

impl Default for HttpsSection {
    fn default() -> Self {
        Self {
            mode: "auto".to_string(),
            port: 3443,
            cert_path: None,
            key_path: None,
        }
    }
}

/// AI relevance-filter settings (`ai_filter` in config.json).
///
/// Pulp does not bundle a model: point it at any LLM endpoint that speaks
/// the OpenAI Chat Completions API (`POST {base_url}/chat/completions`) — a
/// local Ollama/LM Studio/llama-server, or a hosted provider (OpenAI,
/// OpenRouter, …). The filter is always optional: it activates only when
/// `enabled` is true and both `base_url` and `model` are non-empty.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AiFilterSection {
    pub enabled: bool,
    /// OpenAI-compatible base URL, e.g. `http://localhost:11434/v1` (Ollama)
    /// or `https://api.openai.com/v1`. The `/chat/completions` path is appended.
    pub base_url: String,
    /// Model name to request from the endpoint (e.g. `llama3.2`, `gpt-4o-mini`).
    pub model: String,
    /// Bearer token for hosted endpoints. `null` for local servers that don't
    /// require auth.
    pub api_key: Option<String>,
}

impl Default for AiFilterSection {
    fn default() -> Self {
        Self {
            enabled: false,
            base_url: DEFAULT_LLM_BASE_URL.to_string(),
            model: String::new(),
            api_key: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Resolved config
// ---------------------------------------------------------------------------

/// Where the effective config came from (for the startup log line).
#[derive(Debug, Clone, PartialEq)]
pub enum ConfigSource {
    /// Built-in defaults only (no file involved; used by tests).
    Defaults,
    /// No config.json existed; defaults were written to the given path.
    CreatedDefault(PathBuf),
    /// config.json was loaded from the given path.
    LoadedFile(PathBuf),
}

impl fmt::Display for ConfigSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Defaults => write!(f, "built-in defaults"),
            Self::CreatedDefault(p) => write!(f, "created default {}", p.display()),
            Self::LoadedFile(p) => write!(f, "loaded {}", p.display()),
        }
    }
}

/// AI-filter settings after merging defaults < config.json < env.
#[derive(Debug, Clone)]
pub struct AiFilterSettings {
    pub enabled: bool,
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
}

impl Default for AiFilterSettings {
    fn default() -> Self {
        let s = AiFilterSection::default();
        Self {
            enabled: s.enabled,
            base_url: s.base_url,
            model: s.model,
            api_key: s.api_key,
        }
    }
}

#[derive(Clone)]
pub struct Config {
    /// The resolved app home directory (`PULP_HOME` or `~/.pulp`).
    pub home: PathBuf,
    pub database_url: String,
    pub bind: String,
    /// Merged `ai_filter` section (config.json + env): the optional LLM judge.
    pub ai_filter: AiFilterSettings,
    /// Merged `server.https` section (defaults < config.json < env).
    pub https: HttpsSection,
    /// VAPID `sub` contact for Web Push (defaults < config.json < env).
    pub vapid_subject: String,
    pub config_source: ConfigSource,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            home: PathBuf::new(),
            database_url: "sqlite::memory:".to_string(),
            bind: "127.0.0.1:0".to_string(),
            ai_filter: AiFilterSettings::default(),
            // Tests/defaults stay HTTP-only; `auto` is applied when loading
            // from config.json (i.e. real `serve` runs).
            https: HttpsSection {
                mode: "off".to_string(),
                ..HttpsSection::default()
            },
            vapid_subject: crate::notifier::webpush::DEFAULT_VAPID_SUBJECT.to_string(),
            config_source: ConfigSource::Defaults,
        }
    }
}

impl Config {
    /// Full startup load: resolve the app home, create its directory tree,
    /// load `<home>/config.json` (writing one with explicit defaults on first
    /// run), then apply env-var overrides on top.
    pub fn load() -> anyhow::Result<Self> {
        let home = resolve_home()?;
        ensure_home_dirs(&home)?;

        let config_path = home.join("config.json");
        let (file, source) = if config_path.exists() {
            let text = std::fs::read_to_string(&config_path)
                .with_context(|| format!("failed to read {}", config_path.display()))?;
            let file: FileConfig = serde_json::from_str(&text)
                .with_context(|| format!("invalid JSON in {}", config_path.display()))?;
            (file, ConfigSource::LoadedFile(config_path))
        } else {
            let file = FileConfig::default();
            let mut json = serde_json::to_string_pretty(&file)?;
            json.push('\n');
            std::fs::write(&config_path, json)
                .with_context(|| format!("failed to write {}", config_path.display()))?;
            (file, ConfigSource::CreatedDefault(config_path))
        };

        Ok(Self::merge(home, file, source))
    }

    /// Merge precedence: built-in defaults < `file` (config.json) < env vars.
    fn merge(home: PathBuf, file: FileConfig, config_source: ConfigSource) -> Self {
        let database_url = std::env::var("DATABASE_URL")
            .ok()
            .or_else(|| {
                file.database_path
                    .as_deref()
                    .map(|p| sqlite_url(Path::new(p)))
            })
            .unwrap_or_else(|| sqlite_url(&home.join("pulp.db")));

        let bind = std::env::var("BIND")
            .unwrap_or_else(|_| format!("{}:{}", file.server.host, file.server.port));

        let nonempty = |v: Option<String>| v.filter(|s| !s.trim().is_empty());
        let ai_filter = AiFilterSettings {
            enabled: file.ai_filter.enabled,
            base_url: nonempty(std::env::var("PULP_LLM_BASE_URL").ok())
                .unwrap_or(file.ai_filter.base_url),
            model: nonempty(std::env::var("PULP_LLM_MODEL").ok()).unwrap_or(file.ai_filter.model),
            api_key: nonempty(std::env::var("PULP_LLM_API_KEY").ok()).or(file.ai_filter.api_key),
        };

        let https = HttpsSection {
            mode: std::env::var("PULP_HTTPS_MODE")
                .ok()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or(file.server.https.mode),
            port: std::env::var("PULP_HTTPS_PORT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(file.server.https.port),
            cert_path: std::env::var("PULP_TLS_CERT")
                .ok()
                .or(file.server.https.cert_path),
            key_path: std::env::var("PULP_TLS_KEY")
                .ok()
                .or(file.server.https.key_path),
        };

        let vapid_subject = std::env::var("PULP_VAPID_SUBJECT")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or(file.webpush.subject);

        Self {
            home,
            database_url,
            bind,
            ai_filter,
            https,
            vapid_subject,
            config_source,
        }
    }

    /// Persist an updated `ai_filter` section to `<home>/config.json`, leaving
    /// every other section as it is on disk. Used by the settings API/CLI so a
    /// running server's config survives restarts. Re-reads the file first so we
    /// never clobber concurrent edits to unrelated sections.
    pub fn save_ai_filter(&self, ai_filter: &AiFilterSection) -> anyhow::Result<()> {
        let config_path = self.home.join("config.json");
        let mut file: FileConfig = if config_path.exists() {
            let text = std::fs::read_to_string(&config_path)
                .with_context(|| format!("failed to read {}", config_path.display()))?;
            serde_json::from_str(&text)
                .with_context(|| format!("invalid JSON in {}", config_path.display()))?
        } else {
            FileConfig::default()
        };
        file.ai_filter = ai_filter.clone();
        let mut json = serde_json::to_string_pretty(&file)?;
        json.push('\n');
        std::fs::write(&config_path, json)
            .with_context(|| format!("failed to write {}", config_path.display()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    /// Env vars that influence `Config::load()`. Each test clears them and the
    /// guard restores prior values on drop (including on panic).
    const ENV_VARS: &[&str] = &[
        "PULP_HOME",
        "DATABASE_URL",
        "BIND",
        "PULP_LLM_BASE_URL",
        "PULP_LLM_MODEL",
        "PULP_LLM_API_KEY",
        "PULP_HTTPS_MODE",
        "PULP_HTTPS_PORT",
        "PULP_TLS_CERT",
        "PULP_TLS_KEY",
        "PULP_VAPID_SUBJECT",
    ];

    struct EnvGuard {
        saved: Vec<(&'static str, Option<String>)>,
    }

    impl EnvGuard {
        fn clear_all() -> Self {
            let saved = ENV_VARS
                .iter()
                .map(|&k| (k, std::env::var(k).ok()))
                .collect();
            for k in ENV_VARS {
                std::env::remove_var(k);
            }
            Self { saved }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (k, v) in &self.saved {
                match v {
                    Some(v) => std::env::set_var(k, v),
                    None => std::env::remove_var(k),
                }
            }
        }
    }

    fn temp_home() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pulp-config-test-{}", ulid::Ulid::new()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn fwd(p: &Path) -> String {
        p.display().to_string().replace('\\', "/")
    }

    #[test]
    #[serial]
    fn first_run_creates_home_tree_and_default_config() {
        let _guard = EnvGuard::clear_all();
        let home = temp_home();
        std::env::set_var("PULP_HOME", &home);

        let config = Config::load().unwrap();

        // Home directory created.
        assert!(home.is_dir());

        // config.json written with all defaults explicit.
        let config_path = home.join("config.json");
        assert!(config_path.is_file());
        let written: FileConfig =
            serde_json::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
        assert_eq!(written, FileConfig::default());

        // Built-in defaults applied.
        assert_eq!(config.home, home);
        assert_eq!(
            config.database_url,
            format!("sqlite:{}/pulp.db", fwd(&home))
        );
        assert_eq!(config.bind, "127.0.0.1:3000");
        assert!(!config.ai_filter.enabled);
        assert_eq!(config.ai_filter.base_url, DEFAULT_LLM_BASE_URL);
        assert_eq!(config.ai_filter.model, "");
        assert_eq!(config.ai_filter.api_key, None);
        assert!(matches!(
            config.config_source,
            ConfigSource::CreatedDefault(_)
        ));

        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    #[serial]
    fn config_json_overrides_defaults() {
        let _guard = EnvGuard::clear_all();
        let home = temp_home();
        std::env::set_var("PULP_HOME", &home);

        let db_path = home.join("custom").join("my.db");
        let file_json = serde_json::json!({
            "server": { "host": "0.0.0.0", "port": 8080 },
            "database_path": db_path.to_str().unwrap(),
            "ai_filter": {
                "enabled": true,
                "model": "llama3.2",
                "base_url": "http://ollama.lan:11434/v1",
                "api_key": "sk-test"
            }
        });
        std::fs::write(home.join("config.json"), file_json.to_string()).unwrap();

        let config = Config::load().unwrap();

        assert_eq!(config.bind, "0.0.0.0:8080");
        assert_eq!(config.database_url, format!("sqlite:{}", fwd(&db_path)));
        assert!(config.ai_filter.enabled);
        assert_eq!(config.ai_filter.model, "llama3.2");
        assert_eq!(config.ai_filter.base_url, "http://ollama.lan:11434/v1");
        assert_eq!(config.ai_filter.api_key.as_deref(), Some("sk-test"));
        assert!(matches!(config.config_source, ConfigSource::LoadedFile(_)));

        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    #[serial]
    fn env_overrides_config_json() {
        let _guard = EnvGuard::clear_all();
        let home = temp_home();
        std::env::set_var("PULP_HOME", &home);

        let file_json = serde_json::json!({
            "server": { "host": "0.0.0.0", "port": 8080 },
            "database_path": "C:/somewhere/else.db",
            "ai_filter": {
                "enabled": true,
                "model": "from-file",
                "base_url": "http://from-file:11434/v1"
            }
        });
        std::fs::write(home.join("config.json"), file_json.to_string()).unwrap();

        std::env::set_var("DATABASE_URL", "sqlite:./env-override.db");
        std::env::set_var("BIND", "127.0.0.1:9999");
        std::env::set_var("PULP_LLM_BASE_URL", "http://from-env:11434/v1");
        std::env::set_var("PULP_LLM_MODEL", "from-env-model");
        std::env::set_var("PULP_LLM_API_KEY", "sk-from-env");

        let config = Config::load().unwrap();

        assert_eq!(config.database_url, "sqlite:./env-override.db");
        assert_eq!(config.bind, "127.0.0.1:9999");
        // Merged ai_filter view reflects env overrides.
        assert_eq!(config.ai_filter.model, "from-env-model");
        assert_eq!(config.ai_filter.base_url, "http://from-env:11434/v1");
        assert_eq!(config.ai_filter.api_key.as_deref(), Some("sk-from-env"));
        // `enabled` comes from config.json (no env equivalent).
        assert!(config.ai_filter.enabled);

        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    #[serial]
    fn partial_config_json_keeps_defaults_and_ignores_unknown_fields() {
        let _guard = EnvGuard::clear_all();
        let home = temp_home();
        std::env::set_var("PULP_HOME", &home);

        std::fs::write(
            home.join("config.json"),
            r#"{ "server": { "port": 8080 }, "some_future_field": true }"#,
        )
        .unwrap();

        let config = Config::load().unwrap();

        assert_eq!(config.bind, "127.0.0.1:8080"); // host kept its default
        assert!(!config.ai_filter.enabled);
        assert_eq!(config.ai_filter.model, "");
        assert_eq!(config.ai_filter.base_url, DEFAULT_LLM_BASE_URL);
        assert_eq!(
            config.database_url,
            format!("sqlite:{}/pulp.db", fwd(&home))
        );

        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    #[serial]
    fn home_resolution_prefers_pulp_home_env() {
        let _guard = EnvGuard::clear_all();

        let custom = temp_home();
        std::env::set_var("PULP_HOME", &custom);
        assert_eq!(resolve_home().unwrap(), custom);

        std::env::remove_var("PULP_HOME");
        let resolved = resolve_home().unwrap();
        assert!(resolved.ends_with(".pulp"));
        assert!(resolved.starts_with(home::home_dir().unwrap()));

        std::fs::remove_dir_all(&custom).ok();
    }
}
