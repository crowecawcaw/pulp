# Security Policy

## Threat Model

Pulp is **deliberately unauthenticated**. The API and web UI require no login by design — the MVP is intended to run behind your own network controls and not be exposed to the public internet. The full threat/mitigation table — which attackers are defended against, which are accepted, and where the evidence lives — is in [docs/THREAT_MODEL.md](docs/THREAT_MODEL.md).

If you deploy Pulp:

- **Restrict network access** to trusted users only (firewall, VPN, private network, etc.)
- **Access-restrict the database file** (`~/.pulp/pulp.db`) — all data and credentials are stored there
- **Keep the `config.json` private** — it may contain API keys for collectors and AI filters

## Credential Storage

Collector credentials (GitHub tokens, API keys, etc.) are stored as JSON in the SQLite database (`channel_configs.credentials`). There is no separate secrets table. Ensure your database file has appropriate file permissions:

```bash
chmod 600 ~/.pulp/pulp.db
```

## Reporting Vulnerabilities

If you discover a security vulnerability, please **open a private security advisory** on GitHub:

1. Go to **Security** → **Advisories** on the repository
2. Click **Report a vulnerability**
3. Describe the issue and any proof-of-concept

Do NOT open a public issue for security vulnerabilities. We will triage and coordinate a fix with you before public disclosure.

## Dependency Updates

Keep Rust and Node.js dependencies up to date:

```bash
# Rust
cd backend && cargo update

# Node.js
cd frontend && npm update
```

Check for security advisories regularly via `cargo audit` and `npm audit`.

---

For questions about security, please open a security advisory or contact via GitHub issues.
