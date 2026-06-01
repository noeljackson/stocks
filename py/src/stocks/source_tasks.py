"""Source-task worker for acquisition retries and freshness checks.

The Rust ingest loops own market-data providers. This worker owns Python-native
retrieval tasks, starting with product/theme web research. It claims due
`source_task` rows, executes the provider action, then refreshes evidence state
so the UI and cognition sweep see the new result immediately.
"""

from __future__ import annotations

import argparse
import asyncio
import datetime as dt
import json
import logging
import os
from collections.abc import Sequence

import asyncpg

from . import config
from .evidence import load_evidence_counts, load_source_health, sync_evidence_requirements
from .research import refresh_research_evidence

log = logging.getLogger("source_tasks")

WEB_RESEARCH_ACTIONS = ("gdelt_doc_search", "bing_news_rss_search")
CLAIMABLE_STATES = ("queued", "no_rows", "failed", "rate_limited", "satisfied")
RESEARCH_PROVIDER_ACTION = {
    "gdelt_doc": "gdelt_doc_search",
    "bing_news_rss": "bing_news_rss_search",
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


def retry_delay_minutes(attempts: int) -> int:
    """Bounded exponential retry delay for transient source-task failures."""
    attempt = max(1, attempts)
    return min(6 * 60, 15 * (2 ** (attempt - 1)))


def is_rate_limit_error(error: str | None) -> bool:
    if not error:
        return False
    normalized = error.lower()
    return "429" in normalized or "too many requests" in normalized or "rate limit" in normalized


async def claim_due_web_research_symbols(
    pool: asyncpg.Pool,
    *,
    limit: int,
    actions: Sequence[str] = WEB_RESEARCH_ACTIONS,
) -> list[str]:
    rows = await pool.fetch(
        """WITH due_symbols AS (
               SELECT target_id,
                      min(CASE priority
                            WHEN 'blocking' THEN 0
                            WHEN 'high' THEN 1
                            WHEN 'medium' THEN 2
                            ELSE 3
                          END) AS priority_rank,
                      min(due_at) AS first_due_at
                 FROM source_task
                WHERE scope = 'symbol'
                  AND action = ANY($1::text[])
                  AND state = ANY($2::text[])
                  AND due_at <= now()
             GROUP BY target_id
             ORDER BY priority_rank, first_due_at
                LIMIT $3
           ),
           claimed AS (
               UPDATE source_task st
                  SET state = 'fetching',
                      attempts = st.attempts + 1,
                      last_error = NULL,
                      updated_at = now(),
                      source_ref = st.source_ref || jsonb_build_object(
                          'claimed_by', 'source_tasks.web_research',
                          'claimed_at', now()
                      )
                 FROM due_symbols ds
                WHERE st.scope = 'symbol'
                  AND st.target_id = ds.target_id
                  AND st.action = ANY($1::text[])
                  AND st.state = ANY($2::text[])
                  AND st.due_at <= now()
             RETURNING st.target_id
           )
           SELECT DISTINCT target_id
             FROM claimed
         ORDER BY target_id""",
        list(actions),
        list(CLAIMABLE_STATES),
        limit,
    )
    return [row["target_id"] for row in rows]


async def _mark_failed(pool: asyncpg.Pool, symbol: str, error: str) -> None:
    attempts = int(
        await pool.fetchval(
            """SELECT COALESCE(max(attempts), 1)
                 FROM source_task
                WHERE scope = 'symbol'
                  AND target_id = $1
                  AND action = ANY($2::text[])""",
            symbol,
            list(WEB_RESEARCH_ACTIONS),
        )
        or 1
    )
    retry_at = dt.datetime.now(dt.UTC) + dt.timedelta(minutes=retry_delay_minutes(attempts))
    await pool.execute(
        """UPDATE source_task
              SET state = 'failed',
                  next_retry_at = $3,
                  due_at = $3,
                  last_error = $4,
                  updated_at = now(),
                  source_ref = source_ref || jsonb_build_object(
                      'failed_by', 'source_tasks.web_research',
                      'failed_at', now()
                  )
            WHERE scope = 'symbol'
              AND target_id = $1
              AND action = ANY($2::text[])""",
        symbol,
        list(WEB_RESEARCH_ACTIONS),
        retry_at,
        error[:500],
    )


async def apply_recent_provider_failures(pool: asyncpg.Pool, symbol: str) -> int:
    rows = await pool.fetch(
        """WITH latest AS (
               SELECT DISTINCT ON (provider) provider, status, last_error
                 FROM research_retrieval_run
                WHERE symbol = $1
                  AND finished_at > now() - interval '5 minutes'
                  AND provider = ANY($2::text[])
             ORDER BY provider, finished_at DESC
           )
           SELECT provider, last_error
             FROM latest
            WHERE status = 'failed'""",
        symbol,
        list(RESEARCH_PROVIDER_ACTION.keys()),
    )
    updated = 0
    for row in rows:
        action = RESEARCH_PROVIDER_ACTION.get(row["provider"])
        if not action:
            continue
        error = row["last_error"] or "provider failed"
        state = "rate_limited" if is_rate_limit_error(error) else "failed"
        attempts = int(
            await pool.fetchval(
                """SELECT COALESCE(attempts, 1)
                     FROM source_task
                    WHERE scope = 'symbol'
                      AND target_id = $1
                      AND action = $2""",
                symbol,
                action,
            )
            or 1
        )
        retry_at = dt.datetime.now(dt.UTC) + dt.timedelta(minutes=retry_delay_minutes(attempts))
        result = await pool.execute(
            """UPDATE source_task
                  SET state = $3,
                      next_retry_at = $4,
                      due_at = $4,
                      last_error = $5,
                      updated_at = now(),
                      source_ref = source_ref || jsonb_build_object(
                          'provider_failure_from', 'research_retrieval_run',
                          'provider_failure_at', now()
                      )
                WHERE scope = 'symbol'
                  AND target_id = $1
                  AND action = $2""",
            symbol,
            action,
            state,
            retry_at,
            error[:500],
        )
        if result.endswith("1"):
            updated += 1
    return updated


async def process_web_research_symbol(pool: asyncpg.Pool, symbol: str) -> int:
    inserted = await refresh_research_evidence(pool, symbol, force=True)
    evidence_counts = await load_evidence_counts(pool, symbol)
    source_health = await load_source_health(pool)
    await sync_evidence_requirements(pool, symbol, evidence_counts, source_health)
    await apply_recent_provider_failures(pool, symbol)
    return inserted


async def run_once(pool: asyncpg.Pool, *, limit: int = 5) -> int:
    symbols = await claim_due_web_research_symbols(pool, limit=limit)
    if not symbols:
        return 0
    processed = 0
    for symbol in symbols:
        try:
            inserted = await process_web_research_symbol(pool, symbol)
            log.info("source task web research complete: %s inserted=%d", symbol, inserted)
            processed += 1
        except Exception as exc:  # noqa: BLE001
            log.exception("source task web research failed for %s", symbol)
            await _mark_failed(pool, symbol, str(exc))
    return processed


async def loop(pool: asyncpg.Pool) -> None:
    interval = _env_int("SOURCE_TASK_SWEEP_SECONDS", 60)
    if interval <= 0:
        log.info("source task worker disabled")
        return
    limit = max(1, _env_int("SOURCE_TASK_MAX_SYMBOLS_PER_SWEEP", 5))
    log.info("source task worker enabled: every %ss, max %s symbols", interval, limit)
    await asyncio.sleep(10)
    while True:
        try:
            processed = await run_once(pool, limit=limit)
            if processed:
                log.info("source task worker processed %d symbol(s)", processed)
        except Exception:  # noqa: BLE001
            log.exception("source task worker sweep failed")
        await asyncio.sleep(interval)


async def _run_cli(limit: int, once: bool) -> None:
    cfg = config.load()
    pool = await asyncpg.create_pool(cfg.database_url, min_size=1, max_size=2)
    assert pool is not None
    try:
        if once:
            processed = await run_once(pool, limit=limit)
            print(json.dumps({"processed": processed}, sort_keys=True))
        else:
            await loop(pool)
    finally:
        await pool.close()


def _cli() -> None:
    parser = argparse.ArgumentParser(prog="source_tasks")
    parser.add_argument("--limit", type=int, default=5)
    parser.add_argument("--once", action="store_true")
    args = parser.parse_args()
    logging.basicConfig(level=logging.INFO, format="%(asctime)s %(name)s %(levelname)s %(message)s")
    asyncio.run(_run_cli(args.limit, args.once))


if __name__ == "__main__":
    _cli()
