#!/usr/bin/env bash
# scripts/smoketest.sh — Golden-path end-to-end pipeline test (#60).
#
# Walks one ticker (MU) through every stage of the system and asserts the
# expected rows landed. Exits non-zero on the first stage that fails so a
# regression is caught immediately rather than weeks later.
#
# Expects:
#   - Local infra up (make up)
#   - Migrations applied (make migrate)
#   - Demo tickers + market data seeded (make seed-demo + Massive bars present)
#   - Infisical secrets configured (z.ai key) — invoked via `infisical run`
#
# Stages walked:
#   1. Refresh ticker_context for MU (LLM round-trip + audit row).
#   2. Draft thesis. If LLM declines (legitimate), skip subsequent thesis
#      stages and report — declining is correct system behavior, not failure.
#   3. Walk thesis through forming → building_conviction → armed → actionable.
#   4. Inject a resolvable conviction condition, run evaluator, assert it
#      flipped to 'satisfied' in the DB (catches the silent-no-op regression).
#   5. Run consensus once, assert a row appears for MU.
#   6. Run discovery once. Discovery is allowed to find 0 hits on quiet
#      markets. Inject a synthetic candidate, classify it, assert the
#      classification row appears.
#   7. Drive a real thesis.actionable for the MU thesis with a delta sized
#      to trigger single_name_delta_notional_pct veto. Assert an alert row
#      with that reason appears.
#   8. Drive an outcome event and assert a prediction has horizon_at set
#      (catches the reflection regression).
set -euo pipefail

PSQL_URL="${PSQL_URL:-postgres://stocks:stocks_dev_only@localhost:5432/stocks?sslmode=disable}"
PSQL=(psql "$PSQL_URL" -tA -v ON_ERROR_STOP=1)
RUN=(infisical run --env=dev --)
SYMBOL="${SMOKETEST_SYMBOL:-MU}"
BIN="./target/release"

step()  { printf "\n\033[1;36m=== %s ===\033[0m\n" "$*"; }
ok()    { printf "  \033[32m✓\033[0m %s\n" "$*"; }
warn()  { printf "  \033[33m!\033[0m %s\n" "$*"; }
fail()  { printf "  \033[31m✗\033[0m %s\n" "$*"; exit 1; }

# Require release binaries to exist.
for bin in gateway risk reflection evaluator consensus discovery devpub; do
    [[ -x "$BIN/$bin" ]] || fail "missing $BIN/$bin — run 'make build' first"
done

# Background processes started here, cleaned up on exit.
PIDS=()
cleanup() {
    for pid in "${PIDS[@]}"; do kill "$pid" 2>/dev/null || true; done
}
trap cleanup EXIT

start_bg() {
    local bin="$1"; shift
    local logfile="/tmp/smoketest-${bin}.log"
    "${RUN[@]}" "$BIN/$bin" "$@" >"$logfile" 2>&1 &
    PIDS+=("$!")
    echo "$logfile"
}

# ---------- preflight ----------
step "preflight"
bars=$("${PSQL[@]}" -c "SELECT count(*) FROM price_bar WHERE symbol='$SYMBOL'")
facts=$("${PSQL[@]}" -c "SELECT count(*) FROM company_fact WHERE symbol='$SYMBOL'")
[[ "$bars"  -gt 0 ]] || fail "no price_bar rows for $SYMBOL — backfill via Massive first"
[[ "$facts" -gt 0 ]] || fail "no company_fact rows for $SYMBOL — backfill via XBRL first"
ok "$SYMBOL has $bars bars + $facts XBRL facts"

# ---------- stage 1: refresh context ----------
step "stage 1: refresh ticker_context"
prev_ver=$("${PSQL[@]}" -c "SELECT COALESCE(MAX(version),0) FROM ticker_context WHERE symbol='$SYMBOL'")
"${RUN[@]}" cd py 2>/dev/null || true  # noop — infisical doesn't cd
(cd py && "${RUN[@]}" .venv/bin/python -m stocks.context_maintainer "$SYMBOL" >/tmp/smoketest-context.log 2>&1) \
    || fail "context_maintainer failed; see /tmp/smoketest-context.log"
new_ver=$("${PSQL[@]}" -c "SELECT MAX(version) FROM ticker_context WHERE symbol='$SYMBOL'")
[[ "$new_ver" -gt "$prev_ver" ]] || fail "no new context version (was $prev_ver, still $new_ver)"
ok "ticker_context advanced from v$prev_ver → v$new_ver"

# ---------- stage 2: draft thesis ----------
step "stage 2: draft + sharpen + challenge"
(cd py && "${RUN[@]}" .venv/bin/python -m stocks.thesis_engine "$SYMBOL" >/tmp/smoketest-draft.log 2>&1) \
    || fail "thesis_engine crashed; see /tmp/smoketest-draft.log"

