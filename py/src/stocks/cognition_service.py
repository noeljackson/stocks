"""Cognition consumer (#100).

Subscribes to `discovery.confirmed` and runs context_maintainer → thesis_engine
→ sharpen → challenge for the symbol the operator just promoted. It also runs a
maintenance sweep over active tickers so the system does not depend on manual
`make refresh-context SYMBOL=X` or opening a UI tab.

Honest decline path: thesis_engine may return edge_present=false. We still
persist the context refresh (already valuable) and emit a single 'no_thesis'
attention item (severity=info) so the operator sees the system tried.

Usage:
    python -m stocks.cognition_service

Reads:
    NATS_URL                — default nats://localhost:4222
    STREAM_MARKET           — discovery.* subjects are under MARKET stream
    DURABLE                 — "cognition-consumer"
    COGNITION_SWEEP_SECONDS — default 900; set 0 to disable maintenance sweep
    COGNITION_CONTEXT_MAX_AGE_HOURS — default 12
    COGNITION_OPEN_THESIS_MAX_AGE_MINUTES — default 30
    COGNITION_DECLINE_RETRY_HOURS — default 6
    COGNITION_MAX_SYMBOLS_PER_SWEEP — default 5
    COGNITION_ACK_PROGRESS_SECONDS — default 10
"""

from __future__ import annotations

import asyncio
import json
import logging
import os
from collections.abc import Awaitable, Callable

import asyncpg
import nats
from nats.errors import TimeoutError as NatsTimeout
from nats.js.errors import NotFoundError

from . import config
from .challenge import challenge as challenge_thesis
from .context_maintainer import BlockingEvidenceMissing
from .context_maintainer import refresh as refresh_context
from .evidence import load_open_evidence_requirements
from .sharpen import sharpen as sharpen_thesis
from .thesis_engine import draft as draft_thesis

log = logging.getLogger("cognition")

STREAM = "MARKET"
SUBJECT = "discovery.confirmed"
DURABLE = "cognition-consumer"
_IN_FLIGHT_SYMBOLS: set[str] = set()


def _env_int(name: str, default: int) -> int:
    raw = os.getenv(name)
    if raw is None or raw == "":
        return default
    try:
        return int(raw)
    except ValueError:
        log.warning("invalid %s=%r; using %d", name, raw, default)
        return default


async def _open_thesis_count(pool: asyncpg.Pool, symbol: str) -> int:
    return int(await pool.fetchval(
        """SELECT count(*)
             FROM thesis
            WHERE symbol = $1
              AND state NOT IN ('closed', 'disqualified')""",
        symbol,
    ) or 0)


async def _record_decline(
    pool: asyncpg.Pool,
    symbol: str,
    candidate_id: int | None,
    reason: str | None,
    source_ref: dict,
) -> None:
    evidence = await load_open_evidence_requirements(pool, symbol)
    full_ref = dict(source_ref)
    full_ref["missing_evidence"] = evidence
    await pool.execute(
        """WITH updated AS (
               UPDATE attention_item
                  SET reason = $4,
                      source_ref = $5::jsonb
                WHERE status = 'open'
                  AND kind = 'thesis_incomplete'
                  AND symbol = $1
              RETURNING id
           )
           INSERT INTO attention_item
             (kind, symbol, candidate_id, severity, title, reason,
              source, source_ref)
           SELECT 'thesis_incomplete', $1, $2, 'info', $3, $4,
                  'thesis', $5::jsonb
            WHERE NOT EXISTS (SELECT 1 FROM updated)
           ON CONFLICT DO NOTHING""",
        symbol,
        candidate_id,
        f"{symbol}: system declined to draft a thesis",
        reason,
        json.dumps(full_ref),
    )


