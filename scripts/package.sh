#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

for command in pnpm cargo; do
  if ! command -v "$command" >/dev/null 2>&1; then
    echo "Missing $command. Install Node 22, pnpm 10, Rust, and the Tauri prerequisites first." >&2
    exit 1
  fi
done

profile=release
build_args=()
if [[ "${1:-}" == "--debug" ]]; then
  profile=debug
  build_args+=(--debug)
fi

pnpm version:check
pnpm install --frozen-lockfile
pnpm tauri build "${build_args[@]}"

mapfile -t bundle_roots < <(
  find target src-tauri/target -maxdepth 3 -type d -path "*/$profile/bundle" -print 2>/dev/null
)

mapfile -t files < <(
  for root in "${bundle_roots[@]}"; do
    find "$root" -type f \( -name '*.deb' -o -name '*.rpm' -o -name '*.AppImage' -o -name '*.dmg' \) -print
  done
)

if [[ ${#files[@]} -eq 0 ]]; then
  echo "Build finished, but no installer was found under target/$profile/bundle." >&2
  exit 1
fi

mkdir -p artifacts
: > artifacts/SHA256SUMS.txt
for file in "${files[@]}"; do
  cp "$file" artifacts/
  name=$(basename "$file")
  hash=$(sha256sum "artifacts/$name" | awk '{print $1}')
  printf '%s  %s\n' "$hash" "$name" >> artifacts/SHA256SUMS.txt
  echo "Installer: artifacts/$name"
done

echo
echo "Checksums written to artifacts/SHA256SUMS.txt"