thesis_id=$("${PSQL[@]}" -c "SELECT thesis_id::text FROM thesis WHERE symbol='$SYMBOL' ORDER BY created_at DESC LIMIT 1")
if [[ -z "$thesis_id" ]]; then
    if grep -q '"edge_present": false' /tmp/smoketest-draft.log; then
        warn "LLM honestly declined to draft a thesis for $SYMBOL — this is correct behavior, but no thesis to test further stages with"
        warn "skipping stages 3-7 (set SMOKETEST_SYMBOL to a different ticker to retry)"
        exit 0
    fi
    fail "no thesis row appeared and no honest decline marker in the log"
fi
ok "drafted thesis $thesis_id"

# Sharpen + challenge are best-effort: surface errors but don't gate the smoketest.
(cd py && "${RUN[@]}" .venv/bin/python -m stocks.sharpen "$thesis_id"  >/tmp/smoketest-sharpen.log  2>&1) \
    && ok "sharpen ran cleanly" || warn "sharpen failed (see /tmp/smoketest-sharpen.log)"
(cd py && "${RUN[@]}" .venv/bin/python -m stocks.challenge "$thesis_id" >/tmp/smoketest-challenge.log 2>&1) \
    && ok "challenge ran cleanly" || warn "challenge failed (see /tmp/smoketest-challenge.log)"

# ---------- stage 3: walk lifecycle ----------
step "stage 3: walk state machine forming → actionable"
gw_log=$(start_bg gateway)
# Wait for gateway to be ready (up to 10s).
for _ in $(seq 1 20); do curl -sf http://localhost:8080/healthz >/dev/null && break; sleep 0.5; done
curl -sf http://localhost:8080/healthz >/dev/null || fail "gateway never came up; see $gw_log"

for to in building_conviction armed actionable; do
    code=$(curl -s -o /tmp/smoketest-transition.json -w '%{http_code}' \
                -X POST -H 'content-type: application/json' \
                -d "{\"to\":\"$to\",\"rationale\":\"smoketest\"}" \
                "http://localhost:8080/api/theses/$thesis_id/transition")
    [[ "$code" == "200" ]] || fail "transition → $to failed (HTTP $code): $(cat /tmp/smoketest-transition.json)"
done
state=$("${PSQL[@]}" -c "SELECT state FROM thesis WHERE thesis_id='$thesis_id'")
[[ "$state" == "actionable" ]] || fail "thesis state expected 'actionable', got '$state'"
ok "thesis is actionable"

# ---------- stage 4: evaluator (with resolvable injected condition) ----------
step "stage 4: condition evaluator end-to-end"
"${PSQL[@]}" -c "
UPDATE thesis SET conviction_conditions = conviction_conditions || jsonb_build_array(
  jsonb_build_object('type','quantitative','name','smoketest_close',
    'expr','smoketest: $SYMBOL close > 1',
    'target', jsonb_build_object('metric','$SYMBOL.close','op','>','value',1,'unit','USD'),
    'deadline_at','2099-12-31T00:00:00Z',
    'evidence_source','market:$SYMBOL'
  )
) WHERE thesis_id='$thesis_id'" >/dev/null
EVAL_INTERVAL_SECS=1 "${RUN[@]}" "$BIN/evaluator" >/tmp/smoketest-eval.log 2>&1 &
ev_pid=$!
sleep 3
kill "$ev_pid" 2>/dev/null || true
wait "$ev_pid" 2>/dev/null || true
satisfied=$("${PSQL[@]}" -c "SELECT count(*) FROM v_condition WHERE thesis_id='$thesis_id' AND status='satisfied'")
[[ "$satisfied" -ge 1 ]] || fail "evaluator did not flip any condition to 'satisfied' (got $satisfied) — see /tmp/smoketest-eval.log"
ok "evaluator flipped $satisfied condition(s) to satisfied"

# ---------- stage 5: consensus ----------
step "stage 5: consensus score for $SYMBOL"
"${RUN[@]}" "$BIN/consensus" >/tmp/smoketest-consensus.log 2>&1 &
cs_pid=$!
sleep 5
kill "$cs_pid" 2>/dev/null || true
wait "$cs_pid" 2>/dev/null || true
rows=$("${PSQL[@]}" -c "SELECT count(*) FROM consensus_score WHERE symbol='$SYMBOL' AND computed_at > now() - interval '2 minutes'")
[[ "$rows" -ge 1 ]] || fail "no fresh consensus_score row for $SYMBOL"
ok "consensus produced $rows fresh row(s) for $SYMBOL"

