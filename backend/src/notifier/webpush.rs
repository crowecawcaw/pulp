//! Web Push delivery (browser/PWA push notifications).
//!
//! Implemented directly against the relevant RFCs using pure-Rust crypto
//! (RustCrypto + ring is already in the tree via rustls) so it adds **no native
//! build dependencies** — the obvious `web-push` crate is avoided because it
//! pulls OpenSSL in through `ece`, which would contradict this project's
//! deliberate rustls/ring, single-binary stance.
//!
//! - Message encryption: RFC 8291 (`aes128gcm`, the modern single-record scheme
//!   from RFC 8188).
//! - Request authorization: RFC 8292 (VAPID — an ES256 JWT plus the server's
//!   public key), so push services accept messages from this server.
//!
//! Web Push subscriptions are now stored per-workspace as `webpush`
//! notifications (`config` = `{endpoint, p256dh, auth}`). A feed-visible mention
//! is delivered to each `webpush` notification in its workspace; a subscription
//! the push service reports as gone (HTTP 404/410) is pruned by removing the
//! notification.

use std::path::Path;
use std::sync::Arc;

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes128Gcm, Key, Nonce};
use anyhow::{anyhow, Context};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use hkdf::Hkdf;
use p256::ecdsa::signature::Signer;
use p256::ecdsa::{Signature, SigningKey};
use p256::elliptic_curve::sec1::ToEncodedPoint;
use p256::{ecdh, PublicKey, SecretKey};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::db::repos::traits::{Mention, Notification};
use crate::state::AppState;

/// Default `mailto:`/`https:` contact embedded in the VAPID JWT `sub` claim
/// when none is configured. Push services use it to reach the operator about
/// misbehaving pushes; no mail is sent and the value is not verified.
/// The default is a placeholder; operators should override it with their own
/// contact (e.g., `mailto:operator@example.com`) via `webpush.subject` in
/// config.json or the `PULP_VAPID_SUBJECT` env var.
pub const DEFAULT_VAPID_SUBJECT: &str = "mailto:pulp@localhost";

/// How long a delivered notification is retained by the push service if the
/// device is offline (seconds). Four weeks — the common maximum.
const TTL_SECONDS: u32 = 2_419_200;

/// Record size advertised in the aes128gcm header. We always send a single
/// record, so this only needs to exceed the (small) payload; 4096 matches the
/// RFC 8291 worked example, which our unit test pins against.
const RECORD_SIZE: u32 = 4096;

// ── VAPID keypair ──────────────────────────────────────────────────────────

/// Persisted shape of `<home>/vapid.json`. Both keys are URL-safe base64
/// without padding: `private_key` is the 32-byte P-256 scalar, `public_key` is
/// the 65-byte uncompressed point (what the browser needs as
/// `applicationServerKey`).
#[derive(Serialize, Deserialize)]
struct VapidFile {
    private_key: String,
    public_key: String,
}

/// The server's VAPID identity. Generated once and persisted; the same public
/// key is handed to the browser at subscribe time and sent in the `Authorization`
/// header of every push so the push service can tie the two together.
pub struct VapidKeys {
    signing_key: SigningKey,
    /// 65-byte uncompressed public point, base64url — served to the frontend
    /// and used verbatim in the `Authorization: vapid k=...` header.
    pub public_b64: String,
    /// VAPID `sub` claim (operator contact) embedded in every push JWT.
    subject: String,
}

impl VapidKeys {
    /// Generate a fresh P-256 VAPID keypair with the default subject.
    pub fn generate() -> Self {
        let secret = SecretKey::random(&mut rand_core::OsRng);
        Self::from_secret(secret, DEFAULT_VAPID_SUBJECT.to_string())
    }

    fn from_secret(secret: SecretKey, subject: String) -> Self {
        let public_b64 =
            URL_SAFE_NO_PAD.encode(secret.public_key().to_encoded_point(false).as_bytes());
        let signing_key = SigningKey::from(&secret);
        Self {
            signing_key,
            public_b64,
            subject,
        }
    }

    /// Load `<home>/vapid.json`, or generate a keypair and write it on first
    /// run. A malformed file is treated as fatal rather than silently rotating
    /// the key (which would invalidate every existing subscription). `subject`
    /// is the configured VAPID `sub` contact (it lives in config, not the key
    /// file, so changing it takes effect without rotating the keypair).
    pub fn load_or_create(home: &Path, subject: &str) -> anyhow::Result<Self> {
        let path = home.join("vapid.json");
        if path.exists() {
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("failed to read {}", path.display()))?;
            let file: VapidFile = serde_json::from_str(&text)
                .with_context(|| format!("invalid VAPID key file {}", path.display()))?;
            let bytes = URL_SAFE_NO_PAD
                .decode(file.private_key.trim())
                .context("VAPID private_key is not valid base64url")?;
            let secret = SecretKey::from_slice(&bytes)
                .context("VAPID private_key is not a valid P-256 key")?;
            return Ok(Self::from_secret(secret, subject.to_string()));
        }

