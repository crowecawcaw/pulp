//! HTTPS certificate resolution for the built-in TLS listener.
//!
//! Two sources, tried in order:
//! 1. **Explicit paths** — `server.https.cert_path`/`key_path` in config.json
//!    (or `PULP_TLS_CERT`/`PULP_TLS_KEY`).
//! 2. **Tailscale** — when the `tailscale` CLI is present and the tailnet has
//!    HTTPS certificates enabled, `tailscale cert` provisions a Let's Encrypt
//!    cert for this machine's tailnet name into `<home>/certs/`. This is what
//!    makes `pulp serve` HTTPS-by-default on a Tailscale box.
//!
//! Everything here is best-effort: any failure resolves to `None` and the
//! server stays HTTP-only with a hint logged (see `server::spawn_https`).

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::Config;

/// Re-run `tailscale cert` when the cert file is older than this. The command
/// is cheap when the cert is still valid and renews it when close to expiry
/// (Let's Encrypt certs last 90 days), so a generous refresh interval works.
const CERT_REFRESH_SECS: u64 = 24 * 60 * 60;

#[derive(Debug, Clone)]
pub struct ResolvedTls {
    pub cert: PathBuf,
    pub key: PathBuf,
    /// The hostname the cert is for (used in the startup URL log). `None` for
    /// manually supplied certs, where we can't cheaply know the SAN.
    pub host: Option<String>,
    /// This machine's Tailscale IP. The cert's name resolves to this address,
    /// so when `server.host` is loopback the HTTPS listener binds here instead
    /// — reachable from the tailnet without exposing the (unauthenticated)
    /// API to the whole LAN the way 0.0.0.0 would.
    pub bind_ip: Option<std::net::IpAddr>,
    /// Tailscale-provisioned certs are periodically refreshed; manual ones
    /// are reloaded only on restart.
    pub tailscale: bool,
}

/// The URL to hand to another device (e.g. a phone installing the PWA over
/// Tailscale). Prefers the Tailscale HTTPS URL (`https://<tailnet-name>:<port>`,
/// reachable across the tailnet); falls back to the local loopback HTTP URL
/// (`http://127.0.0.1:<port>`) when no Tailscale host is resolvable.
///
/// NOTE: [`resolve`] shells out to the `tailscale` CLI and, as a side effect,
/// may re-provision certs. Do NOT call this on a GUI event-loop thread —
/// compute it on a worker/server thread (or once at startup before the loop
/// runs) and cache the result. The desktop-tray launcher does the latter, so
/// the cert side effect happens once at startup rather than per menu open; the
/// server thread's own `tls::resolve` (for its HTTPS listener) is the same
/// idempotent call, so the only cost is one extra `tailscale cert` invocation
/// at launch, which is cheap while the cert is still valid.
pub fn install_url(config: &Config) -> String {
    match resolve(config).and_then(|t| t.host) {
        Some(host) => format!("https://{}:{}", host, config.https.port),
        None => format!("http://{}", crate::cli::serve::connect_addr(&config.bind)),
    }
}

/// Resolve cert/key per the config. Returns `None` (HTTP-only) when mode is
/// `off` or nothing is resolvable.
pub fn resolve(config: &Config) -> Option<ResolvedTls> {
    if config.https.mode == "off" {
        return None;
    }

    if let (Some(cert), Some(key)) = (&config.https.cert_path, &config.https.key_path) {
        let (cert, key) = (PathBuf::from(cert), PathBuf::from(key));
        if cert.is_file() && key.is_file() {
            return Some(ResolvedTls {
                cert,
                key,
                host: None,
                bind_ip: None,
                tailscale: false,
            });
        }
        tracing::error!(
            "https cert_path/key_path configured but not readable ({} / {}); \
             falling back to Tailscale resolution",
            cert.display(),
            key.display()
        );
    }

    resolve_tailscale(&config.home)
}

/// Provision (or reuse) a Tailscale cert for this machine into `<home>/certs/`.
pub fn resolve_tailscale(home: &Path) -> Option<ResolvedTls> {
    let ts = tailscale_bin()?;
    let (host, bind_ip) = tailscale_self(&ts)?;

    let dir = home.join("certs");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::warn!("cannot create {}: {}", dir.display(), e);
        return None;
    }
    let cert = dir.join(format!("{host}.crt"));
    let key = dir.join(format!("{host}.key"));

    if needs_refresh(&cert) {
        tracing::info!(
            "provisioning HTTPS certificate for {} via tailscale cert",
            host
        );
        let out = Command::new(&ts)
            .args([
                "cert",
                "--cert-file",
                &cert.display().to_string(),
                "--key-file",
                &key.display().to_string(),
                &host,
            ])
            .output();
        match out {
            Ok(o) if o.status.success() => {}
            Ok(o) => {
                tracing::warn!(
                    "`tailscale cert {}` failed: {}",
                    host,
                    String::from_utf8_lossy(&o.stderr).trim()
                );
                // A previously provisioned (possibly stale-but-valid) pair is
                // still better than no HTTPS.
                if !(cert.is_file() && key.is_file()) {
                    return None;
                }
            }
            Err(e) => {
                tracing::warn!("could not run tailscale cert: {}", e);
                return None;
            }
        }
    }

    if cert.is_file() && key.is_file() {
        Some(ResolvedTls {
            cert,
            key,
            host: Some(host),
            bind_ip,
            tailscale: true,
        })
    } else {
        None
    }
}