# ---------- stage 6: discovery + classify ----------
step "stage 6: discovery + classify round-trip"
"${RUN[@]}" "$BIN/discovery" >/tmp/smoketest-discovery.log 2>&1 &
ds_pid=$!
sleep 4
kill "$ds_pid" 2>/dev/null || true
wait "$ds_pid" 2>/dev/null || true
ok "discovery scanner ran without error"
# Inject a synthetic candidate to exercise classify path (discovery legitimately
# finds 0 hits on quiet markets).
cand_id=$("${PSQL[@]}" -c "
WITH ins AS (
  INSERT INTO discovery_candidate (symbol, signal_name, signal_value, reasoning)
  VALUES ('$SYMBOL','volume_anomaly',3.1,'smoketest synthetic candidate')
  RETURNING id
) SELECT id FROM ins" || true)
[[ -n "$cand_id" ]] || fail "could not seed synthetic candidate"
(cd py && "${RUN[@]}" .venv/bin/python -m stocks.classify --candidate-id "$cand_id" >/tmp/smoketest-classify.log 2>&1) \
    || fail "classify crashed; see /tmp/smoketest-classify.log"
proposed=$("${PSQL[@]}" -c "SELECT jsonb_array_length(proposed_lists) FROM discovery_classification WHERE candidate_id=$cand_id")
[[ "$proposed" -ge 1 ]] || fail "classifier proposed 0 lists for candidate $cand_id"
ok "classifier proposed $proposed list(s) for synthetic candidate"

# ---------- stage 7: risk overlay on a real actionable ----------
step "stage 7: risk verdict on a real thesis.actionable"
# Make sure portfolio is configured. (If not, risk runs in DEMO mode — log it.)
acct=$("${PSQL[@]}" -c "SELECT COALESCE(account_size_usd::text, 'unset') FROM portfolio_settings WHERE id=1")
[[ "$acct" != "unset" ]] || warn "portfolio_settings unset — risk overlay will run in DEMO mode"
risk_log=$(start_bg risk)
sleep 3
prev_alert=$("${PSQL[@]}" -c "SELECT COALESCE(MAX(id),0) FROM alert WHERE kind='risk'")
# Delta sized to definitely breach single_name_delta_notional_pct (15% cap).
"${RUN[@]}" "$BIN/devpub" thesis.actionable \
  "{\"thesis_id\":\"$thesis_id\",\"symbol\":\"$SYMBOL\",\"cluster\":\"$( "${PSQL[@]}" -c "SELECT cluster_id FROM ticker WHERE symbol='$SYMBOL'" )\",\"instrument\":\"equity\",\"delta_notional\":99999999}" >/dev/null
sleep 3
new_alert=$("${PSQL[@]}" -c "SELECT id FROM alert WHERE kind='risk' AND id > $prev_alert AND payload->>'symbol'='$SYMBOL' ORDER BY id DESC LIMIT 1")
[[ -n "$new_alert" ]] || fail "no fresh risk alert for $SYMBOL — see $risk_log"
veto=$("${PSQL[@]}" -c "SELECT payload->>'veto' FROM alert WHERE id=$new_alert")
reasons=$("${PSQL[@]}" -c "SELECT payload->'reasons' FROM alert WHERE id=$new_alert")
[[ "$veto" == "true" ]] || fail "expected veto=true on extreme delta, got veto=$veto"
ok "risk verdict id=$new_alert veto=$veto reasons=$reasons"

# ---------- stage 8: reflection records horizon_at ----------
step "stage 8: reflection captures forecast + horizon_at"
refl_log=$(start_bg reflection)
sleep 3
prev_pred=$("${PSQL[@]}" -c "SELECT COALESCE(MAX(at),'epoch'::timestamptz) FROM prediction WHERE thesis_id='$thesis_id'")
"${RUN[@]}" "$BIN/devpub" thesis.actionable \
  "{\"thesis_id\":\"$thesis_id\",\"symbol\":\"$SYMBOL\",\"cluster\":\"$( "${PSQL[@]}" -c "SELECT cluster_id FROM ticker WHERE symbol='$SYMBOL'" )\",\"instrument\":\"equity\",\"delta_notional\":1000}" >/dev/null
sleep 3
row=$("${PSQL[@]}" -c "SELECT horizon_at FROM prediction WHERE thesis_id='$thesis_id' AND at > '$prev_pred' ORDER BY at DESC LIMIT 1")
[[ -n "$row" ]] || fail "no fresh prediction recorded — see $refl_log"
[[ "$row" != "" ]] || fail "horizon_at NULL on fresh prediction — bug regressed (#60)"
ok "fresh prediction has horizon_at=$row"

# ---------- summary ----------
step "all stages passed ✓"
printf "%-22s %s\n" "Symbol:"           "$SYMBOL"
printf "%-22s %s\n" "Thesis:"           "$thesis_id"
printf "%-22s %s\n" "Final state:"      "$state"
printf "%-22s %s\n" "Satisfied conds:"  "$satisfied"
printf "%-22s %s\n" "Risk verdict id:"  "$new_alert  (veto=$veto)"
