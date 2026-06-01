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
    COGNITION_DECLINE_RETRY_HOURS — default 6
    COGNITION_MAX_SYMBOLS_PER_SWEEP — default 5
"""

from __future__ import annotations

import asyncio
import json
import logging
import os

import asyncpg
import nats
from nats.errors import TimeoutError as NatsTimeout
from nats.js.errors import NotFoundError

from . import config
from .challenge import challenge as challenge_thesis
from .context_maintainer import refresh as refresh_context
from .sharpen import sharpen as sharpen_thesis
from .thesis_engine import draft as draft_thesis

log = logging.getLogger("cognition")

STREAM = "MARKET"
SUBJECT = "discovery.confirmed"
DURABLE = "cognition-consumer"


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
    await pool.execute(
        """INSERT INTO attention_item
             (kind, symbol, candidate_id, severity, title, reason,
              source, source_ref)
           VALUES ('thesis_incomplete', $1, $2, 'info', $3, $4,
                   'thesis', $5::jsonb)
           ON CONFLICT DO NOTHING""",
        symbol,
        candidate_id,
        f"{symbol}: system declined to draft a thesis",
        reason,
        json.dumps(source_ref),
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
    except Exception:  # noqa: BLE001
        log.exception("cognition: context refresh failed for %s", symbol)

    if not draft_when_thesis_exists and await _open_thesis_count(pool, symbol) > 0:
        log.info("cognition: %s already has an open thesis; refresh only", symbol)
        return

    thesis_id = None
    try:
        result = await draft_thesis(symbol)
        if result and result.get("_thesis_id"):
            thesis_id = result["_thesis_id"]
            log.info("cognition: %s thesis drafted %s", symbol, thesis_id)
        else:
            log.info("cognition: %s thesis declined (no edge)", symbol)
            await _record_decline(
                pool,
                symbol,
                candidate_id,
                (result or {}).get("no_edge_reason"),
                source_ref or {"reason": "no_edge"},
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

    await _run_pipeline(
        pool,
        symbol,
        candidate_id=candidate_id,
        draft_when_thesis_exists=True,
        source_ref={"reason": "no_edge", "trigger": "discovery.confirmed"},
    )
    await msg.ack()


async def _sweep_targets(
    pool: asyncpg.Pool,
    *,
    context_max_age_hours: int,
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
           ), latest_decline AS (
               SELECT symbol, max(created_at) AS at
                 FROM attention_item
                WHERE kind = 'thesis_incomplete'
             GROUP BY symbol
           )
           SELECT t.symbol,
                  lc.version AS context_version,
                  lc.created_at AS context_at,
                  (lc.market IS NOT NULL AND lc.market <> '{}'::jsonb) AS context_has_market,
                  COALESCE(ot.n, 0) AS open_theses,
                  ld.at AS decline_at
             FROM ticker t
             LEFT JOIN latest_context lc ON lc.symbol = t.symbol
             LEFT JOIN open_thesis ot ON ot.symbol = t.symbol
             LEFT JOIN latest_decline ld ON ld.symbol = t.symbol
            WHERE t.status = 'active'
              AND (
                    lc.created_at IS NULL
                 OR lc.market = '{}'::jsonb
                 OR lc.created_at < now() - ($1::text || ' hours')::interval
                 OR (
                      COALESCE(ot.n, 0) = 0
                      AND (
                           ld.at IS NULL
                        OR ld.at < now() - ($2::text || ' hours')::interval
                      )
                    )
              )
         ORDER BY
              CASE WHEN lc.created_at IS NULL THEN 0 ELSE 1 END,
              lc.created_at ASC NULLS FIRST,
              t.tier ASC,
              t.added_at ASC
            LIMIT $3""",
        str(context_max_age_hours),
        str(decline_retry_hours),
        limit,
    )


async def _sweep_once(pool: asyncpg.Pool) -> None:
    context_max_age_hours = _env_int("COGNITION_CONTEXT_MAX_AGE_HOURS", 12)
    decline_retry_hours = _env_int("COGNITION_DECLINE_RETRY_HOURS", 6)
    limit = max(1, _env_int("COGNITION_MAX_SYMBOLS_PER_SWEEP", 5))
    targets = await _sweep_targets(
        pool,
        context_max_age_hours=context_max_age_hours,
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
        await _run_pipeline(
            pool,
            symbol,
            draft_when_thesis_exists=False,
            source_ref={
                "reason": "no_edge",
                "trigger": "maintenance_sweep",
                "context_version": row["context_version"],
            },
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