        let secret = SecretKey::random(&mut rand_core::OsRng);
        let keys = Self::from_secret(secret.clone(), subject.to_string());
        let file = VapidFile {
            private_key: URL_SAFE_NO_PAD.encode(secret.to_bytes()),
            public_key: keys.public_b64.clone(),
        };
        let mut json = serde_json::to_string_pretty(&file)?;
        json.push('\n');
        std::fs::write(&path, json)
            .with_context(|| format!("failed to write {}", path.display()))?;
        tracing::info!("Generated VAPID keypair at {}", path.display());
        Ok(keys)
    }

    /// Build the `Authorization` header value for a push to `endpoint`:
    /// `vapid t=<ES256 JWT>, k=<server public key>`.
    fn authorization_header(&self, endpoint: &str, now: i64) -> anyhow::Result<String> {
        let aud = reqwest::Url::parse(endpoint)
            .context("invalid push endpoint URL")?
            .origin()
            .ascii_serialization();

        let header = URL_SAFE_NO_PAD.encode(br#"{"typ":"JWT","alg":"ES256"}"#);
        let claims = serde_json::json!({
            "aud": aud,
            "exp": now + 12 * 60 * 60,
            "sub": self.subject.as_str(),
        });
        let claims_b64 = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims)?);
        let signing_input = format!("{header}.{claims_b64}");

        let signature: Signature = self
            .signing_key
            .try_sign(signing_input.as_bytes())
            .context("VAPID JWT signing failed")?;
        let token = format!(
            "{signing_input}.{}",
            URL_SAFE_NO_PAD.encode(signature.to_bytes())
        );

        Ok(format!("vapid t={token}, k={}", self.public_b64))
    }
}

// ── RFC 8291 message encryption ──────────────────────────────────────────────

/// Encrypt `plaintext` for a subscription using a freshly generated ephemeral
/// key and salt. Returns the complete `aes128gcm` body to POST.
fn encrypt_payload(
    ua_public: &PublicKey,
    auth_secret: &[u8],
    plaintext: &[u8],
) -> anyhow::Result<Vec<u8>> {
    let as_secret = SecretKey::random(&mut rand_core::OsRng);
    let mut salt = [0u8; 16];
    rand_core::RngCore::fill_bytes(&mut rand_core::OsRng, &mut salt);
    encrypt_with(
        &as_secret,
        &salt,
        RECORD_SIZE,
        ua_public,
        auth_secret,
        plaintext,
    )
}

/// Core RFC 8291 + RFC 8188 encryption with the ephemeral key, salt, and record
/// size supplied explicitly — split out so the RFC 8291 Appendix A test vector
/// can be reproduced exactly.
fn encrypt_with(
    as_secret: &SecretKey,
    salt: &[u8; 16],
    record_size: u32,
    ua_public: &PublicKey,
    auth_secret: &[u8],
    plaintext: &[u8],
) -> anyhow::Result<Vec<u8>> {
    let as_public_bytes = as_secret.public_key().to_encoded_point(false);
    let as_public_bytes = as_public_bytes.as_bytes();
    let ua_public_bytes = ua_public.to_encoded_point(false);
    let ua_public_bytes = ua_public_bytes.as_bytes();

    // ECDH(as_private, ua_public) → 32-byte shared secret.
    let shared = ecdh::diffie_hellman(as_secret.to_nonzero_scalar(), ua_public.as_affine());
    let ecdh_secret = shared.raw_secret_bytes();

    // RFC 8291 §3.4: derive the input keying material, keyed by the auth secret.
    let mut key_info = Vec::with_capacity(14 + 65 + 65);
    key_info.extend_from_slice(b"WebPush: info\0");
    key_info.extend_from_slice(ua_public_bytes);
    key_info.extend_from_slice(as_public_bytes);
    let mut ikm = [0u8; 32];
    Hkdf::<Sha256>::new(Some(auth_secret), ecdh_secret)
        .expand(&key_info, &mut ikm)
        .map_err(|_| anyhow!("HKDF expand (IKM) failed"))?;

    // RFC 8188 §2.2: content-encryption key and nonce, keyed by the record salt.
    let hk = Hkdf::<Sha256>::new(Some(salt), &ikm);
    let mut cek = [0u8; 16];
    hk.expand(b"Content-Encoding: aes128gcm\0", &mut cek)
        .map_err(|_| anyhow!("HKDF expand (CEK) failed"))?;
    let mut nonce = [0u8; 12];
    hk.expand(b"Content-Encoding: nonce\0", &mut nonce)
        .map_err(|_| anyhow!("HKDF expand (nonce) failed"))?;

    // Single record: append the 0x02 final-record delimiter (RFC 8188 §2.1),
    // then AES-128-GCM seal (ciphertext has the 16-byte tag appended).
    let mut record = plaintext.to_vec();
    record.push(0x02);
    let cipher = Aes128Gcm::new(Key::<Aes128Gcm>::from_slice(&cek));
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce), record.as_slice())
        .map_err(|_| anyhow!("AES-128-GCM encryption failed"))?;

    // aes128gcm header: salt(16) | rs(4, BE) | idlen(1) | keyid(=as_public,65).
    let mut body = Vec::with_capacity(16 + 4 + 1 + 65 + ciphertext.len());
    body.extend_from_slice(salt);
    body.extend_from_slice(&record_size.to_be_bytes());
    body.push(as_public_bytes.len() as u8);
    body.extend_from_slice(as_public_bytes);
    body.extend_from_slice(&ciphertext);
    Ok(body)
}

