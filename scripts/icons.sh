#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
icon_dir=apps/desktop/src-tauri/icons
generated_dir="$(mktemp -d)"
trap 'rm -rf "$generated_dir"' EXIT
node_modules/.bin/tauri icon "$icon_dir/source.svg" --output "$generated_dir"
cp "$generated_dir/icon.png" "$icon_dir/icon.png"
cp "$generated_dir/icon.icns" "$icon_dir/icon.icns"
cp "$generated_dir/icon.ico" "$icon_dir/icon.ico"
