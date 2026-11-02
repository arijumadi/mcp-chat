# Menjalankan Jarvis di PC desktop

Repo ini dibuat di server headless. Untuk melihat window aplikasi, pindahkan **source** ke PC yang punya layar, lalu build di sana. Jangan copy `node_modules/` atau `target/` — keduanya harus dibuat ulang di PC desktop.

## 1. Ambil source

Pilih salah satu:

**Download ZIP** (paling mudah, termasuk untuk Windows):

```
mcp-chat-src.zip
```

**SCP** dari server (ganti `user@server` kalau berbeda):

```bash
scp user@server:/path/to/downloads/mcp-chat-src.zip .
```

**Git bundle** (kalau ingin riwayat git):

```bash
scp user@server:/path/to/downloads/mcp-chat.bundle .
git clone mcp-chat.bundle mcp-chat
```

Kalau PC tidak satu jaringan, gunakan alamat server yang bisa diakses dari PC target.

Ekstrak ZIP, lalu buka terminal/PowerShell **di dalam folder `mcp-chat`**.

## 2. Prasyarat

### Windows 10/11

1. **Node.js 22 LTS** dari <https://nodejs.org> (centang “Add to PATH”).
2. **pnpm**:
   ```powershell
   npm install -g pnpm@10
   ```
3. **Rust** dari <https://rustup.rs> — pilih toolchain default `x86_64-pc-windows-msvc`.
4. **Visual Studio Build Tools**:
   - Installer: <https://visualstudio.microsoft.com/visual-cpp-build-tools/>
   - Pilih workload **Desktop development with C++** (MSVC v143 + Windows 10/11 SDK).
5. **WebView2**: Windows 11 sudah punya. Windows 10 bisa install “Evergreen WebView2 Runtime” dari Microsoft.
6. Tutup dan buka ulang PowerShell setelah install supaya PATH terbaca.

Jalankan:

```powershell
pnpm install
pnpm tauri dev
```

Atau:

```powershell
powershell -ExecutionPolicy Bypass -File scripts/run-desktop.ps1
```

### Linux (Ubuntu / Debian / Linux Mint)

```bash
sudo apt update
sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file \
  libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev
```

Lalu install Node 22, pnpm, dan Rust:

```bash
npm install -g pnpm@10
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Jalankan:

```bash
pnpm install
pnpm tauri dev
```

Atau:

```bash
./scripts/run-desktop.sh
```

### macOS

```bash
xcode-select --install
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
npm install -g pnpm@10
pnpm install
pnpm tauri dev
```

## 3. Build installer Windows

Kalau ingin membuat installer, bukan menjalankan dev server:

```powershell
pnpm install
pnpm version:check
powershell -ExecutionPolicy Bypass -File scripts/package.ps1
```

Hasilnya ada di folder `artifacts/` (NSIS `.exe` dan MSI `.msi`) beserta `SHA256SUMS.txt`. Detail signing dan packaging ada di `PACKAGING.md`.

## 4. Pertama kali buka aplikasi

1. **Settings → Add provider**
   - OpenAI-compatible: base URL misalnya `https://api.openai.com/v1`.
   - Anthropic: base URL `https://api.anthropic.com`.
   - Isi model dan API key. Key disimpan di OS keychain (Windows Credential Manager, macOS Keychain, Linux Secret Service), tidak pernah ditulis ke SQLite.
2. **MCP servers → Add server** kalau mau tools. URL harus HTTPS (localhost boleh HTTP). Klik **Test connection**.
3. **New chat**, kirim pesan. Untuk tool call pertama, akan muncul kartu approval.

Tidak perlu menaruh API key di file mana pun. Jangan commit `.env` atau key.

## 5. Tes tanpa API key (mock)

Di terminal terpisah, di folder repo:

```bash
cargo run -p mcp-mock
```

Lalu di aplikasi:

- Provider: base URL `http://127.0.0.1:8788/v1`, model `mock-tool`, key boleh kosong (localhost).
- MCP server: `http://127.0.0.1:8787/mcp`, policy `ask`.
- Kirim “Use the echo tool to say hello”.

Mock juga menyediakan `mock-text`, `mock-parallel`, `mock-cut`, `mock-overflow`, dll. Lihat `README.md`.

## 6. Kalau error

- **`error while loading shared libraries: libwebkit2gtk`** → paket dev/runtime WebKit belum lengkap. Ulangi perintah apt di atas.
- **`cannot open display`** → PC ini tidak punya sesi grafis. Jalankan dari terminal di desktop, bukan SSH tanpa X.
- **`cargo: command not found`** → buka terminal baru setelah install Rust, atau `source "$HOME/.cargo/env"`.
- **`pnpm: command not found`** → install pnpm dan buka terminal baru.
- **Port 1420 dipakai** → tutup proses Vite lain.
- **Keychain error di Linux** → install/enable `gnome-keyring` atau gunakan provider localhost tanpa key.

Kalau tetap gagal, kirim output command yang gagal (tanpa API key) ke saya.
