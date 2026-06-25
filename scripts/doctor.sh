#!/usr/bin/env bash
# Local dev-stack health check. Writes diagnostics only under repo-local,
# gitignored runtime/cache directories.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
psql_url="${PSQL_URL:-postgres://stocks:stocks_dev_only@localhost:5432/stocks?sslmode=disable}"
gateway_url="${GATEWAY_URL:-http://localhost:8080}"
runtime_dir="${STOCKS_RUNTIME_DIR:-$repo_root/.runtime}"
playwright_workdir="${PLAYWRIGHT_WORKDIR:-$repo_root/web/.cache/playwright-work}"
bun_cache_dir="${BUN_INSTALL_CACHE_DIR:-$repo_root/web/.cache/bun-install}"

failures=0
warnings=0

mkdir -p "$runtime_dir" "$playwright_workdir" "$bun_cache_dir"

ok() {
    printf "OK   %s\n" "$*"
}

warn_msg() {
    warnings=$((warnings + 1))
    printf "WARN %s\n" "$*"
}

fail_msg() {
    failures=$((failures + 1))
    printf "FAIL %s\n" "$*"
}

bytes_human() {
    awk -v kb="$1" 'BEGIN {
        gb = kb / 1024 / 1024;
        if (gb >= 1) {
            printf "%.1f GiB", gb;
        } else {
            printf "%.0f MiB", kb / 1024;
        }
    }'
}

check_fs() {
    local path="$1"
    local label="$2"
    local row
    if ! row="$(df -Pk "$path" 2>/dev/null | awk 'NR == 2 {print $4 " " $5 " " $6}')"; then
        warn_msg "could not read disk usage for $label ($path)"
        return
    fi

    local avail_kb used_pct mount
    read -r avail_kb used_pct mount <<<"$row"
    local avail
    avail="$(bytes_human "$avail_kb")"

    if (( avail_kb < 2 * 1024 * 1024 )); then
        fail_msg "$label has only $avail free ($used_pct used at $mount)"
    elif (( avail_kb < 5 * 1024 * 1024 )); then
        warn_msg "$label has $avail free ($used_pct used at $mount)"
    else
        ok "$label has $avail free ($used_pct used at $mount)"
    fi
}

check_docker() {
    if ! command -v docker >/dev/null 2>&1; then
        warn_msg "docker is not on PATH"
        return
    fi
    if ! docker info >/dev/null 2>&1; then
        fail_msg "docker daemon is not reachable"
        return
    fi
    ok "docker daemon reachable"

    printf "\nDocker disk usage:\n"
    if docker system df --format '  {{.Type}}: size={{.Size}} reclaimable={{.Reclaimable}}' 2>/dev/null; then
        :
    else
        docker system df 2>/dev/null | sed 's/^/  /' || warn_msg "could not read docker system df"
    fi
    local build_reclaimable
    build_reclaimable="$(
        docker system df --format '{{.Type}}\t{{.Reclaimable}}' 2>/dev/null \
            | awk -F'\t' '$1 == "Build Cache" {print $2; exit}'
    )"
    if [[ -n "$build_reclaimable" ]]; then
        if awk -v size="$build_reclaimable" 'BEGIN {
            n = size + 0;
            if (size ~ /TB/) exit 0;
            if (size ~ /GB/ && n >= 50) exit 0;
            exit 1;
        }'; then
            warn_msg "Docker build cache has $build_reclaimable reclaimable; run docker builder prune if root space gets tight"
        else
            ok "Docker build cache reclaimable: $build_reclaimable"
        fi
    fi

    local compose=(docker compose -f "$repo_root/deploy/local/docker-compose.yml")
    if "${compose[@]}" ps postgres >/dev/null 2>&1; then
        printf "\nPostgres compose status:\n"
        "${compose[@]}" ps postgres | sed 's/^/  /'
    else
        warn_msg "could not read docker compose postgres status"
    fi
}

check_postgres() {
    if command -v pg_isready >/dev/null 2>&1; then
        if pg_isready -d "$psql_url" >/dev/null 2>&1; then
            ok "pg_isready can reach Postgres"
        else
            fail_msg "pg_isready cannot reach Postgres at $psql_url"
        fi
    else
        warn_msg "pg_isready is not installed"
    fi

    if command -v psql >/dev/null 2>&1; then
        local db_row
        if db_row="$(psql "$psql_url" -v ON_ERROR_STOP=1 -Atqc "select current_database() || ' checked at ' || now()" 2>&1)"; then
            ok "psql query succeeded: $db_row"
        else
            fail_msg "psql query failed: $db_row"
        fi
    else
        warn_msg "psql is not installed"
    fi
}

check_gateway() {
    if ! command -v curl >/dev/null 2>&1; then
        warn_msg "curl is not installed; skipping gateway probe"
        return
    fi

    local status_file="$runtime_dir/system-status.json"
    if curl -fsS "$gateway_url/api/system-status" -o "$status_file" 2>/dev/null; then
        if command -v jq >/dev/null 2>&1; then
            local db_status db_reason
            db_status="$(jq -r '.database.status // "missing"' "$status_file")"
            db_reason="$(jq -r '.database.reason // ""' "$status_file")"
            if [[ "$db_status" == "ok" ]]; then
                ok "gateway /api/system-status reports database ok"
            elif [[ "$db_status" == "missing" ]]; then
                warn_msg "gateway /api/system-status has no database status field"
            else
                fail_msg "gateway /api/system-status reports database $db_status: $db_reason"
            fi
        elif grep -q '"database"' "$status_file"; then
            ok "gateway /api/system-status returned a database status field"
        else
            warn_msg "gateway /api/system-status returned no database status field"
        fi
    elif curl -fsS "$gateway_url/healthz" >/dev/null 2>&1; then
        fail_msg "gateway healthz is reachable but /api/system-status failed"
    else
        warn_msg "gateway is not reachable at $gateway_url"
    fi
}

printf "Stocks dev doctor\n"
printf "repo: %s\n" "$repo_root"
printf "runtime dir: %s\n" "$runtime_dir"
printf "Playwright workdir: %s\n" "$playwright_workdir"
printf "Bun cache dir: %s\n\n" "$bun_cache_dir"

check_fs "/" "root filesystem"
check_fs "$repo_root" "repo filesystem"
printf "\n"
check_docker
printf "\n"
check_postgres
printf "\n"
check_gateway

cat <<EOF

Recovery commands:
  docker system df
  docker builder prune
  docker image prune
  docker compose -f deploy/local/docker-compose.yml restart postgres
  make web-install BUN_INSTALL_CACHE_DIR="$bun_cache_dir"
  make web-e2e PLAYWRIGHT_WORKDIR="$playwright_workdir"
EOF

if (( failures > 0 )); then
    printf "\nFAIL %d failure(s), %d warning(s)\n" "$failures" "$warnings"
    exit 1
fi

printf "\nOK   0 failures, %d warning(s)\n" "$warnings"
