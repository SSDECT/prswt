# PRSWT privacy policy

> **Effective:** 2026-11-01 · Applies to the PRSWT desktop app (all platforms and the Microsoft Store version) and the optional PRSWT cloud at prswt.juz.ai.

## Summary

The desktop app works offline and collects nothing. It has no telemetry, no analytics, no advertising and no crash reporting. Data leaves your computer only when you sign in to the optional cloud and choose to use it.

## Without signing in

- Your projects are stored only in the `.prswt` files you save, wherever you choose to save them.
- The app keeps its window layout and recent-file list in its local settings folder.
- The app makes no network connections. The local API and MCP channel, when you start them, listen only on your own computer (127.0.0.1 / ::1).

## When you use the cloud (File ▸ Cloud ▸ Sign in…)

- **Sign-in:** you sign in with a Microsoft or Google account in your browser. PRSWT keeps only your email address from that provider, to identify you, plus which provider you used. Your password never reaches PRSWT. Sign-in is limited to invited people and the server's access list.
- **Access token:** the app stores the token for this computer in the operating system's credential store (Windows Credential Manager, macOS Keychain, Linux Secret Service). **Sign out** deletes it and revokes it on the server.
- **Projects:** projects you save to the cloud, their change history and their member list (email and role) are stored on the server so they can sync and be shared. A local copy is cached on your computer so you can work offline.
- **Presence:** while you have a cloud project open, the people you share it with can see your email as "also here".
- **Tokens and sessions:** sign-in sessions and the tokens you create for the desktop app, Claude or other MCP clients are stored only as hashes, with their name and type.

We do not sell, rent or share this data with third parties, and we do not use it for advertising. It is used only to provide sync, sharing and sign-in.

## Your choices

- You can use PRSWT fully without an account.
- You can remove people from projects you own, and revoke tokens at <https://prswt.juz.ai/account>.
- To have a cloud project, or your account and all its data, deleted, open an issue at <https://github.com/SSDECT/prswt/issues> (or write to the project owner through GitHub) and the data is removed within 30 days.

## Changes

Changes to this policy are made in this file; its history is visible on GitHub.

## Contact

<https://github.com/SSDECT/prswt/issues>
