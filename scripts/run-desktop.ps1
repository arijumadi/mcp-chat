$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..")

if (-not (Get-Command pnpm -ErrorAction SilentlyContinue)) {
  throw "pnpm is not installed. Run: npm install -g pnpm@10"
}
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
  throw "Rust/cargo is not installed. Install it from https://rustup.rs"
}

pnpm install
pnpm tauri dev
