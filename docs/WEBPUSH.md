# Web Push notifications

Pulp can push feed mentions to browsers/installed PWAs (phone or desktop)
with no third-party account — it's a notification **destination** of type
`webpush`, alongside the generic `webhook` destination.

- **Pure-Rust, no native build deps.** RFC 8291 message encryption
  (`aes128gcm`) and RFC 8292 VAPID (ES256 JWT) are implemented with
  RustCrypto (`p256`/`hkdf`/`aes-gcm`) in `backend/src/notifier/webpush.rs`
  instead of the `web-push` crate, which pulls in OpenSSL via `ece` — that
  would break this project's rustls/ring, single-static-binary stance. A unit
  test pins the encryption against the RFC 8291 Appendix A worked example.
- **VAPID identity** is generated once and persisted to `<home>/vapid.json`
  (rotating it would invalidate every subscription, so a malformed file is
  fatal, not silently regenerated). The public key is served to the browser.
- **Subscriptions are workspace-scoped**, not device-global: each browser
  subscription is its own `webpush` notification row (`config` =
  `{endpoint, p256dh, auth}`) in the same per-workspace `notifications` table
  as webhooks. A feed-visible mention is delivered to every `webpush`
  notification in its workspace; a subscription the push service reports as
  gone (HTTP 404/410) is pruned by deleting that notification.
- **Endpoints**: `GET /api/push/vapid-public-key` returns the browser's
  `applicationServerKey` (`backend/src/api/push.rs`). Subscriptions
  themselves aren't a separate resource — a browser subscription is a
  per-workspace `webpush` notification via the generic
  `POST /api/notifications` / `DELETE /api/notifications/{id}`
  (`backend/src/api/notifications.rs`).
- **Frontend**: vite-plugin-pwa's `injectManifest` strategy so the custom
  service worker (`frontend/src/sw.ts`) carries `push`/`notificationclick`
  handlers plus the Workbox precache. `frontend/src/lib/push.ts` handles the
  subscribe flow; the Settings page's `NotificationsSettings` component
  exposes "Web Push" as a destination and shows Add-to-Home-Screen
  instructions on iOS (Web Push only reaches installed PWAs there).