// ── Notification payload ─────────────────────────────────────────────────────

/// JSON the service worker receives in its `push` event (`title`/`body`/`url`).
/// `url` is the in-app mention **detail page** (`/mentions/{id}`), so clicking
/// the notification deep-links into Pulp rather than jumping straight to the
/// external post — the detail page itself links out to the original.
pub fn format_push_payload(mention: &Mention) -> serde_json::Value {
    let body: String = mention.content_text.chars().take(140).collect();
    serde_json::json!({
        "title": format!("New {} mention", mention.channel),
        "body": body,
        "url": format!("/mentions/{}", mention.id),
    })
}

/// Payload for a manual "is my setup working?" test push — the same shape the
/// service worker expects, with fixed copy. Triggered by the test button in the
/// UI so a user can confirm delivery without waiting for a real mention.
pub(crate) fn format_test_payload() -> serde_json::Value {
    serde_json::json!({
        "title": "Pulp test notification",
        "body": "Push notifications are working on this device. 🎉",
        "url": "/",
    })
}

// ── Delivery ────────────────────────────────────────────────────────────────

/// The base64url subscription keys parsed out of a `webpush` notification's
/// `config` (`{endpoint, p256dh, auth}`).
struct WebpushConfig {
    endpoint: String,
    p256dh: String,
    auth: String,
}

fn parse_config(notification: &Notification) -> anyhow::Result<WebpushConfig> {
    let get = |k: &str| {
        notification
            .config
            .get(k)
            .and_then(|v| v.as_str())
            .map(str::to_string)
    };
    Ok(WebpushConfig {
        endpoint: get("endpoint")
            .ok_or_else(|| anyhow!("webpush notification missing config.endpoint"))?,
        p256dh: get("p256dh")
            .ok_or_else(|| anyhow!("webpush notification missing config.p256dh"))?,
        auth: get("auth").ok_or_else(|| anyhow!("webpush notification missing config.auth"))?,
    })
}

/// Deliver a mention to a single `webpush` notification. On HTTP 404/410 Gone
/// the dead subscription is pruned by deleting the notification.
pub async fn deliver_to_notification(
    state: &Arc<AppState>,
    notification: &Notification,
    mention: &Mention,
) -> anyhow::Result<()> {
    let payload = serde_json::to_vec(&format_push_payload(mention))?;
    deliver_payload(state, notification, &payload).await
}

/// Send the test notification to a single `webpush` notification.
pub async fn deliver_test_to_notification(
    state: &Arc<AppState>,
    notification: &Notification,
) -> anyhow::Result<()> {
    let payload = serde_json::to_vec(&format_test_payload())?;
    deliver_payload(state, notification, &payload).await
}

/// Host portion of a push `endpoint` for logging (e.g. `fcm.googleapis.com`),
/// falling back to the raw string if it can't be parsed.
fn endpoint_host(endpoint: &str) -> String {
    reqwest::Url::parse(endpoint)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_else(|| endpoint.to_string())
}

/// POST an already-encoded payload to one notification's endpoint, pruning the
/// notification if the push service reports it Gone (404/410).
async fn deliver_payload(
    state: &Arc<AppState>,
    notification: &Notification,
    payload: &[u8],
) -> anyhow::Result<()> {
    let cfg = parse_config(notification)?;
    let host = endpoint_host(&cfg.endpoint);
    let now = chrono::Utc::now().timestamp();

    match send_one(state, &cfg, payload, now).await {
        Ok(SendOutcome::Delivered) => {
            tracing::debug!("webpush: delivered to {}", host);
            Ok(())
        }
        Ok(SendOutcome::Gone) => {
            match state.notifications.delete_by_endpoint(&cfg.endpoint).await {
                Ok(n) => tracing::info!("webpush: pruned {} dead subscription(s) at {}", n, host),
                Err(e) => tracing::warn!("webpush: failed to prune dead subscription: {:?}", e),
            }
            Ok(())
        }
        Err(e) => Err(e),
    }
}

