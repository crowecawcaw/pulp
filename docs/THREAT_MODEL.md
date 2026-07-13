# Threat Model

Pulp is a single-tenant tool that runs on one machine — typically a laptop or
a home-lab box — and is **deliberately unauthenticated**: the API and web UI
have no login, and adding one is a non-goal. The security boundary is instead
**network reachability plus the browser's same-origin policy**. Anyone who can
open a TCP connection to a Pulp listener has full admin: they can read every
mention, read stored collector credentials, and rewrite configuration.

The server has two listeners, both careful about where they bind:

- **HTTP** on `server.host:port` (`BIND`), default `127.0.0.1:3000` —
  loopback only unless the operator explicitly configures otherwise.
- **HTTPS** (when certificates resolve, e.g. via Tailscale) on
  `server.https.port`. When the HTTP bind is loopback, this listener binds
  the machine's **Tailscale IP** (`100.x.y.z`), not `0.0.0.0` — reachable
  from the operator's tailnet, invisible to the surrounding LAN.

So the intended trust model is: *everyone on the tailnet (and everyone with a
local account on the machine) is the operator*. Threats below are evaluated
against that model. If you can't grant tailnet-wide trust, don't enable the
Tailscale listener — or segment with Tailscale ACLs, which act as the
missing per-user layer.

The browser is the one attacker that is always "inside" the network
perimeter: any web page the operator visits runs code on a machine that can
reach the listeners. Pulp therefore sends **no CORS headers at all**, leaving
the browser's same-origin policy fully intact, and requires a JSON body
(which browsers only send cross-origin after a CORS preflight) on
state-changing endpoints.

## Threats

| # | Threat | Mitigation | Evidence |
|---|--------|------------|----------|
| 1 | **Attacker on the same LAN / Wi-Fi connects to the API.** | Not reachable. The HTTP listener binds loopback by default; the HTTPS listener binds the Tailscale IP specifically when the HTTP bind is loopback, so neither accepts LAN connections. Operators who set `BIND` to a non-loopback address opt out of this (respected as configured, warned in README/SECURITY). | Default bind `backend/src/config.rs` (`127.0.0.1`); HTTPS bind-IP selection `backend/src/server.rs` (`serve_https`, loopback → `resolved.bind_ip`); Tailscale IP resolution `backend/src/tls.rs` (`ResolvedTls::bind_ip`). |
| 2 | **Malicious website in the operator's browser calls the API to steal credentials or rewrite config** (drive-by: the page fetches `http://127.0.0.1:3000/api/channels` for the GitHub token, or points `config/ai.base_url` at an attacker host to capture the stored LLM key). | Same-origin policy, deliberately unbroken: the server emits **no** `Access-Control-Allow-*` headers, so browsers refuse to expose responses to foreign pages and never send preflighted writes. State-changing endpoints require `Content-Type: application/json`, which cross-origin HTML forms cannot send without a preflight — form-encoded writes are rejected before any handler runs. Nothing legitimate needs cross-origin access: the production UI is embedded and served same-origin; the Vite dev server proxies `/api`. | No CORS layer in `backend/src/api/mod.rs` (`router`, with a warning comment); JSON extractors on write handlers (e.g. `backend/src/api/config.rs`); regression tests `backend/tests/test_cross_origin.rs`; dev proxy `frontend/vite.config.ts`. |
| 3 | **Attacker (or untrusted device/user) on the tailnet calls the API.** | **Accepted — intentional.** Tailnet membership *is* the authorization layer: the HTTPS listener exists precisely so tailnet devices (the operator's phone PWA) get full access without a login. A hostile tailnet node has full admin. Operators who need finer grain should use Tailscale ACLs to restrict which nodes can reach this machine's ports. | Tailnet-facing bind is deliberate: `backend/src/server.rs` (`serve_https` comment), `backend/src/tls.rs` (`install_url` hands the tailnet URL to other devices); trust statement in `SECURITY.md`. |
| 4 | **DNS rebinding**: a malicious page's origin (`evil.example`) is re-pointed at `127.0.0.1` after load, making the browser treat the API as same-origin and bypassing threat 2's defense. | Partial / residual. The HTTPS listener is immune (the TLS certificate is for the tailnet name; a browser resolving `evil.example` to it fails the handshake). The plain-HTTP loopback listener does not validate the `Host` header today, so it remains theoretically exposed; harm requires the operator's browser to hold a page open for the rebind window. Host-header validation is the known fix if this risk becomes unacceptable. | TLS listener `backend/src/server.rs` (`serve_https`); no Host validation in `backend/src/api/mod.rs` (documented residual, not an oversight). |
| 5 | **Cross-origin "simple" requests as CSRF**: a foreign page fires a body-less `POST` (no preflight needed) at admin trigger endpoints. | Accepted. The only endpoints reachable this way (`/api/admin/collect/:channel`, `/api/admin/notify`) merely run an already-scheduled poll/notify pass early — no data is disclosed (response unreadable cross-origin, per threat 2) and no configuration changes. Every endpoint with meaningful side effects requires a JSON body and is preflight-protected. | Handler signatures in `backend/src/api/admin.rs` (`trigger_collect`/`trigger_notify` take no body; `trigger_backfill` requires JSON); `backend/tests/test_cross_origin.rs` (`form_encoded_writes_are_rejected`). |
| 6 | **Attacker with local file access reads credentials at rest** (GitHub token, LLM API key in the SQLite DB / `config.json`). | Out of scope for the process; delegated to OS file permissions. Documented operator guidance: `chmod 600` the DB, keep `config.json` private. A local account on the machine is inside the trust boundary (it could equally connect to the loopback listener). | `SECURITY.md` (Credential Storage). |

## Non-goals

- **User authentication / multi-tenancy** — see narrative above; network
  reachability is the access control.
- **Protecting against a hostile operator or hostile localhost** — any local
  process can already reach the loopback listener.
- **Egress protection (SSRF)** — the operator configures webhook and LLM
  endpoint URLs; the server calls what it is told to call. Single-tenant, so
  the "attacker" would be the operator themself.

## Keeping this honest

- `backend/tests/test_cross_origin.rs` fails if a CORS layer is ever
  reintroduced or if form-encoded writes start being accepted.
- Any new listener must follow the bind rules in the narrative (loopback or
  Tailscale IP — never `0.0.0.0` by default).
- Any new state-changing endpoint must take a JSON body (preflight
  protection) unless its side effect is as benign as threat 5's.
