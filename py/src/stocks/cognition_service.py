"""Cognition consumer (#100).

Subscribes to `discovery.confirmed` and runs context_maintainer → thesis_engine
→ sharpen → challenge for the symbol the operator just promoted. It also runs a
maintenance sweep over active tickers so the system does not depend on manual
`make refresh-context SYMBOL=X` or opening a UI tab. The sweep reacts to stale
views, provider source-task completions, and newly available normalized
`evidence_item` facts.

Honest decline path: thesis_engine may return edge_present=false. We still
persist the context refresh (already valuable) and emit a single 'no_thesis'
attention item (severity=info) so the operator sees the system tried.

Usage:
    python -m stocks.cognition_service

Reads:
    NATS_URL                — default nats://localhost:4222
    STREAM_MARKET           — discovery.* subjects are under MARKET stream
    DURABLE                 — "cognition-consumer"
    COGNITION_SWEEP_SECONDS — default 300; set 0 to disable maintenance sweep
    COGNITION_CONTEXT_MAX_AGE_HOURS — default 12
    COGNITION_OPEN_THESIS_MAX_AGE_MINUTES — default 30
    COGNITION_DECLINE_RETRY_HOURS — default 6
    COGNITION_MAX_SYMBOLS_PER_SWEEP — default 20
    COGNITION_MIN_SYMBOLS_PER_SWEEP — default 20
    COGNITION_SWEEP_CONCURRENCY — default 2
    COGNITION_EVIDENCE_SYNC_LIMIT — default 200
    BRAIN_THESIS_SWEEP_LIMIT — default 50
    COGNITION_ACK_PROGRESS_SECONDS — default 10
    SOURCE_TASK_SWEEP_SECONDS — default 60; set 0 to disable due task worker
    SOURCE_TASK_MAX_SYMBOLS_PER_SWEEP — default 5
"""

from __future__ import annotations

import asyncio
import datetime as dt
import json
import logging
import os
from collections.abc import Awaitable, Callable

import asyncpg
import nats
from nats.errors import TimeoutError as NatsTimeout
from nats.js.errors import NotFoundError

from . import config
from .brain_maintainer import refresh as refresh_brain_theses
from .challenge import challenge as challenge_thesis
from .context_maintainer import BlockingEvidenceMissing
from .context_maintainer import refresh as refresh_context
from .evidence import (
    load_open_evidence_requirements,
    refresh_open_evidence_requirements,
    sync_llm_missing_evidence,
)
from .sharpen import sharpen as sharpen_thesis
from .source_tasks import loop as source_task_loop
from .thesis_engine import draft as draft_thesis

log = logging.getLogger("cognition")

STREAM = "MARKET"
SUBJECT = "discovery.confirmed"
DURABLE = "cognition-consumer"
_IN_FLIGHT_SYMBOLS: set[str] = set()
BOOTSTRAP_SWEEP_REASONS = {
    "context_missing",
    "context_missing_market",
    "evidence_state_missing",
    "evidence_retry_due",
    "evidence_satisfied_retry",
    "thesis_retry_due",
}
SOURCE_TASK_DELTA_SWEEP_REASONS = {
    "source_task_changed",
    "source_task_changed_retry",
}
EVIDENCE_DELTA_SWEEP_REASONS = {
    "evidence_item_changed",
    "evidence_item_changed_retry",
}


def _env_int(name: str, default: int) -> int:
    raw = os.getenv(name)
    if raw is None or raw == "":
        return default
    try:
        return int(raw)
    except ValueError:
        log.warning("invalid %s=%r; using %d", name, raw, default)
        return default


def _open_thesis_max_age_minutes() -> int:
    return max(1, _env_int("COGNITION_OPEN_THESIS_MAX_AGE_MINUTES", 30))


def _effective_sweep_limit() -> int:
    configured = max(1, _env_int("COGNITION_MAX_SYMBOLS_PER_SWEEP", 20))
    floor = max(1, _env_int("COGNITION_MIN_SYMBOLS_PER_SWEEP", 20))
    if configured < floor:
        log.warning(
            "COGNITION_MAX_SYMBOLS_PER_SWEEP=%s is below SLA floor %s; using %s",
            configured,
            floor,
            floor,
        )
        return floor
    return configured


def _bootstrap_sweep_floor(limit: int) -> int:
    configured = max(1, _env_int("COGNITION_BOOTSTRAP_SYMBOLS_PER_SWEEP", 5))
    return min(limit, configured)


def _sweep_concurrency() -> int:
    return max(1, _env_int("COGNITION_SWEEP_CONCURRENCY", 2))