enum SendOutcome {
    Delivered,
    /// The push service reports the subscription no longer exists (404/410).
    Gone,
}

async fn send_one(
    state: &Arc<AppState>,
    cfg: &WebpushConfig,
    payload: &[u8],
    now: i64,
) -> anyhow::Result<SendOutcome> {
    let p256dh = URL_SAFE_NO_PAD
        .decode(cfg.p256dh.trim_end_matches('='))
        .context("subscription p256dh is not valid base64url")?;
    let auth = URL_SAFE_NO_PAD
        .decode(cfg.auth.trim_end_matches('='))
        .context("subscription auth is not valid base64url")?;
    let ua_public =
        PublicKey::from_sec1_bytes(&p256dh).context("subscription p256dh is not a valid key")?;

    let body = encrypt_payload(&ua_public, &auth, payload)?;
    let authorization = state.vapid.authorization_header(&cfg.endpoint, now)?;

    let resp = state
        .http
        .post(&cfg.endpoint)
        .header("Authorization", authorization)
        .header("Content-Encoding", "aes128gcm")
        .header("Content-Type", "application/octet-stream")
        .header("TTL", TTL_SECONDS.to_string())
        .body(body)
        .send()
        .await
        .context("push request failed")?;

    let status = resp.status();
    if status.is_success() {
        Ok(SendOutcome::Delivered)
    } else if status == reqwest::StatusCode::NOT_FOUND || status == reqwest::StatusCode::GONE {
        Ok(SendOutcome::Gone)
    } else {
        let detail = resp.text().await.unwrap_or_default();
        Err(anyhow!("push service returned {}: {}", status, detail))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b64(s: &str) -> Vec<u8> {
        URL_SAFE_NO_PAD.decode(s).unwrap()
    }

    /// RFC 8291 Appendix A worked example: with the example's fixed application
    /// server key and salt, our encryption must reproduce the published body
    /// byte-for-byte. This pins ECDH, the HKDF derivations, the AES-128-GCM
    /// seal, and the aes128gcm framing all at once.
    #[test]
    fn rfc8291_appendix_a_vector() {
        let plaintext = b"When I grow up, I want to be a watermelon";
        let auth = b64("BTBZMqHH6r4Tts7J_aSIgg");
        let ua_public =
            PublicKey::from_sec1_bytes(&b64("BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4"))
                .unwrap();
        let as_secret =
            SecretKey::from_slice(&b64("yfWPiYE-n46HLnH0KqZOF1fJJU3MYrct3AELtAQ-oRw")).unwrap();
        let mut salt = [0u8; 16];
        salt.copy_from_slice(&b64("DGv6ra1nlYgDCS1FRnbzlw"));

        let body = encrypt_with(&as_secret, &salt, 4096, &ua_public, &auth, plaintext).unwrap();

        let expected = "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A_yl95bQpu6cVPTpK4Mqgkf1CXztLVBSt2Ks3oZwbuwXPXLWyouBWLVWGNWQexSgSxsj_Qulcy4a-fN";
        assert_eq!(URL_SAFE_NO_PAD.encode(&body), expected);
    }

    /// A round-trip via a randomly generated server key still produces the
    /// correct header framing (salt, record size, key id = server public key).
    #[test]
    fn random_encrypt_has_valid_framing() {
        let ua_secret = SecretKey::random(&mut rand_core::OsRng);
        let ua_public = ua_secret.public_key();
        let auth = [7u8; 16];

        let body = encrypt_payload(&ua_public, &auth, b"hello").unwrap();

        // header = salt(16) + rs(4) + idlen(1) + keyid(65) = 86 bytes, then the
        // ciphertext (>= plaintext + delimiter + 16-byte GCM tag).
        assert!(body.len() > 86);
        assert_eq!(
            u32::from_be_bytes(body[16..20].try_into().unwrap()),
            RECORD_SIZE
        );
        assert_eq!(body[20], 65); // idlen = length of the key id that follows
        assert_eq!(body[21], 0x04); // uncompressed EC point prefix on the key id
    }

    #[test]
    fn vapid_authorization_header_is_well_formed() {
        let keys = VapidKeys::generate();
        let header = keys
            .authorization_header("https://fcm.googleapis.com/fcm/send/abc123", 1_700_000_000)
            .unwrap();
        assert!(header.starts_with("vapid t="));
        assert!(header.contains(", k="));
        assert!(header.contains(&keys.public_b64));
        // t=<header>.<claims>.<sig> — three base64url segments.
        let token = header
            .trim_start_matches("vapid t=")
            .split(", k=")
            .next()
            .unwrap();
        assert_eq!(token.split('.').count(), 3);
    }
}
