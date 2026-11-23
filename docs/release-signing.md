# Signing releases

`.github/workflows/release.yml` builds the binaries for every `v*` tag. It signs the macOS and Windows binaries when the secrets below exist in the GitHub environment **`release`**; a platform without its secrets is built unsigned and the workflow says so in a warning. The setup mirrors the one used for mumcord, with the same secret names.

The `release` environment only accepts `v*` tags, so the secrets are never available to pull requests or branch builds.

The same secrets sign both things a release carries: the `boxquell` command (`release.yml`) and the desktop app's packages (`desktop.yml`, called by the release). The desktop app's bundler signs and notarizes the macOS app itself from the Apple values, and signs the Windows program and its installer with `scripts/sign-windows.ps1`.

## Windows: Certum certificate through SimplySign

The certificate lives in Certum's cloud HSM. The workflow installs SimplySign Desktop (pinned by version and SHA-256), logs in with a TOTP code and signs with `signtool`.

| Name | Kind | Value |
|---|---|---|
| `CERTUM_USER` | secret | E-mail address of the SimplySign account |
| `CERTUM_TOTP_SECRET` | secret | The full `otpauth://…` text behind the SimplySign QR code |
| `CERTUM_CERT_SHA1` | variable | SHA-1 thumbprint of the code signing certificate |

What runs: `signtool sign /sha1 <thumbprint> /fd sha256 /tr http://time.certum.pl /td sha256 /d Boxquell boxquell.exe`, then `signtool verify /pa /v`.

The login types into the SimplySign window (`scripts/ci/simplysign-login.ps1`) and is fragile by nature. When it fails, the release fails. Sign by hand on a Windows machine with SimplySign Desktop logged in:

```powershell
pwsh -File scripts/sign-windows.ps1 -Thumbprint <thumbprint> path\to\boxquell.exe
```

A new SimplySign Desktop version needs its version number and checksum updated in the workflow (`SIMPLYSIGN_VERSION`, `SIMPLYSIGN_SHA256`).

## macOS: Developer ID and notarization

| Name | Kind | Value |
|---|---|---|
| `APPLE_CERTIFICATE` | secret | The "Developer ID Application" certificate with its key as `.p12`, base64-encoded (`base64 -i cert.p12`) |
| `APPLE_CERTIFICATE_PASSWORD` | secret | Password of that `.p12` |
| `APPLE_SIGNING_IDENTITY` | secret | `Developer ID Application: Your Name (TEAMID)` |
| `APPLE_API_ISSUER` | secret | Issuer ID of the App Store Connect API key |
| `APPLE_API_KEY` | secret | Key ID of that API key |
| `APPLE_API_KEY_P8` | secret | Full text of the `.p8` key file |

What runs: `codesign --options runtime --timestamp --entitlements assets/entitlements.plist`, then `xcrun notarytool submit --wait`. The entitlements allow starting virtual machines (`com.apple.security.hypervisor`) and loading libkrun, which is installed separately.

Boxquell ships as a bare binary, which cannot carry a stapled notarization ticket. Gatekeeper therefore checks it online the first time it runs.

Without the Apple secrets the binary still gets an ad-hoc signature with the entitlements, because macOS refuses the Hypervisor framework to a binary without them.

## Checking a release before announcing it

1. Push a release-candidate tag, e.g. `v0.1.0-rc1`, and watch the workflow.
2. Windows: right-click `boxquell.exe` → Properties → Digital Signatures, or `signtool verify /pa /v boxquell.exe`.
3. macOS: `codesign --verify --strict --verbose=2 boxquell` and `spctl --assess --type execute --verbose boxquell`.
4. Compare the download with `SHA256SUMS`, and check its build provenance: `gh attestation verify <file> --repo Boxquell/boxquell`.
5. Delete the release candidate when everything is fine, then tag the real version.
