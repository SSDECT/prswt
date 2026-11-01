# Microsoft Store release

> **Updated:** 2026-11-01 · Scorecard DSK-13

The Store is how PRSWT gets a trusted Windows signature for free: Partner Center signs the MSIX package with a Microsoft certificate, so Store installs show no SmartScreen warning. The zip on GitHub releases stays unsigned until a code-signing certificate is set up (`docs/signing.md`).

## How the package is built

`tools/package-desktop.sh` on Windows also builds `dist-store/PRSWT-store-x64.msix` with `makeappx` from the Windows SDK:

- `tools/msix/AppxManifest.xml`: a full-trust Win32 app (`runFullTrust`), `.prswt` file association, `internetClient` for the optional cloud.
- `tools/msix/logos.py`: draws the Store logos (the same picture as the in-app icon), so the repo ships no image files.
- Version: `engine/Cargo.toml` version plus `.0` (the Store needs the fourth part to be 0).
- Identity: the repo variables `MSIX_IDENTITY_NAME`, `MSIX_PUBLISHER` and `MSIX_PUBLISHER_NAME`. Without them the package gets a placeholder identity, builds, and the workflow warns; Partner Center rejects that package.

The `desktop-release` workflow uploads it as the `prswt-microsoft-store` build artifact. It is not attached to the GitHub release, because an unsigned `.msix` does not install outside the Store.

## One-time setup (owner)

1. **Developer account.** Go to <https://storedeveloper.microsoft.com>, choose **Individual developer**, sign in with a personal Microsoft account and finish the ID check (an ID document and a selfie). It is free.
2. **Reserve the name.** Partner Center ▸ **Apps and games ▸ New product ▸ MSIX or PWA app**, name `PRSWT`. If it is taken, reserve a longer name (for example `PRSWT Project Controls`) and change `DisplayName` in the manifest to match.
3. **Copy the identity.** In the new product: **Product management ▸ Product identity**. Note three values:
   - `Package/Identity/Name` (like `12345SSDECT.PRSWT`)
   - `Package/Identity/Publisher` (like `CN=1A2B3C4D-…`)
   - `Package/Properties/PublisherDisplayName`
4. **Store them as repo variables** (they are public identifiers, not secrets):
   ```
   gh variable set MSIX_IDENTITY_NAME --body "<Name>"
   gh variable set MSIX_PUBLISHER --body "<Publisher>"
   gh variable set MSIX_PUBLISHER_NAME --body "<PublisherDisplayName>"
   ```

## Each release

1. Tag the release as usual (`v*`), or run `desktop-release` by hand.
2. Download the `prswt-microsoft-store` artifact from the run and unzip it.
3. Partner Center ▸ PRSWT ▸ **Start your submission** (or **Update** for a new version):
   - **Pricing and availability:** Free; markets: all.
   - **Properties:** **Business** or **Productivity**. Privacy policy URL: `https://github.com/SSDECT/prswt/blob/main/docs/privacy.md`. Website: `https://github.com/SSDECT/prswt`. Support: `https://github.com/SSDECT/prswt/issues`.
   - **Age ratings:** fill in the questionnaire (no user-generated content shared publicly, no purchases); it comes out as 3+ / Everyone.
   - **Packages:** upload `PRSWT-store-x64.msix`.
   - **Store listings (English):** description and features from the README; screenshots from `docs/examples/screens/` (at least one, 1366×768 or larger).
   - **Submission options ▸ restricted capability `runFullTrust`:** "PRSWT is a desktop (Win32) application built with Rust and egui. It needs full trust to run as a normal desktop process: it reads and writes project files the user picks, and can run a local API on the loopback address."
4. **Submit.** Certification usually takes 1–3 working days. The result comes by email; if a check fails, the report says which one.

Later versions only need steps 1–3 with a higher version in `engine/Cargo.toml`: the Store refuses a package whose version is not higher than the published one.
