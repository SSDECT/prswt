---
type: plan
status: active
updated: 2026-10-24
---
# Deployment plan: publish PRSWT on the user's Linux VPS

Decisions (user, 2026-10-24): host **prswt.juz.ai** on the app server (Tailscale `<server-tailscale-ip>`) via the existing Cloudflare Tunnel · Linux VPS over SSH · sign-in with Google/Microsoft · web UI + remote MCP + HTTP API · own domain.

## Target architecture
```
Internet ─HTTPS─▶ Cloudflare edge ─tunnel (cloudflared on the app server)─▶ prswt-server <server-lan-ip>:8460 ─▶ PostgreSQL (compose-internal only)
                                                         ├─ /            web UI (egui → WASM, static files)
                                                         ├─ /auth/*      OIDC login (Google, Microsoft), sessions
                                                         ├─ /v1/*        HTTP API (session cookie or bearer token)
                                                         └─ /mcp         remote MCP, streamable HTTP (bearer token)
```

## Work items (new scorecard section P10)
| Id | Item | Notes |
|---|---|---|
| SRV-01 | `prswt-server` binary: one axum server with API + MCP + static web + auth | reuses prswt-api, prswt-mcp, prswt-app, prswt-store |
| AUTH-01 | OIDC Authorization Code + PKCE for Google and Microsoft; email allowlist | `openidconnect` crate or hand-rolled with reqwest + JWT verify |
| AUTH-02 | Server-side sessions (HttpOnly, Secure, SameSite=Lax cookie) in Postgres | CSRF: SameSite + origin check on writes |
| AUTH-03 | Personal access tokens for MCP/API (created in the web UI, stored hashed) | `Authorization: Bearer prswt_…` |
| AUTH-04 | Roles: owner/admin from the allowlist; logged-in users act as humans; MCP tokens act as agents | keeps the sandbox safety policy |
| MCP-13 | Streamable HTTP transport for MCP (`POST /mcp`, JSON replies) | `claude mcp add --transport http prswt https://<domain>/mcp --header "Authorization: Bearer …"` |
| WEB-01 | Web build of the UI: `PrswtApp` over a `Backend` trait (local workspace or remote HTTP via fetch) | eframe web + trunk; async request queue |
| OPS-01 | Multi-stage Dockerfile, docker-compose (server, postgres) in `/opt/prswt`, healthchecks, volumes, backups | non-root, read-only FS; TLS terminates at Cloudflare |
| OPS-02 | Deploy over Tailscale SSH to `<user>@<server-tailscale-ip>`; tunnel public hostname `prswt.juz.ai` → `http://<server-lan-ip>:8460` | secrets in `/opt/prswt/.env` (0600, owner 1000) → Bitwarden |
| OPS-03 | System test against the public URL (login redirect, token MCP, API, web UI loads) | extends tools/system_test.py |

## Needed from the user
1. SSH: host, port, user, and my key authorized (or a password the user enters themselves).
2. Domain/subdomain, with a DNS A record pointing to the VPS.
3. OAuth apps: Google Cloud Console (OAuth client "Web application") and/or Microsoft Entra app registration, with redirect URIs `https://<domain>/auth/callback/google` and `/auth/callback/microsoft`. Client IDs and secrets go only into the server `.env`.
4. Allowed login emails.
