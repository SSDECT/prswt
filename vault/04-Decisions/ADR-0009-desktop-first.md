# ADR-0009 — Desktop app is the product; the server keeps only API + MCP
**Status:** accepted (user, 2026-10-24)
**Context:** the egui UI ran both natively and in the browser (WEB-01). In the browser it is canvas-only, ~4 MB of WASM, calls the server with synchronous XHR (freezes on long commands) and lacks file dialogs and the control channel. The user found the web page poor and asked for: a fully finished desktop app, local with no sign-in, on Windows, Linux and macOS; the server only for what is needed.
**Decision:**
- `prswt-desktop` is the primary UI. It runs the engine in-process (`LocalEngine`), needs no account, and saves projects as local `.prswt` files.
- Project files are produced and read by engine commands (`file.save_project`, `file.open_project`) that take or return text, like the XER/MSPDI commands. Only the desktop shell touches the file system, so agents still never get file-system access.
- `prswt-server` keeps `/v1/*` (HTTP API), `/mcp` (remote MCP), sign-in and the `/account` token page (needed to issue API/MCP tokens). It no longer serves the WASM web UI.
- Releases are built for x86_64 Windows (MSVC), x86_64 Linux (glibc; Wayland + X11) and macOS (Apple Silicon + Intel). macOS needs a macOS build host, so all three come from one CI matrix.
**Consequences:** `prswt-web` and the `HttpEngine` leave the deployment (the crate is removed, the `Engine` trait stays as the UI's seam for tests). Long commands must run off the UI thread in the desktop app. Unsigned macOS/Windows binaries show OS warnings until code-signing certificates exist.
