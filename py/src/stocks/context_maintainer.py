"""Context maintainer service (SPEC §3, §5.2).

Consumes routed per-ticker events and (eventually) LLM-synthesizes the
structural/narrative bands of the ticker context, append-only. Working skeleton:
connects to NATS + Postgres and runs the loop with the mock LLM.
"""

from __future__ import annotations

import asyncio
import json
import logging
import signal

import asyncpg
import nats

from . import config
from .llm import Message, Request, new_provider

log = logging.getLogger("context_maintainer")


async def main() -> None:
    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s %(name)s %(levelname)s %(message)s",
    )
    cfg = config.load()
    provider = new_provider(cfg.llm_provider)

    pool = await asyncpg.create_pool(cfg.database_url)
    nc = await nats.connect(cfg.nats_url)
    log.info("context_maintainer connected (nats=%s)", cfg.nats_url)

    async def on_msg(msg) -> None:  # noqa: ANN001 (nats Msg)
        symbol = msg.subject.rsplit(".", 1)[-1]
        try:
            event = json.loads(msg.data)
        except json.JSONDecodeError:
            event = {"raw": msg.data.decode("utf-8", "replace")}

        # TODO: real synthesis — read current context, ask LLM to update the
        # relevant band, persist a new ticker_context version (append-only),
        # emit context.shift when significant. For now, exercise the loop:
        await provider.complete(
            Request(
                model=cfg.model_routine,
                system="You maintain a per-ticker structured context.",
                messages=[Message(role="user", content=json.dumps(event)[:4000])],
            )
        )
        log.info("processed event for %s", symbol)

    await nc.subscribe("route.ticker.>", cb=on_msg)
    await nc.subscribe("ingest.filing", cb=on_msg)  # MVP stand-in until the router exists

    stop = asyncio.Event()
    loop = asyncio.get_running_loop()
    for sig in (signal.SIGINT, signal.SIGTERM):
        loop.add_signal_handler(sig, stop.set)
    await stop.wait()

    await nc.drain()
    await pool.close()


if __name__ == "__main__":
    asyncio.run(main())