async def _run_pipeline(
    pool: asyncpg.Pool,
    symbol: str,
    *,
    candidate_id: int | None = None,
    draft_when_thesis_exists: bool = True,
    source_ref: dict | None = None,
) -> None:
    log.info("cognition kickoff: %s (candidate_id=%s)", symbol, candidate_id)

    try:
        ctx_version = await refresh_context(symbol)
        log.info("cognition: %s context refreshed to v%s", symbol, ctx_version)
    except BlockingEvidenceMissing as e:
        log.info(
            "cognition: %s waiting for blocking evidence before context: %s",
            symbol,
            [r["requirement_key"] for r in e.missing],
        )
    except Exception:  # noqa: BLE001
        log.exception("cognition: context refresh failed for %s", symbol)

    if not draft_when_thesis_exists and await _open_thesis_count(pool, symbol) > 0:
        log.info("cognition: %s already has an open thesis; draft will reconcile", symbol)

    open_evidence = await load_open_evidence_requirements(pool, symbol)
    blocking_evidence = [r for r in open_evidence if r["priority"] == "blocking"]
    if blocking_evidence:
        log.info("cognition: %s waiting for blocking evidence before thesis draft", symbol)
        decline_ref = dict(source_ref or {"reason": "missing_evidence"})
        decline_ref["blocking_evidence"] = blocking_evidence
        await _record_decline(
            pool,
            symbol,
            candidate_id,
            "Waiting for blocking evidence before drafting a thesis.",
            decline_ref,
        )
        return

    thesis_id = None
    try:
        result = await draft_thesis(symbol)
        if result and result.get("_thesis_id"):
            thesis_id = result["_thesis_id"]
            log.info("cognition: %s thesis drafted %s", symbol, thesis_id)
        else:
            log.info("cognition: %s thesis declined (no edge)", symbol)
            decline_ref = dict(source_ref or {"reason": "no_edge"})
            if result and result.get("missing_evidence"):
                decline_ref["llm_missing_evidence"] = result["missing_evidence"]
            await _record_decline(
                pool,
                symbol,
                candidate_id,
                (result or {}).get("no_edge_reason"),
                decline_ref,
            )
    except Exception:  # noqa: BLE001
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
                draft_when_thesis_exists=False,
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
                      symbol, thesis_id, state, updated_at
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
           )
           SELECT t.symbol,
                  lc.version AS context_version,
                  lc.created_at AS context_at,
                  (lc.market IS NOT NULL AND lc.market <> '{}'::jsonb) AS context_has_market,
                  COALESCE(ot.n, 0) AS open_theses,
                  lot.thesis_id AS thesis_id,
                  lot.updated_at AS thesis_at,
                  ld.at AS decline_at,
                  de.at AS due_evidence_at,
                  se.at AS evidence_satisfied_at,
                  COALESCE(es.evidence_rows, 0) AS evidence_rows
             FROM ticker t
             LEFT JOIN latest_context lc ON lc.symbol = t.symbol
             LEFT JOIN open_thesis ot ON ot.symbol = t.symbol
             LEFT JOIN latest_open_thesis lot ON lot.symbol = t.symbol
             LEFT JOIN latest_decline ld ON ld.symbol = t.symbol
             LEFT JOIN due_evidence de ON de.symbol = t.symbol
             LEFT JOIN newly_satisfied_evidence se ON se.symbol = t.symbol
             LEFT JOIN evidence_state es ON es.symbol = t.symbol
            WHERE t.status = 'active'
              AND (
                    lc.created_at IS NULL
                 OR lc.market = '{}'::jsonb
                 OR COALESCE(es.evidence_rows, 0) = 0
                 OR lc.created_at < now() - ($1::text || ' hours')::interval
                 OR (
                      lot.thesis_id IS NOT NULL
                      AND lot.updated_at < now() - ($2::text || ' minutes')::interval
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
                WHEN lc.created_at IS NULL THEN 0
                WHEN COALESCE(es.evidence_rows, 0) = 0 THEN 1
                WHEN lot.thesis_id IS NOT NULL
                 AND lot.updated_at < now() - ($2::text || ' minutes')::interval THEN 2
                WHEN lc.market = '{}'::jsonb THEN 3
                WHEN lc.created_at < now() - ($1::text || ' hours')::interval THEN 4
                ELSE 4
              END,
              COALESCE(lot.updated_at, lc.created_at) ASC NULLS FIRST,
              t.tier ASC,
              t.added_at ASC
            LIMIT $4""",
        str(context_max_age_hours),
        str(open_thesis_max_age_minutes),
        str(decline_retry_hours),
        limit,
    )


def _sweep_trigger(evidence_rows: int, thesis_id: object | None) -> str:
    if evidence_rows == 0:
        return "evidence_state_bootstrap"
    if thesis_id:
        return "open_thesis_update_loop"
    return "maintenance_sweep"


async def _sweep_once(pool: asyncpg.Pool) -> None:
    context_max_age_hours = _env_int("COGNITION_CONTEXT_MAX_AGE_HOURS", 12)
    open_thesis_max_age_minutes = _env_int("COGNITION_OPEN_THESIS_MAX_AGE_MINUTES", 30)
    decline_retry_hours = _env_int("COGNITION_DECLINE_RETRY_HOURS", 6)
    limit = max(1, _env_int("COGNITION_MAX_SYMBOLS_PER_SWEEP", 5))
    targets = await _sweep_targets(
        pool,
        context_max_age_hours=context_max_age_hours,
        open_thesis_max_age_minutes=open_thesis_max_age_minutes,
        decline_retry_hours=decline_retry_hours,
        limit=limit,
    )
    if not targets:
        log.info("cognition sweep: no stale active tickers")
        return
    log.info("cognition sweep: %d target(s)", len(targets))
    for row in targets:
        symbol = row["symbol"]
        open_theses = int(row["open_theses"] or 0)
        evidence_rows = int(row["evidence_rows"] or 0)
        thesis_at = row["thesis_at"].isoformat() if row["thesis_at"] else None
        trigger = _sweep_trigger(evidence_rows, row["thesis_id"])
        await _run_symbol_once(
            symbol,
            lambda symbol=symbol, row=row, thesis_at=thesis_at, trigger=trigger: _run_pipeline(
                pool,
                symbol,
                draft_when_thesis_exists=False,
                source_ref={
                    "reason": "no_edge",
                    "trigger": trigger,
                    "context_version": row["context_version"],
                    "thesis_id": str(row["thesis_id"]) if row["thesis_id"] else None,
                    "thesis_at": thesis_at,
                },
            ),
        )
        if open_theses > 0:
            log.info("cognition sweep: %s refreshed existing thesis context", symbol)


async def _sweep_loop(pool: asyncpg.Pool) -> None:
    interval = _env_int("COGNITION_SWEEP_SECONDS", 900)
    if interval <= 0:
        log.info("cognition maintenance sweep disabled")
        return
    log.info(
        "cognition maintenance sweep enabled: every %ss, max %s symbols",
        interval,
        _env_int("COGNITION_MAX_SYMBOLS_PER_SWEEP", 5),
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
    pool = await asyncpg.create_pool(cfg.database_url, min_size=1, max_size=3)
    assert pool is not None
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
        await asyncio.gather(_message_loop(pool, psub), _sweep_loop(pool))
    finally:
        await nc.drain()
        await pool.close()


def _cli() -> None:
    logging.basicConfig(level=logging.INFO,
                        format="%(asctime)s %(name)s %(levelname)s %(message)s")
    asyncio.run(run())


if __name__ == "__main__":
    _cli()