/// True when the cert is missing or due for its periodic `tailscale cert`
/// refresh (which renews near expiry and is cheap otherwise).
fn needs_refresh(cert: &Path) -> bool {
    match std::fs::metadata(cert).and_then(|m| m.modified()) {
        Ok(modified) => match modified.elapsed() {
            Ok(age) => age.as_secs() > CERT_REFRESH_SECS,
            Err(_) => false, // clock skew: mtime in the future, treat as fresh
        },
        Err(_) => true, // missing
    }
}

/// Locate the tailscale CLI: PATH first, then the standard install locations.
fn tailscale_bin() -> Option<PathBuf> {
    let candidates: &[&str] = if cfg!(windows) {
        &[
            "tailscale",
            r"C:\Program Files\Tailscale\tailscale.exe",
            r"C:\Program Files (x86)\Tailscale\tailscale.exe",
        ]
    } else {
        &[
            "tailscale",
            "/usr/bin/tailscale",
            "/usr/local/bin/tailscale",
        ]
    };
    for c in candidates {
        if Command::new(c)
            .arg("version")
            .output()
            .is_ok_and(|o| o.status.success())
        {
            return Some(PathBuf::from(c));
        }
    }
    None
}

/// This machine's cert-eligible tailnet name and Tailscale IPv4, from
/// `tailscale status --json`. `CertDomains` is populated only when the
/// tailnet has HTTPS certificates enabled — the admin-console toggle the
/// user must flip once.
fn tailscale_self(ts: &Path) -> Option<(String, Option<std::net::IpAddr>)> {
    let out = Command::new(ts).args(["status", "--json"]).output().ok()?;
    if !out.status.success() {
        tracing::debug!("tailscale status failed; not connected?");
        return None;
    }
    let status: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    let domain = status
        .get("CertDomains")
        .and_then(|d| d.as_array())
        .and_then(|a| a.first())
        .and_then(|v| v.as_str())
        .map(|s| s.trim_end_matches('.').to_string());
    let Some(domain) = domain else {
        tracing::info!(
            "Tailscale is installed but reports no cert domains — enable HTTPS \
             certificates in the Tailscale admin console (DNS page) for automatic HTTPS"
        );
        return None;
    };
    // The name resolves to the Tailscale IP, so that's where the HTTPS
    // listener must bind. Prefer IPv4.
    let ips: Vec<std::net::IpAddr> = status
        .get("Self")
        .and_then(|s| s.get("TailscaleIPs"))
        .and_then(|a| a.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str())
                .filter_map(|s| s.parse().ok())
                .collect()
        })
        .unwrap_or_default();
    let bind_ip = ips
        .iter()
        .find(|ip| ip.is_ipv4())
        .or_else(|| ips.first())
        .copied();
    Some((domain, bind_ip))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::HttpsSection;

    fn config_with(https: HttpsSection) -> Config {
        Config {
            https,
            ..Config::default()
        }
    }

    #[test]
    fn off_mode_never_resolves() {
        let cfg = config_with(HttpsSection {
            mode: "off".into(),
            ..HttpsSection::default()
        });
        assert!(resolve(&cfg).is_none());
    }

    #[test]
    fn explicit_paths_win_when_readable() {
        let dir = std::env::temp_dir().join(format!("pulp-tls-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cert = dir.join("c.pem");
        let key = dir.join("k.pem");
        std::fs::write(&cert, "x").unwrap();
        std::fs::write(&key, "x").unwrap();

        let cfg = config_with(HttpsSection {
            mode: "auto".into(),
            cert_path: Some(cert.display().to_string()),
            key_path: Some(key.display().to_string()),
            ..HttpsSection::default()
        });
        let resolved = resolve(&cfg).expect("explicit cert paths resolve");
        assert!(!resolved.tailscale);
        assert_eq!(resolved.cert, cert);
        assert_eq!(resolved.key, key);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn install_url_falls_back_to_loopback_without_tailscale() {
        // HTTPS off => `resolve` returns None without shelling out to
        // tailscale, so `install_url` must yield the local loopback URL, with
        // the wildcard bind host rewritten to 127.0.0.1.
        let cfg = Config {
            bind: "0.0.0.0:3000".into(),
            https: HttpsSection {
                mode: "off".into(),
                ..HttpsSection::default()
            },
            ..Config::default()
        };
        assert_eq!(install_url(&cfg), "http://127.0.0.1:3000");
    }

    #[test]
    fn missing_cert_needs_refresh_and_fresh_does_not() {
        let dir = std::env::temp_dir().join(format!("pulp-tls-fresh-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cert = dir.join("c.crt");
        assert!(needs_refresh(&cert), "missing cert must refresh");
        std::fs::write(&cert, "x").unwrap();
        assert!(!needs_refresh(&cert), "just-written cert is fresh");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
