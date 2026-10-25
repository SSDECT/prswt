# Signing the desktop app

> **Updated:** 2026-10-26

The release workflow (`.github/workflows/desktop-release.yml`) signs each package when its secrets are set in the GitHub repo (Settings ▸ Secrets and variables ▸ Actions). A `v*` tag without them fails, so no unsigned release ships. Manual runs (workflow_dispatch) still build unsigned for testing. Keep every value in Bitwarden too (`prswt/<name>`).

## Windows: Azure Trusted Signing

Removes the SmartScreen "unknown publisher" warning. Costs about $10 a month (Basic SKU).

1. Create an Azure subscription in the juz.ai tenant (the tenant has none yet).
2. Register the resource provider `Microsoft.CodeSigning`, then create a **Trusted Signing account**. Note its region endpoint, e.g. `https://weu.codesigning.azure.net/`.
3. In the account: **Identity validation ▸ New ▸ Public**. Microsoft checks the organisation (legal name, address, website); this takes 1–3 business days.
4. After approval: **Certificate profiles ▸ Create ▸ Public Trust**, using that validation.
5. Create an Entra app registration (e.g. `prswt-signing`) with a client secret. On the signing account, give it the role **Trusted Signing Certificate Profile Signer**.
6. GitHub secrets:

| Secret | Value |
|---|---|
| `AZURE_TENANT_ID` | juz.ai tenant id |
| `AZURE_CLIENT_ID` | app registration's client id |
| `AZURE_CLIENT_SECRET` | its client secret |
| `AZURE_SIGNING_ENDPOINT` | region endpoint from step 2 |
| `AZURE_SIGNING_ACCOUNT` | signing account name |
| `AZURE_SIGNING_PROFILE` | certificate profile name |

## macOS: Developer ID + notarization

Removes the Gatekeeper "cannot be opened" dialog. Needs the Apple Developer Program ($99 a year).

1. Enrol at <https://developer.apple.com/programs/> as the organisation (needs a D-U-N-S number) or as an individual.
2. **Certificates ▸ +** ▸ **Developer ID Application**. Create the CSR in Keychain Access on a Mac, download the certificate, then export it with its key as a `.p12` with a password.
3. At <https://account.apple.com>: **Sign-In and Security ▸ App-Specific Passwords** ▸ create one for `prswt-notary`.
4. GitHub secrets:

| Secret | Value |
|---|---|
| `MACOS_CERTIFICATE_P12` | `base64 -i cert.p12` output |
| `MACOS_CERTIFICATE_PASSWORD` | the `.p12` password |
| `MACOS_SIGN_IDENTITY` | `Developer ID Application: <Name> (<TEAMID>)` |
| `APPLE_ID` | Apple ID email |
| `APPLE_TEAM_ID` | 10-character team id |
| `APPLE_APP_PASSWORD` | the app-specific password |

The package script signs `PRSWT.app` with the hardened runtime, signs the `.dmg`, notarizes it (`notarytool --wait`) and staples the ticket.

## Release

Once the secrets are set, push a tag (`git tag v0.1.2 && git push origin v0.1.2`). Check the results:
- Windows: `Get-AuthenticodeSignature prswt.exe` reports `Valid`.
- macOS: `spctl -a -vv -t install prswt-*.dmg` reports `accepted, source=Notarized Developer ID`.
