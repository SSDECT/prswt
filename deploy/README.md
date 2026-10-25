# Deploying PRSWT to `prswt.juz.ai`

> **Last updated:** 2026-10-24 · Target: VM 130 `apps` (Proxmox vm01 guest, Debian 13, Docker), Tailscale `100.64.0.13`.

```
Browser / Claude ──HTTPS──▶ Cloudflare edge ──Tunnel (cloudflared on VM 130)──▶ 127.0.0.1:8460 ──▶ prswt-server ──▶ postgres (internal network)
```
TLS terminates at Cloudflare. Nothing is published to the internet from VM 130; the server binds only on loopback `127.0.0.1` (cloudflared on the same VM uses host networking).

## One-time prerequisites (owner)
1. **Google sign-in**: Google Cloud Console → APIs & Services → Credentials → OAuth client (Web application).
   - Authorized redirect URI: `https://prswt.juz.ai/auth/callback/google`
   - Authorized JavaScript origin: `https://prswt.juz.ai`
2. **Microsoft sign-in (optional)**: Entra → App registrations → New.
   - Platform "Web", redirect URI `https://prswt.juz.ai/auth/callback/microsoft`.
   - Create a client secret.
   - Choose single tenant (Juz.ai only, then `MICROSOFT_TENANT=<tenant id>`) or multi-tenant (`MICROSOFT_TENANT=organizations` plus `MICROSOFT_ALLOWED_TENANTS`). `MICROSOFT_TENANT` has no default: the server refuses to start without it.
3. **Cloudflare Tunnel** (JuzSign tunnel): add a published application route `prswt.juz.ai` → **HTTPS** `127.0.0.1:8460`. Under *Additional settings → TLS*, set **Origin Server Name** to `prswt.juz.ai` and leave *No TLS Verify* **off**. cloudflared then verifies PRSWT's Let's Encrypt certificate end to end.
4. **Allowed emails**: the comma-separated list for `PRSWT_ALLOWED_EMAILS`.

Secrets go **only** into `~/prswt/deploy/.env` on the server (`chmod 600`, owner `srs`). They move to Bitwarden later.

## Build & deploy (operator)
1. Copy the source to the server: `engine/` (without `target/`) and `deploy/` → `~/prswt/`.
2. On the server, in `~/prswt/deploy`:
   - Create `.env` from `.env.example`.
   - Generate the DB password: `POSTGRES_PASSWORD=$(openssl rand -base64 32 | tr -d '/+=')`.
3. Run `docker compose up -d --build`. The image compiles the server inside Docker (Rust on Debian trixie).
4. Smoke test from VM 130: `curl -s http://127.0.0.1:8460/healthz` should print `ok`, and `curl -sI http://127.0.0.1:8460/` should redirect to `/login`.
5. Public test: open `https://prswt.juz.ai` → sign in → **Account** (the only page; there is no web UI) → create a token → connect Claude:
   `claude mcp add --transport http prswt https://prswt.juz.ai/mcp --header "Authorization: Bearer <token>"`

## TLS on the last hop
PRSWT serves HTTPS itself (`PRSWT_TLS_CERT`/`PRSWT_TLS_KEY`). The certificate is from Let's Encrypt, obtained by DNS-01 with the Cloudflare DNS token:
- `printf '%s
' "$TOKEN" | ./le_cert.sh run` issues it; the key is generated on VM 130 and never leaves it.
- Files are in `tls/certificates/`, mode 600, owned by `srs`. The container runs as uid 1000 and mounts `tls/` read-only.
- Renewal: the `cert-renew` compose service checks daily and renews below 30 days left, using `tls/cf.env` (`CF_DNS_API_TOKEN=…`, mode 600). The server checks the certificate file hourly and hot-reloads it, so no restart is needed. If a reload fails, it keeps the old certificate.
- Without renewal the certificate expires after 90 days (the current one on 2027-01-21).

## Operations
- Logs: `docker compose logs -f server`.
- Update: rebuild the web UI, copy the source, then `docker compose up -d --build` in `deploy/`. Pack the source on Windows with `git -c core.autocrlf=false archive -o src.tar HEAD`: with CRLF line endings the migrations no longer match their recorded checksums and the server refuses to start.
- Backup: `docker compose exec db pg_dump -U prswt prswt | gzip > /opt/backups/prswt-$(date +%F).sql.gz`.
- Rollback: `docker compose down` (data stays in the `pgdata` volume), then redeploy the previous source.
- Revoke access: remove the email from `PRSWT_ALLOWED_EMAILS` (or its domain from `PRSWT_ALLOWED_DOMAINS`) and remove it from the projects it was invited to, then restart. Sessions and tokens of removed users stop working immediately.
- Sharing (ADR-0010): each project has members (owner, editor, viewer). Project owners invite people from the desktop app (File ▸ Cloud ▸ Share). Projects created before sharing existed belong to the people in `PRSWT_ALLOWED_EMAILS`.
- Other organizations: set `MICROSOFT_TENANT=organizations`, list the tenant ids allowed to sign in in `MICROSOFT_ALLOWED_TENANTS` (comma-separated), and make the Azure app registration multi-tenant ("Accounts in any organizational directory"). In this mode an email is accepted only when its directory verified it (the `xms_edov` claim: add it as an optional ID-token claim in the app registration), because anyone can put any address on an account in their own directory ("nOAuth").
- Read-only tokens change nothing (not even creating projects). A token can revoke itself (`DELETE /v1/me/token`; the desktop app does this when signing out).
