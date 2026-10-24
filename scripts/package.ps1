param(
  [switch]$DebugBuild
)

$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..")

foreach ($command in @("pnpm", "cargo")) {
  if (-not (Get-Command $command -ErrorAction SilentlyContinue)) {
    throw "Missing $command. Install Node 22, pnpm 10, and Rust from https://rustup.rs first."
  }
}

pnpm version:check
pnpm install --frozen-lockfile

if ($DebugBuild) {
  pnpm tauri build --debug
  $buildProfile = "debug"
} else {
  pnpm tauri build
  $buildProfile = "release"
}

$roots = @(
  (Join-Path $PWD "target\$buildProfile\bundle"),
  (Join-Path $PWD "src-tauri\target\$buildProfile\bundle")
) | Where-Object { Test-Path $_ }

$files = @()
foreach ($root in $roots) {
  $files += Get-ChildItem -Path $root -Recurse -File |
    Where-Object { $_.Extension -in ".exe", ".msi" }
}

if ($files.Count -eq 0) {
  throw "Build finished, but no installer was found under target\$buildProfile\bundle."
}

$out = Join-Path $PWD "artifacts"
New-Item -ItemType Directory -Path $out -Force | Out-Null
$lines = @()
foreach ($file in $files) {
  $destination = Join-Path $out $file.Name
  Copy-Item $file.FullName $destination -Force
  $hash = (Get-FileHash $destination -Algorithm SHA256).Hash.ToLowerInvariant()
  $lines += "$hash  $($file.Name)"
  Write-Host "Installer: $destination"
}

$checksums = Join-Path $out "SHA256SUMS.txt"
$lines | Set-Content -Path $checksums -Encoding ascii
Write-Host ""
Write-Host "Checksums written to $checksums"
