#!/usr/bin/env bash
# Install web dependencies only when the locked dependency inputs changed.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
web_dir="$repo_root/web"
stamp="$web_dir/node_modules/.stocks-bun-install-inputs.sha256"

mapfile -t inputs < <(
    find "$repo_root" \
        \( -path "$repo_root/.git" -o -path "$repo_root/target" -o -path "$repo_root/web/node_modules" \) -prune \
        -o \( -name package.json -o -name bun.lock -o -name bun.lockb -o -name bunfig.toml \) -type f -print \
        | sort
)

if [[ "${#inputs[@]}" -eq 0 ]]; then
    echo "web-preflight: no package manifests or Bun locks found"
    exit 1
fi

input_hash="$(
    for file in "${inputs[@]}"; do
        rel="${file#"$repo_root/"}"
        printf '%s  %s\n' "$(sha256sum "$file" | awk '{print $1}')" "$rel"
    done | sha256sum | awk '{print $1}'
)"

if [[ ! -d "$web_dir/node_modules" || ! -f "$stamp" || "$(cat "$stamp")" != "$input_hash" ]]; then
    bun install --cwd "$web_dir" --frozen-lockfile --ignore-scripts
    printf '%s\n' "$input_hash" >"$stamp"
else
    echo "web-preflight: dependencies unchanged; skipping bun install"
fi
