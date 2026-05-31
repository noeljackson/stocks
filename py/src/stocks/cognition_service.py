"""Cognition consumer (#100). Subscribes to `discovery.confirmed` and runs
context_maintainer → thesis_engine → sharpen → challenge for the symbol the
operator just promoted. Closes the manual `make refresh-context SYMBOL=X`
gap — the system keeps cooking after confirm.

Honest decline path: thesis_engine may return edge_present=false. We still
persist the context refresh (already valuable) and emit a single 'no_thesis'
attention item (severity=info) so the operator sees the system tried.

Usage:
    python -m stocks.cognition_service

Reads:
    NATS_URL                — default nats://localhost:4222
    STREAM_MARKET           — discovery.* subjects are under MARKET stream
    DURABLE                 — "cognition-consumer"
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

    log.info("cognition kickoff: %s (candidate_id=%s)", symbol, candidate_id)

    # 1. Refresh context — fast, almost always succeeds.
    try:
        ctx_version = await refresh_context(symbol)
        log.info("cognition: %s context refreshed to v%s", symbol, ctx_version)
    except Exception:  # noqa: BLE001
        log.exception("cognition: context refresh failed for %s", symbol)
        ctx_version = None

    # 2. Draft thesis — may honestly decline.
    thesis_id = None
    try:
        result = await draft_thesis(symbol)
        if result and result.get("_thesis_id"):
            thesis_id = result["_thesis_id"]
            log.info("cognition: %s thesis drafted %s", symbol, thesis_id)
        else:
            log.info("cognition: %s thesis declined (no edge)", symbol)
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
                (result or {}).get("no_edge_reason"),
                json.dumps({"reason": "no_edge"}),
            )
    except Exception:  # noqa: BLE001
        log.exception("cognition: thesis_engine failed for %s", symbol)

    # 3. If thesis drafted, run sharpen + challenge (advisory; never blocks).
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

    await msg.ack()


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
    finally:
        await nc.drain()
        await pool.close()


def _cli() -> None:
    logging.basicConfig(level=logging.INFO,
                        format="%(asctime)s %(name)s %(levelname)s %(message)s")
    asyncio.run(run())


if __name__ == "__main__":
    _cli()
