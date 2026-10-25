# Packaging Jarvis

Jarvis is configured to produce native installers with Tauri:

| OS | Artifacts |
| --- | --- |
| Windows 10/11 | NSIS setup `.exe` and WiX `.msi` |
| macOS 10.15+ | `.app` and `.dmg` |
| Linux | `.deb`, `.rpm` and `.AppImage` |

Installers must be built on the target OS. The Linux development server used for this repository is headless and does not have the WebKit/GTK toolchain, so the final Windows package must be built on a Windows PC or CI runner.

## Versioning

The version must match in all three files:

- `package.json`
- `Cargo.toml` → `[workspace.package]`
- `src-tauri/tauri.conf.json`

Check them with:

```bash
pnpm version:check
```

Bump all three before a release. The installer and app version come from `tauri.conf.json`; the Rust build uses the Cargo workspace version.

## Windows

### Prerequisites

- Windows 10/11 x64
- Node.js 22 LTS
- `pnpm` 10
- Rust stable with the `x86_64-pc-windows-msvc` toolchain
- Visual Studio Build Tools with **Desktop development with C++**
- WebView2 (already present on Windows 11)

See `RUN-ON-DESKTOP.md` for the full setup.

### Build

From the repository root:

```powershell
pnpm install
pnpm version:check
powershell -ExecutionPolicy Bypass -File scripts/package.ps1
```

The script runs `pnpm tauri build`, copies the installers to `artifacts/`, and writes `artifacts/SHA256SUMS.txt`.

You can also run Tauri directly:

```powershell
pnpm tauri build
```

Typical output locations:

```
target\release\bundle\nsis\Jarvis_<version>_x64-setup.exe
target\release\bundle\msi\Jarvis_<version>_x64_en-US.msi
```

If Tauri uses the nested target directory in your checkout, look under `src-tauri\target\release\bundle`. The packaging script searches both.

If WiX/MSI is unavailable or fails on the build machine, build only the NSIS installer:

```powershell
pnpm tauri build --bundles nsis
```

The current NSIS installer is **per-user** (`currentUser`) and does not require administrator rights. MSI is also produced by the `all` bundle target. WebView2 uses the `downloadBootstrapper` mode, so a Windows 10 machine without WebView2 needs internet during the first install.

### Code signing

The default build is unsigned and Windows SmartScreen will warn users. For a trusted release, use an Authenticode code-signing certificate.

Option A — let Tauri sign during the build:

1. Import the certificate into `CurrentUser\My` on the build machine.
2. Copy its SHA-1 thumbprint.
3. Add these fields under `bundle.windows` in `src-tauri/tauri.conf.json`:

```json
"certificateThumbprint": "YOUR_CERT_THUMBPRINT",
"digestAlgorithm": "sha256",
"timestampUrl": "http://timestamp.digicert.com"
```

Do not commit the certificate or its password. This is suitable for a dedicated Windows build machine.

Option B — sign after packaging:

```powershell
signtool sign /fd SHA256 /td SHA256 `
  /tr http://timestamp.digicert.com `
  /f C:\path\to\certificate.pfx /p $env:CERT_PASSWORD `
  artifacts\Jarvis_<version>_x64-setup.exe
```

Sign both the application executable and the installer for the best result. Tauri's Windows signing configuration is documented at <https://v2.tauri.app/distribute/sign/windows/>.

### Smoke test

Install the NSIS setup on a clean Windows VM or another user account, then verify:

- the app launches from the Start menu;
- the existing `com.mcpchat.app` data and keychain entries are still found after an upgrade;
- provider setup and one chat turn work;
- MCP connection and approval work;
- image/file picker, drag-and-drop and attachment preview work;
- workspace selection and revert work;
- Settings → Export conversation and Delete all data work;
- uninstall removes the app.

If you want to remove local data before uninstalling, use **Settings → Delete all data** first.

## macOS

Build on macOS with the Tauri prerequisites:

```bash
pnpm install
pnpm version:check
pnpm tauri build
```

For distribution outside your machine, sign and notarize the app. Set the standard Tauri/Apple environment variables (`APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID`) and rebuild. The app targets macOS 10.15 or newer.

## Linux

Install the Tauri dependencies listed in `RUN-ON-DESKTOP.md`, then:

```bash
pnpm install
pnpm version:check
./scripts/package.sh
```

Artifacts are copied to `artifacts/`. The script currently collects `.deb`, `.rpm` and `.AppImage` bundles.

## Icons and installer artwork

The source app icon is `app-icon.png` (1024×1024 Jarvis hexagonal mark). Regenerate platform icons with:

```bash
pnpm tauri icon app-icon.png
```

The NSIS installer artwork lives in:

- `src-tauri/icons/installer-header.bmp` (150×57)
- `src-tauri/icons/installer-sidebar.bmp` (164×314)

Regenerate or replace those separately if the brand changes.

## Not included yet

- automatic updates / Tauri updater signatures;
- Microsoft Store MSIX packaging;
- a hosted CI release pipeline;
- notarized macOS builds (the configuration is prepared, but a certificate is required).