def _effective_sweep_interval_seconds() -> int:
    configured = _env_int("COGNITION_SWEEP_SECONDS", 300)
    if configured <= 0:
        return configured
    max_age_minutes = _open_thesis_max_age_minutes()
    interval_cap = max(60, min(300, (max_age_minutes * 60) // 2))
    if configured > interval_cap:
        log.warning(
            "COGNITION_SWEEP_SECONDS=%s exceeds thesis freshness cap %s; using %s",
            configured,
            interval_cap,
            interval_cap,
        )
        return interval_cap
    return configured


async def _open_thesis_count(pool: asyncpg.Pool, symbol: str) -> int:
    return int(await pool.fetchval(
        """SELECT count(*)
             FROM thesis
            WHERE symbol = $1
              AND state NOT IN ('closed', 'disqualified')""",
        symbol,
    ) or 0)


def _json_default(value):
    if hasattr(value, "isoformat"):
        return value.isoformat()
    return str(value)


async def _start_cognition_run(
    pool: asyncpg.Pool,
    symbol: str,
    source_ref: dict | None,
) -> int:
    ref = dict(source_ref or {})
    trigger = str(ref.get("trigger") or "manual")
    sweep_reason = ref.get("sweep_reason")
    return int(await pool.fetchval(
        """INSERT INTO cognition_run
             (symbol, trigger, sweep_reason, status, reason, source_ref)
           VALUES ($1, $2, $3, 'running', $4, $5::jsonb)
        RETURNING id""",
        symbol,
        trigger,
        str(sweep_reason) if sweep_reason else None,
        str(ref.get("reason") or trigger),
        json.dumps(ref, default=_json_default),
    ))


def _parse_dt(value) -> dt.datetime | None:
    if value is None:
        return None
    if isinstance(value, dt.datetime):
        return value
    if isinstance(value, str):
        try:
            return dt.datetime.fromisoformat(value.replace("Z", "+00:00"))
        except ValueError:
            return None
    return None


def _next_retry_from_evidence(evidence: list[dict]) -> dt.datetime | None:
    dates = [
        parsed
        for parsed in (_parse_dt(row.get("next_retry_at")) for row in evidence)
        if parsed is not None
    ]
    return min(dates) if dates else None


def _status_for_thesis_result(result: dict | None) -> tuple[str, str | None]:
    if result and result.get("_thesis_id"):
        classification = result.get("_reconciliation_classification")
        if result.get("_reconciled_existing_thesis"):
            if classification == "no_change":
                return "no_change", classification
            return "reconciled", classification
        return "drafted", classification
    return "declined", None


async def _finish_cognition_run(
    pool: asyncpg.Pool,
    run_id: int,
    *,
    status: str,
    reason: str | None,
    context_version: int | None,
    thesis_id: str | None,
    thesis_classification: str | None,
    evidence: list[dict],
    error: str | None = None,
    source_ref: dict | None = None,
) -> None:
    blocking = [row for row in evidence if row.get("priority") == "blocking"]
    await pool.execute(
        """UPDATE cognition_run
              SET status = $2,
                  reason = $3,
                  context_version = $4,
                  thesis_id = $5::uuid,
                  thesis_classification = $6,
                  evidence_open_count = $7,
                  evidence_blocking_count = $8,
                  finished_at = now(),
                  next_retry_at = $9,
                  error = $10,
                  source_ref = source_ref || $11::jsonb
            WHERE id = $1""",
        run_id,
        status,
        reason,
        context_version,
        thesis_id,
        thesis_classification,
        len(evidence),
        len(blocking),
        _next_retry_from_evidence(evidence),
        error,
        json.dumps(source_ref or {}, default=_json_default),
    )


async def _reclaim_running_cognition_runs(
    pool: asyncpg.Pool,
    reason: str = "orphaned_by_cognition_startup",
) -> int:
    count = await pool.fetchval(
        """WITH updated AS (
               UPDATE cognition_run
                  SET status = 'failed',
                      reason = $1::text,
                      finished_at = now(),
                      error = COALESCE(error, $2::text),
                      source_ref = source_ref || jsonb_build_object(
                          'reclaimed_by', 'cognition_service',
                          'reclaimed_at', now(),
                          'reclaim_reason', $1::text
                      )
                WHERE status = 'running'
            RETURNING 1
           )
           SELECT count(*) FROM updated""",
        reason,
        "cognition service reclaimed an orphaned running run",
    )
    return int(count or 0)


async def _reclaim_stale_cognition_runs(
    pool: asyncpg.Pool,
    *,
    max_age_minutes: int,
    reason: str = "stale_running_reclaim",
) -> int:
    count = await pool.fetchval(
        """WITH updated AS (
               UPDATE cognition_run
                  SET status = 'failed',
                      reason = $2::text,
                      finished_at = now(),
                      error = COALESCE(error, $3::text),
                      source_ref = source_ref || jsonb_build_object(
                          'reclaimed_by', 'cognition_service',
                          'reclaimed_at', now(),
                          'reclaim_reason', $2::text,
                          'max_age_minutes', $1::int
                      )
                WHERE status = 'running'
                  AND started_at < now() - make_interval(mins => $1::int)
            RETURNING 1
           )
           SELECT count(*) FROM updated""",
        max(1, max_age_minutes),
        reason,
        "cognition service reclaimed a stale running run",
    )
    return int(count or 0)


async def _record_decline(
    pool: asyncpg.Pool,
    symbol: str,
    candidate_id: int | None,
    reason: str | None,
    source_ref: dict,
    extra_missing_evidence: list[dict] | None = None,
) -> None:
    evidence = await load_open_evidence_requirements(pool, symbol)
    full_ref = dict(source_ref)
    full_ref["missing_evidence"] = evidence
    if extra_missing_evidence:
        full_ref["llm_missing_evidence"] = extra_missing_evidence
    fsm_state, owner, state_reason = _decline_attention_assignment(
        evidence,
        extra_missing_evidence,
    )
    await pool.execute(
        """WITH updated AS (
               UPDATE attention_item
                  SET reason = $4,
                      source_ref = $5::jsonb,
                      fsm_state = $6,
                      owner = $7,
                      state_reason = $8
                WHERE status = 'open'
                  AND kind = 'thesis_incomplete'
                  AND symbol = $1
              RETURNING id
           )
           INSERT INTO attention_item
             (kind, symbol, candidate_id, severity, title, reason,
              source, source_ref, fsm_state, owner, state_reason)
           SELECT 'thesis_incomplete', $1, $2, 'info', $3, $4,
                  'thesis', $5::jsonb, $6, $7, $8
            WHERE NOT EXISTS (SELECT 1 FROM updated)
           ON CONFLICT DO NOTHING""",
        symbol,
        candidate_id,
        f"{symbol}: system declined to draft a thesis",
        reason,
        json.dumps(full_ref),
        fsm_state,
        owner,
        state_reason,
    )


def _decline_attention_assignment(
    evidence: list[dict],
    extra_missing_evidence: list[dict] | None = None,
) -> tuple[str, str, str]:
    has_missing_evidence = bool(evidence or extra_missing_evidence)
    if has_missing_evidence:
        return "waiting_on_data", "source", "missing_evidence"
    return "ready_for_review", "operator", "thesis_declined"


async def _run_pipeline(
    pool: asyncpg.Pool,
    symbol: str,
    *,
    candidate_id: int | None = None,
    source_ref: dict | None = None,
) -> None:
    symbol = symbol.upper()
    run_id = await _start_cognition_run(pool, symbol, source_ref)
    context_version: int | None = None
    thesis_id: str | None = None
    thesis_classification: str | None = None
    final_status = "failed"
    final_reason: str | None = None
    final_error: str | None = None
    open_evidence: list[dict] = []
    log.info("cognition kickoff: %s (candidate_id=%s run_id=%s)", symbol, candidate_id, run_id)

    try:
        try:
            context_version = await refresh_context(symbol)
            final_status = "context_refreshed"
            final_reason = f"context refreshed to v{context_version}"
            log.info("cognition: %s context refreshed to v%s", symbol, context_version)
        except BlockingEvidenceMissing as e:
            open_evidence = list(e.missing)
            log.info(
                "cognition: %s waiting for blocking evidence before context: %s",
                symbol,
                [r["requirement_key"] for r in e.missing],
            )
        except Exception as e:  # noqa: BLE001
            final_error = str(e)
            final_reason = "context refresh failed"
            log.exception("cognition: context refresh failed for %s", symbol)

        if await _open_thesis_count(pool, symbol) > 0:
            log.info("cognition: %s already has an open thesis; draft will reconcile", symbol)

        open_evidence = await load_open_evidence_requirements(pool, symbol)
        blocking_evidence = [r for r in open_evidence if r["priority"] == "blocking"]
        if blocking_evidence:
            log.info("cognition: %s waiting for blocking evidence before thesis draft", symbol)
            decline_ref = dict(source_ref or {"reason": "missing_evidence"})
            decline_ref["blocking_evidence"] = blocking_evidence
            final_status = "blocked_on_evidence"
            final_reason = "Waiting for blocking evidence before drafting a thesis."
            await _record_decline(
                pool,
                symbol,
                candidate_id,
                final_reason,
                decline_ref,
            )
            return

        result = None
        try:
            result = await draft_thesis(symbol)
            final_status, thesis_classification = _status_for_thesis_result(result)
            if result and result.get("_thesis_id"):
                thesis_id = result["_thesis_id"]
                final_reason = (
                    f"thesis {final_status}: {thesis_classification}"
                    if thesis_classification
                    else f"thesis {final_status}"
                )
                log.info("cognition: %s thesis drafted/reconciled %s", symbol, thesis_id)
            else:
                log.info("cognition: %s thesis declined (no edge)", symbol)
                decline_ref = dict(source_ref or {"reason": "no_edge"})
                llm_missing_evidence = []
                if result and result.get("missing_evidence"):
                    llm_missing_evidence = result["missing_evidence"]
                    synced = await sync_llm_missing_evidence(pool, symbol, llm_missing_evidence)
                    decline_ref["synced_missing_evidence"] = [
                        r["requirement_key"] for r in synced
                    ]
                    open_evidence = await load_open_evidence_requirements(pool, symbol)
                final_reason = (result or {}).get("no_edge_reason") or "thesis declined"
                await _record_decline(
                    pool,
                    symbol,
                    candidate_id,
                    (result or {}).get("no_edge_reason"),
                    decline_ref,
                    llm_missing_evidence,
                )
        except Exception as e:  # noqa: BLE001
            final_status = "failed"
            final_error = str(e)
            final_reason = "thesis_engine failed"
            log.exception("cognition: thesis_engine failed for %s", symbol)

        if thesis_id:
            try:
                await sharpen_thesis(thesis_id)
                log.info("cognition: %s sharpen complete", symbol)
            except Exception:  # noqa: BLE001
                log.exception("cognition: sharpen failed for %s", symbol)
            try:
                await challenge_thesis(thesis_id)
                log.info("cognition: %s challenge complete", symbol)
            except Exception:  # noqa: BLE001
                log.exception("cognition: challenge failed for %s", symbol)
    finally:
        try:
            open_evidence = await load_open_evidence_requirements(pool, symbol)
        except Exception:  # noqa: BLE001
            log.warning("cognition: failed to reload evidence state for run ledger", exc_info=True)
        await _finish_cognition_run(
            pool,
            run_id,
            status=final_status,
            reason=final_reason,
            context_version=context_version,
            thesis_id=thesis_id,
            thesis_classification=thesis_classification,
            evidence=open_evidence,
            error=final_error,
            source_ref={"candidate_id": candidate_id},
        )


async def _run_symbol_once(
    symbol: str,
    run: Callable[[], Awaitable[None]],
    in_flight_symbols: set[str] | None = None,
) -> bool:
    in_flight = _IN_FLIGHT_SYMBOLS if in_flight_symbols is None else in_flight_symbols
    normalized = symbol.upper()
    if normalized in in_flight:
        log.info("cognition: %s already in flight; skipping duplicate kickoff", normalized)
        return False
    in_flight.add(normalized)
    try:
        await run()
        return True
    finally:
        in_flight.discard(normalized)


async def _on_confirmed(pool: asyncpg.Pool, msg) -> None:
    try:
        env = json.loads(msg.data.decode("utf-8"))
    except Exception as e:  # noqa: BLE001
        log.warning("malformed discovery.confirmed: %s", e)
        await msg.ack()
        return
    symbol = env.get("symbol")
    candidate_id = env.get("candidate_id")
    if not symbol:
        log.warning("discovery.confirmed missing symbol; ack-dropping")
        await msg.ack()
        return

    await _await_with_ack_progress(
        msg,
        _run_symbol_once(
            symbol,
            lambda: _run_pipeline(
                pool,
                symbol,
                candidate_id=candidate_id,
                source_ref={"reason": "no_edge", "trigger": "discovery.confirmed"},
            ),
        ),
        progress_interval_seconds=max(1, _env_int("COGNITION_ACK_PROGRESS_SECONDS", 10)),
    )
    await msg.ack()


async def _await_with_ack_progress(
    msg,
    awaitable,
    *,
    progress_interval_seconds: float,
):
    task = asyncio.create_task(awaitable)
    while True:
        done, _ = await asyncio.wait({task}, timeout=progress_interval_seconds)
        if done:
            return await task
        try:
            await msg.in_progress()
        except Exception:  # noqa: BLE001
            log.warning("cognition: failed to send JetStream in-progress ack", exc_info=True)


async def _sweep_targets(
    pool: asyncpg.Pool,
    *,
    context_max_age_hours: int,
    open_thesis_max_age_minutes: int,
    decline_retry_hours: int,
    limit: int,
) -> list[asyncpg.Record]:
    return await pool.fetch(
        """WITH latest_context AS (
               SELECT DISTINCT ON (symbol) symbol, version, created_at, market
                 FROM ticker_context
             ORDER BY symbol, version DESC
           ), open_thesis AS (
               SELECT symbol, count(*) AS n
                 FROM thesis
                WHERE state NOT IN ('closed', 'disqualified')
             GROUP BY symbol
           ), latest_open_thesis AS (
               SELECT DISTINCT ON (symbol)
                      symbol, thesis_id, state, updated_at, last_evaluated_at
                 FROM thesis
                WHERE state NOT IN ('closed', 'disqualified')
             ORDER BY symbol, updated_at DESC, created_at DESC
           ), latest_decline AS (
               SELECT symbol, max(created_at) AS at
                 FROM attention_item
                WHERE kind = 'thesis_incomplete'
             GROUP BY symbol
           ), due_evidence AS (
               SELECT symbol, max(updated_at) AS at
                 FROM evidence_requirement
                WHERE blocking_state <> 'satisfied'
                  AND (next_retry_at IS NULL OR next_retry_at <= now())
             GROUP BY symbol
           ), newly_satisfied_evidence AS (
               SELECT symbol, max(satisfied_at) AS at
                 FROM evidence_requirement
                WHERE satisfied_at IS NOT NULL
             GROUP BY symbol
           ), evidence_state AS (
               SELECT symbol, count(*) AS evidence_rows
                 FROM evidence_requirement
             GROUP BY symbol
           ), latest_source_task AS (
               SELECT target_id AS symbol,
                      max(updated_at) FILTER (
                          WHERE state = 'satisfied'
                            AND source_ref->>'result' = 'rows_seen'
                      ) AS latest_source_task_at
                 FROM source_task
                WHERE scope = 'symbol'
                  AND target_id <> ''
             GROUP BY target_id
           ), latest_evidence_item AS (
               SELECT symbol,
                      max(updated_at) AS latest_evidence_item_at,
                      count(*) AS normalized_evidence_rows
                 FROM evidence_item
                WHERE NOT (
                    kind = 'product_research'
                    AND source = 'web_research'
                    AND COALESCE((source_ref->'relevance'->>'accepted')::boolean, false) = false
                )
             GROUP BY symbol
           )
           SELECT t.symbol,
                  lc.version AS context_version,
                  lc.created_at AS context_at,
                  (lc.market IS NOT NULL AND lc.market <> '{}'::jsonb) AS context_has_market,
                  COALESCE(ot.n, 0) AS open_theses,
                  lot.thesis_id AS thesis_id,
                  lot.updated_at AS thesis_at,
                  COALESCE(lot.last_evaluated_at, lot.updated_at) AS thesis_evaluated_at,
                  ld.at AS decline_at,
                  de.at AS due_evidence_at,
                  se.at AS evidence_satisfied_at,
                  COALESCE(es.evidence_rows, 0) AS evidence_rows,
                  lst.latest_source_task_at AS source_task_at,
                  lei.latest_evidence_item_at AS evidence_item_at,
                  COALESCE(lei.normalized_evidence_rows, 0) AS normalized_evidence_rows,
                  CASE
                    WHEN lot.thesis_id IS NOT NULL
                     AND lst.latest_source_task_at IS NOT NULL
                     AND lst.latest_source_task_at
                         > COALESCE(
                             lot.last_evaluated_at,
                             lot.updated_at,
                             '-infinity'::timestamptz
                         )
                      THEN 'source_task_changed'
                    WHEN lot.thesis_id IS NOT NULL
                     AND lei.latest_evidence_item_at IS NOT NULL
                     AND lei.latest_evidence_item_at
                         > COALESCE(
                             lot.last_evaluated_at,
                             lot.updated_at,
                             '-infinity'::timestamptz
                         )
                      THEN 'evidence_item_changed'
                    WHEN COALESCE(ot.n, 0) = 0
                     AND lst.latest_source_task_at IS NOT NULL
                     AND (ld.at IS NULL OR lst.latest_source_task_at > ld.at)
                      THEN 'source_task_changed_retry'
                    WHEN COALESCE(ot.n, 0) = 0
                     AND lei.latest_evidence_item_at IS NOT NULL
                     AND (ld.at IS NULL OR lei.latest_evidence_item_at > ld.at)
                      THEN 'evidence_item_changed_retry'
                    WHEN lot.thesis_id IS NOT NULL
                     AND COALESCE(lot.last_evaluated_at, lot.updated_at)
                         < now() - ($2::text || ' minutes')::interval
                      THEN 'open_thesis_due'
                    WHEN lc.created_at IS NULL THEN 'context_missing'
                    WHEN lc.market = '{}'::jsonb THEN 'context_missing_market'
                    WHEN COALESCE(es.evidence_rows, 0) = 0 THEN 'evidence_state_missing'
                    WHEN COALESCE(ot.n, 0) = 0 AND de.at IS NOT NULL THEN 'evidence_retry_due'
                    WHEN COALESCE(ot.n, 0) = 0
                     AND se.at IS NOT NULL
                     AND (ld.at IS NULL OR se.at > ld.at)
                      THEN 'evidence_satisfied_retry'
                    WHEN lc.created_at < now() - ($1::text || ' hours')::interval
                      THEN 'context_stale'
                    WHEN COALESCE(ot.n, 0) = 0
                     AND (
                          ld.at IS NULL
                       OR ld.at < now() - ($3::text || ' hours')::interval
                       OR (
                            lc.created_at IS NOT NULL
                        AND ld.at IS NOT NULL
                        AND lc.created_at > ld.at
                       )
                     )
                      THEN 'thesis_retry_due'
                    ELSE 'maintenance_sweep'
                  END AS sweep_reason
             FROM ticker t
             LEFT JOIN latest_context lc ON lc.symbol = t.symbol
             LEFT JOIN open_thesis ot ON ot.symbol = t.symbol
             LEFT JOIN latest_open_thesis lot ON lot.symbol = t.symbol
             LEFT JOIN latest_decline ld ON ld.symbol = t.symbol
             LEFT JOIN due_evidence de ON de.symbol = t.symbol
             LEFT JOIN newly_satisfied_evidence se ON se.symbol = t.symbol
             LEFT JOIN evidence_state es ON es.symbol = t.symbol
             LEFT JOIN latest_source_task lst ON lst.symbol = t.symbol
             LEFT JOIN latest_evidence_item lei ON lei.symbol = t.symbol
            WHERE t.status = 'active'
              AND (
                    lc.created_at IS NULL
                 OR lc.market = '{}'::jsonb
                 OR COALESCE(es.evidence_rows, 0) = 0
                 OR lc.created_at < now() - ($1::text || ' hours')::interval
                 OR (
                      lot.thesis_id IS NOT NULL
                      AND COALESCE(lot.last_evaluated_at, lot.updated_at)
                          < now() - ($2::text || ' minutes')::interval
                    )
                 OR (
                      lot.thesis_id IS NOT NULL
                      AND lst.latest_source_task_at IS NOT NULL
                      AND lst.latest_source_task_at
                          > COALESCE(
                              lot.last_evaluated_at,
                              lot.updated_at,
                              '-infinity'::timestamptz
                          )
                    )
                 OR (
                      lot.thesis_id IS NOT NULL
                      AND lei.latest_evidence_item_at IS NOT NULL
                      AND lei.latest_evidence_item_at
                          > COALESCE(
                              lot.last_evaluated_at,
                              lot.updated_at,
                              '-infinity'::timestamptz
                          )
                    )
                 OR (
                      COALESCE(ot.n, 0) = 0
                      AND lst.latest_source_task_at IS NOT NULL
                      AND (ld.at IS NULL OR lst.latest_source_task_at > ld.at)
                    )
                 OR (
                      COALESCE(ot.n, 0) = 0
                      AND lei.latest_evidence_item_at IS NOT NULL
                      AND (ld.at IS NULL OR lei.latest_evidence_item_at > ld.at)
                    )
                 OR (
                      COALESCE(ot.n, 0) = 0
                      AND de.at IS NOT NULL
                    )
                 OR (
                      COALESCE(ot.n, 0) = 0
                      AND se.at IS NOT NULL
                      AND (ld.at IS NULL OR se.at > ld.at)
                    )
                 OR (
                      COALESCE(ot.n, 0) = 0
                      AND (
                           ld.at IS NULL
                        OR ld.at < now() - ($3::text || ' hours')::interval
                        OR (
                             lc.created_at IS NOT NULL
                         AND ld.at IS NOT NULL
                         AND lc.created_at > ld.at
                        )
                      )
                    )
              )
         ORDER BY
              CASE
                WHEN lot.thesis_id IS NOT NULL
                 AND lst.latest_source_task_at IS NOT NULL
                 AND lst.latest_source_task_at
                     > COALESCE(
                         lot.last_evaluated_at,
                         lot.updated_at,
                         '-infinity'::timestamptz
                     ) THEN 0
                WHEN lot.thesis_id IS NOT NULL
                 AND lei.latest_evidence_item_at IS NOT NULL
                 AND lei.latest_evidence_item_at
                     > COALESCE(
                         lot.last_evaluated_at,
                         lot.updated_at,
                         '-infinity'::timestamptz
                     ) THEN 1
                WHEN lot.thesis_id IS NOT NULL
                 AND COALESCE(lot.last_evaluated_at, lot.updated_at)
                     < now() - ($2::text || ' minutes')::interval THEN 2
                WHEN lc.created_at IS NULL THEN 3
                WHEN lc.market = '{}'::jsonb THEN 4
                WHEN COALESCE(es.evidence_rows, 0) = 0 THEN 5
                WHEN COALESCE(ot.n, 0) = 0 AND de.at IS NOT NULL THEN 6
                WHEN COALESCE(ot.n, 0) = 0
                 AND lst.latest_source_task_at IS NOT NULL
                 AND (ld.at IS NULL OR lst.latest_source_task_at > ld.at) THEN 7
                WHEN COALESCE(ot.n, 0) = 0
                 AND lei.latest_evidence_item_at IS NOT NULL
                 AND (ld.at IS NULL OR lei.latest_evidence_item_at > ld.at) THEN 8
                WHEN lc.created_at < now() - ($1::text || ' hours')::interval THEN 9
                ELSE 10
              END,
              COALESCE(lot.last_evaluated_at, lot.updated_at, lc.created_at) ASC NULLS FIRST,
              t.tier ASC,
              t.added_at ASC
            LIMIT $4""",
        str(context_max_age_hours),
        str(open_thesis_max_age_minutes),
        str(decline_retry_hours),
        limit,
    )


def _sweep_trigger(
    evidence_rows: int,
    thesis_id: object | None,
    sweep_reason: str | None = None,
) -> str:
    if sweep_reason in SOURCE_TASK_DELTA_SWEEP_REASONS:
        return "source_task_delta"
    if sweep_reason in EVIDENCE_DELTA_SWEEP_REASONS:
        return "evidence_delta"
    if thesis_id:
        return "open_thesis_update_loop"
    if evidence_rows == 0:
        return "evidence_state_bootstrap"
    return "maintenance_sweep"


def _select_sweep_targets(
    rows: list,
    *,
    limit: int,
    bootstrap_floor: int,
) -> list:
    """Reserve some sweep capacity for symbols that have not bootstrapped yet.

    Open theses are intentionally re-evaluated often, but when many are stale
    they can occupy every sweep slot. New watchlist/discovery symbols then keep
    showing "initialize evidence" indefinitely. This selector preserves the SQL
    order while forcing a small number of no-thesis/bootstrap rows through each
    pass whenever they exist.
    """
    if limit <= 0:
        return []
    bootstrap: list = []
    selected: list = []
    seen: set[str] = set()
    for row in rows:
        reason = str(row["sweep_reason"] or "")
        symbol = str(row["symbol"])
        if reason in BOOTSTRAP_SWEEP_REASONS and symbol not in seen:
            bootstrap.append(row)
        if len(bootstrap) >= bootstrap_floor:
            break
    for row in bootstrap[:bootstrap_floor]:
        symbol = str(row["symbol"])
        if symbol in seen:
            continue
        selected.append(row)
        seen.add(symbol)
        if len(selected) >= limit:
            return selected
    for row in rows:
        symbol = str(row["symbol"])
        if symbol in seen:
            continue
        selected.append(row)
        seen.add(symbol)
        if len(selected) >= limit:
            return selected
    return selected


async def _run_sweep_targets(pool: asyncpg.Pool, targets: list) -> None:
    if not targets:
        return
    concurrency = min(_sweep_concurrency(), len(targets))
    semaphore = asyncio.Semaphore(concurrency)

    async def run_target(row) -> None:
        async with semaphore:
            await _run_sweep_target(pool, row)

    await asyncio.gather(*(run_target(row) for row in targets))


async def _run_sweep_target(pool: asyncpg.Pool, row) -> None:
    symbol = row["symbol"]
    open_theses = int(row["open_theses"] or 0)
    evidence_rows = int(row["evidence_rows"] or 0)
    thesis_at = row["thesis_at"].isoformat() if row["thesis_at"] else None
    thesis_evaluated_at = (
        row["thesis_evaluated_at"].isoformat() if row["thesis_evaluated_at"] else None
    )
    source_task_at = row["source_task_at"].isoformat() if row["source_task_at"] else None
    evidence_item_at = (
        row["evidence_item_at"].isoformat()
        if hasattr(row["evidence_item_at"], "isoformat")
        else row["evidence_item_at"]
    )
    trigger = _sweep_trigger(evidence_rows, row["thesis_id"], row["sweep_reason"])

    async def run_symbol() -> None:
        await _run_pipeline(
            pool,
            symbol,
            source_ref={
                "reason": "no_edge",
                "trigger": trigger,
                "context_version": row["context_version"],
                "thesis_id": str(row["thesis_id"]) if row["thesis_id"] else None,
                "thesis_at": thesis_at,
                "thesis_evaluated_at": thesis_evaluated_at,
                "source_task_at": source_task_at,
                "evidence_item_at": evidence_item_at,
                "sweep_reason": row["sweep_reason"],
                "sweep_concurrency": _sweep_concurrency(),
            },
        )

    await _run_symbol_once(
        symbol,
        run_symbol,
    )
    if open_theses > 0:
        log.info("cognition sweep: %s reconciled existing thesis", symbol)


async def _sweep_once(pool: asyncpg.Pool) -> None:
    context_max_age_hours = _env_int("COGNITION_CONTEXT_MAX_AGE_HOURS", 12)
    open_thesis_max_age_minutes = _open_thesis_max_age_minutes()
    decline_retry_hours = _env_int("COGNITION_DECLINE_RETRY_HOURS", 6)
    limit = _effective_sweep_limit()
    bootstrap_floor = _bootstrap_sweep_floor(limit)
    fetch_limit = max(limit, limit * 5, limit + bootstrap_floor * 10)
    evidence_sync_limit = max(1, _env_int("COGNITION_EVIDENCE_SYNC_LIMIT", 200))
    brain_thesis_limit = max(1, _env_int("BRAIN_THESIS_SWEEP_LIMIT", 50))
    reclaimed_runs = await _reclaim_stale_cognition_runs(
        pool,
        max_age_minutes=max(1, _env_int("COGNITION_RUNNING_RECLAIM_MINUTES", 30)),
    )
    if reclaimed_runs:
        log.warning("cognition sweep: reclaimed %d stale running run(s)", reclaimed_runs)
    evidence_synced = await refresh_open_evidence_requirements(
        pool,
        limit=evidence_sync_limit,
    )
    if evidence_synced:
        log.info("cognition sweep: refreshed evidence state for %d symbol(s)", evidence_synced)
    targets = await _sweep_targets(
        pool,
        context_max_age_hours=context_max_age_hours,
        open_thesis_max_age_minutes=open_thesis_max_age_minutes,
        decline_retry_hours=decline_retry_hours,
        limit=fetch_limit,
    )
    targets = _select_sweep_targets(
        targets,
        limit=limit,
        bootstrap_floor=bootstrap_floor,
    )
    if not targets:
        log.info("cognition sweep: no stale active tickers")
    else:
        log.info(
            "cognition sweep: %d target(s), concurrency=%d",
            len(targets),
            min(_sweep_concurrency(), len(targets)),
        )
        await _run_sweep_targets(pool, targets)
    brain_updated = await refresh_brain_theses(pool, limit=brain_thesis_limit)
    if brain_updated:
        log.info("cognition sweep: evaluated %d parent brain thesis row(s)", brain_updated)


async def _sweep_loop(pool: asyncpg.Pool) -> None:
    interval = _effective_sweep_interval_seconds()
    if interval <= 0:
        log.info("cognition maintenance sweep disabled")
        return
    log.info(
        "cognition maintenance sweep enabled: every %ss, max %s symbols, concurrency %s",
        interval,
        _effective_sweep_limit(),
        _sweep_concurrency(),
    )
    await asyncio.sleep(5)
    while True:
        try:
            await _sweep_once(pool)
        except Exception:  # noqa: BLE001
            log.exception("cognition sweep failed")
        await asyncio.sleep(interval)


async def _message_loop(pool: asyncpg.Pool, psub) -> None:
    while True:
        try:
            msgs = await psub.fetch(batch=1, timeout=10)
        except NatsTimeout:
            continue
        for msg in msgs:
            try:
                await _on_confirmed(pool, msg)
            except Exception:  # noqa: BLE001
                log.exception("cognition: handler failed")
                await msg.nak()


async def run() -> None:
    cfg = config.load()
    pool = await asyncpg.create_pool(
        cfg.database_url,
        min_size=1,
        max_size=max(3, _sweep_concurrency() + 2),
    )
    assert pool is not None
    reclaimed_runs = await _reclaim_running_cognition_runs(pool)
    if reclaimed_runs:
        log.warning("cognition startup: reclaimed %d orphaned running run(s)", reclaimed_runs)
    nats_url = os.getenv("NATS_URL", "nats://localhost:4222")
    nc = await nats.connect(nats_url)
    js = nc.jetstream()
    # Ensure stream exists. NotFoundError can fire on a brand-new cluster;
    # the Rust services normally create it but be defensive.
    try:
        await js.stream_info(STREAM)
    except NotFoundError:
        await js.add_stream(name=STREAM, subjects=["regime.*", "discovery.*"])

    psub = await js.pull_subscribe(SUBJECT, durable=DURABLE, stream=STREAM)
    log.info("cognition consumer subscribed: stream=%s subject=%s durable=%s",
             STREAM, SUBJECT, DURABLE)
    try:
        await asyncio.gather(_message_loop(pool, psub), _sweep_loop(pool), source_task_loop(pool))
    finally:
        await nc.drain()
        await pool.close()


def _cli() -> None:
    logging.basicConfig(level=logging.INFO,
                        format="%(asctime)s %(name)s %(levelname)s %(message)s")
    asyncio.run(run())


if __name__ == "__main__":
    _cli()
