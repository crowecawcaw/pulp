//! Minimal typed HTTP client for the Pulp API.
//!
//! Every call serializes/deserializes the same structs the server handlers
//! use, so a contract change that breaks the CLI fails to compile instead of
//! failing at runtime.

use anyhow::{bail, Context};
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::db::repos::traits::Workspace;

pub struct ApiClient {
    base: String,
    http: reqwest::Client,
}

/// Default server base: `server.host:port` from `~/.pulp/config.json`
/// when present (the same file `pulp serve` binds from), else the
/// built-in default.
fn default_base() -> String {
    let section = crate::config::load_file_config().server;
    format!("http://{}:{}", section.host, section.port)
}

impl ApiClient {
    pub fn new(server: Option<&str>) -> Self {
        let mut base = server.map(str::to_string).unwrap_or_else(default_base);
        if !base.starts_with("http://") && !base.starts_with("https://") {
            base = format!("http://{}", base);
        }
        ApiClient {
            base: base.trim_end_matches('/').to_string(),
            http: reqwest::Client::new(),
        }
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> anyhow::Result<T> {
        self.decode(self.send(self.http.get(self.url(path))).await?)
            .await
    }

    pub async fn post<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> anyhow::Result<T> {
        self.decode(self.send(self.http.post(self.url(path)).json(body)).await?)
            .await
    }

    pub async fn put<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> anyhow::Result<T> {
        self.decode(self.send(self.http.put(self.url(path)).json(body)).await?)
            .await
    }

    /// POST with no request/response body (admin triggers).
    pub async fn post_no_response(&self, path: &str) -> anyhow::Result<()> {
        self.send(self.http.post(self.url(path))).await.map(|_| ())
    }

    /// POST with no request body but a JSON response (e.g. the notifications
    /// test endpoint, whose only input is the query string).
    pub async fn post_query<T: DeserializeOwned>(&self, path: &str) -> anyhow::Result<T> {
        self.decode(self.send(self.http.post(self.url(path))).await?)
            .await
    }

    /// POST a body and return only the status code (e.g. backfill's 200/202).
    pub async fn post_status<B: Serialize>(&self, path: &str, body: &B) -> anyhow::Result<u16> {
        let resp = self.send(self.http.post(self.url(path)).json(body)).await?;
        Ok(resp.status().as_u16())
    }

    pub async fn delete(&self, path: &str) -> anyhow::Result<()> {
        self.send(self.http.delete(self.url(path)))
            .await
            .map(|_| ())
    }

    /// DELETE with a JSON body (e.g. push unsubscribe, whose identity is the
    /// endpoint URL and so travels in the body rather than the path).
    pub async fn delete_body<B: Serialize>(&self, path: &str, body: &B) -> anyhow::Result<()> {
        self.send(self.http.delete(self.url(path)).json(body))
            .await
            .map(|_| ())
    }

    /// Resolve a workspace id: pass-through when given; otherwise use the
    /// sole existing workspace, or fail with a list to pick from.
    pub async fn resolve_workspace(&self, explicit: Option<String>) -> anyhow::Result<String> {
        if let Some(id) = explicit {
            return Ok(id);
        }
        let workspaces: Vec<Workspace> = self.get("/api/workspaces").await?;
        match workspaces.len() {
            0 => bail!("no workspaces exist yet — create one with `pulp workspaces create <name>`"),
            1 => Ok(workspaces[0].id.clone()),
            _ => {
                let listing = workspaces
                    .iter()
                    .map(|w| format!("  {}  {}", w.id, w.name))
                    .collect::<Vec<_>>()
                    .join("\n");
                bail!(
                    "multiple workspaces exist; pass --workspace <id>:\n{}",
                    listing
                )
            }
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    /// Send and fail on connection errors / non-2xx statuses with messages an
    /// agent can act on.
    async fn send(&self, rb: reqwest::RequestBuilder) -> anyhow::Result<reqwest::Response> {
        let resp = rb.send().await.with_context(|| {
            format!(
                "could not reach the Pulp server at {} — is it running? \
                 Start it with `pulp serve`, or point --server / PULP_SERVER at it",
                self.base
            )
        })?;
        let status = resp.status();
        if status.is_success() {
            return Ok(resp);
        }
        let body = resp.text().await.unwrap_or_default();
        // The API wraps errors as {"error": "..."}; fall back to the raw body.
        let msg = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(String::from))
            .unwrap_or(body);
        bail!("server returned {}: {}", status.as_u16(), msg.trim())
    }

    async fn decode<T: DeserializeOwned>(&self, resp: reqwest::Response) -> anyhow::Result<T> {
        let body = resp.text().await.context("reading response body")?;
        serde_json::from_str(&body).with_context(|| {
            format!(
                "unexpected response shape from the server (CLI/server version mismatch?): {}",
                crate::cli::util::snippet(&body, 200)
            )
        })
    }
}

/// Incremental query-string builder for the list endpoints.
#[derive(Default)]
pub struct Qs(Vec<(String, String)>);

impl Qs {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, key: &str, value: impl ToString) -> &mut Self {
        self.0.push((key.to_string(), value.to_string()));
        self
    }

    pub fn push_opt(&mut self, key: &str, value: Option<impl ToString>) -> &mut Self {
        if let Some(v) = value {
            self.push(key, v);
        }
        self
    }

    pub fn build(&self) -> String {
        if self.0.is_empty() {
            return String::new();
        }
        let pairs = self
            .0
            .iter()
            .map(|(k, v)| format!("{}={}", k, crate::collectors::percent_encode(v)))
            .collect::<Vec<_>>()
            .join("&");
        format!("?{}", pairs)
    }
}
