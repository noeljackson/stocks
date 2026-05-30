"""Context maintainer service (SPEC §3, §5.2).

Phase A (#7): single-symbol CLI that reads ingest_event rows for a ticker,
calls GLM-5.1 with prompts/synthesize-context.md, persists a new
ticker_context row. No NATS subscription, no other tickers, no scheduling —
prove the loop on NVDA first.

Usage:  python -m stocks.context_maintainer SYMBOL [--limit N]
Example: python -m stocks.context_maintainer NVDA
"""

from __future__ import annotations

import argparse
import asyncio
import datetime as dt
import json
import logging
from pathlib import Path

import asyncpg

from . import config
from .llm import TransportConfig, detect, new_provider
from .prompts import AsyncpgRecorder, invoke, load

log = logging.getLogger("context_maintainer")


def _provider_name(cfg: config.Config) -> str:
    if cfg.llm_provider:
        return cfg.llm_provider
    return detect(
        TransportConfig(
            anthropic_api_key=cfg.anthropic_api_key,
            openai_base_url=cfg.openai_base_url,
            openai_api_key=cfg.openai_api_key,
        )
    )


def _llm_cfg(cfg: config.Config) -> TransportConfig:
    return TransportConfig(
        provider=cfg.llm_provider,
        model=cfg.model_routine,
        anthropic_base_url=cfg.anthropic_base_url,
        anthropic_api_key=cfg.anthropic_api_key,
        anthropic_version=cfg.anthropic_version,
        openai_base_url=cfg.openai_base_url,
        openai_api_key=cfg.openai_api_key,
    )


async def _load_prior_context(pool: asyncpg.Pool, symbol: str) -> dict | None:
    row = await pool.fetchrow(
        """SELECT version, structural, narrative, created_at
             FROM ticker_context
            WHERE symbol = $1
         ORDER BY version DESC
            LIMIT 1""",
        symbol,
    )
    if row is None:
        return None
    def _j(v):
        return json.loads(v) if isinstance(v, str) else v
    return {
        "version": row["version"],
        "structural": _j(row["structural"]),
        "narrative": _j(row["narrative"]),
        "as_of": row["created_at"].isoformat(),
    }


async def _load_events(
    pool: asyncpg.Pool, symbol: str, since: dt.datetime | None, limit: int,
) -> list[dict]:
    if since is None:
        rows = await pool.fetch(
            """SELECT source, kind, payload, source_ts, ingested_at
                 FROM ingest_event
                WHERE symbol = $1
             ORDER BY ingested_at DESC
                LIMIT $2""",
            symbol,
            limit,
        )
    else:
        rows = await pool.fetch(
            """SELECT source, kind, payload, source_ts, ingested_at
                 FROM ingest_event
                WHERE symbol = $1 AND ingested_at > $2
             ORDER BY ingested_at DESC
                LIMIT $3""",
            symbol,
            since,
            limit,
        )
    out = []
    for r in rows:
        payload = r["payload"]
        if isinstance(payload, str):
            payload = json.loads(payload)
        out.append({
            "source": r["source"],
            "kind": r["kind"],
            "payload": payload,
            "source_ts": r["source_ts"].isoformat() if r["source_ts"] else None,
            "ingested_at": r["ingested_at"].isoformat(),
        })
    return out


def _build_user_message(symbol: str, prior: dict | None, events: list[dict], today: str) -> str:
    """The system prompt is the rendered template. This user message carries
    the actual data the LLM should reason over."""
    return json.dumps(
        {
            "symbol": symbol,
            "today": today,
            "prior_context": prior,
            "new_events": events,
        },
        indent=2,
        default=str,
    )


def _extract_json(content: str) -> dict:
    """LLMs sometimes wrap JSON in ```json fences despite the prompt. Try
    strict-parse first; fall back to finding the first {...} block."""
    s = content.strip()
    try:
        return json.loads(s)
    except json.JSONDecodeError:
        pass
    # Strip common markdown wrappers.
    for fence in ("```json", "```"):
        if s.startswith(fence):
            s = s[len(fence):].lstrip()
            break
    if s.endswith("```"):
        s = s[:-3].rstrip()
    try:
        return json.loads(s)
    except json.JSONDecodeError:
        pass
    # Last resort: find first balanced brace.
    start = s.find("{")
    end = s.rfind("}")
    if start >= 0 and end > start:
        return json.loads(s[start:end + 1])
    raise ValueError(f"could not parse JSON from LLM response: {s[:200]}")


async def _persist_context(
    pool: asyncpg.Pool,
    symbol: str,
    structural: dict,
    narrative: dict,
    prior_version: int | None,
) -> int:
    """Append a new ticker_context row. Idempotent in the sense that a fresh
    call just bumps the version."""
    new_version = (prior_version or 0) + 1
    now = dt.datetime.now(dt.UTC)
    await pool.execute(
        """INSERT INTO ticker_context
             (symbol, version, structural, structural_as_of,
              narrative,  narrative_as_of,
              market,     market_as_of,
              created_at)
           VALUES ($1, $2, $3::jsonb, $4, $5::jsonb, $4, '{}'::jsonb, NULL, $4)""",
        symbol,
        new_version,
        json.dumps(structural),
        now,
        json.dumps(narrative),
    )
    return new_version


async def refresh(symbol: str, *, limit: int = 50) -> int:
    """Refresh context for `symbol`. Returns the new ticker_context.version."""
    cfg = config.load()
    pool = await asyncpg.create_pool(cfg.database_url, min_size=1, max_size=2)
    assert pool is not None  # asyncpg returns Optional; assert for type-narrowing

    try:
        # 1. Ensure ticker row exists (so the FK on ticker_context.symbol holds).
        await pool.execute(
            "INSERT INTO ticker (symbol) VALUES ($1) ON CONFLICT DO NOTHING",
            symbol,
        )

        # 2. Load prior context (if any) and new events.
        prior = await _load_prior_context(pool, symbol)
        since = dt.datetime.fromisoformat(prior["as_of"]) if prior else None
        events = await _load_events(pool, symbol, since, limit)
        log.info(
            "symbol=%s prior=%s events_count=%d",
            symbol,
            f"v{prior['version']}" if prior else "none",
            len(events),
        )

        if not events and prior is not None:
            log.info("no new events since v%d — skipping refresh", prior["version"])
            return prior["version"]

        # 3. Build prompt + call LLM with audit recorder.
        registry = load(_repo_root() / "prompts")
        prompt = registry.get("synthesize-context")
        if prompt is None:
            raise RuntimeError("prompts/synthesize-context.md missing")

        today = dt.date.today().isoformat()
        user_msg = _build_user_message(symbol, prior, events, today)
        provider = new_provider(_llm_cfg(cfg))
        provider_name = _provider_name(cfg)
        log.info("calling LLM provider=%s model=%s prompt=%s@%s",
                 provider_name, cfg.model_routine, prompt.name, prompt.hash[:12])

        resp = await invoke(
            provider=provider,
            recorder=AsyncpgRecorder(pool),
            prompt=prompt,
            vars={"symbol": symbol, "today": today},
            user_message=user_msg,
            provider_name=provider_name,
            model=cfg.model_routine,
            max_tokens=4096,
        )

        # 4. Parse + persist.
        parsed = _extract_json(resp.content)
        structural = parsed.get("structural", {})
        narrative = parsed.get("narrative", {})
        new_version = await _persist_context(
            pool, symbol, structural, narrative, prior["version"] if prior else None
        )
        log.info(
            "persisted ticker_context v%d for %s (input_tokens=%d output_tokens=%d)",
            new_version, symbol, resp.usage.input_tokens, resp.usage.output_tokens,
        )
        return new_version
    finally:
        await pool.close()


def _repo_root() -> Path:
    """The prompts/ dir sits at repo root. Walk up from this file until found."""
    here = Path(__file__).resolve()
    for parent in (here, *here.parents):
        if (parent / "prompts").is_dir():
            return parent
    raise RuntimeError("could not find prompts/ dir")


def _cli() -> None:
    parser = argparse.ArgumentParser(prog="context_maintainer")
    parser.add_argument("symbol", help="ticker symbol, e.g. NVDA")
    parser.add_argument("--limit", type=int, default=50, help="max events to include in prompt")
    args = parser.parse_args()

    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s %(name)s %(levelname)s %(message)s",
    )
    version = asyncio.run(refresh(args.symbol.upper(), limit=args.limit))
    print(f"ticker_context for {args.symbol.upper()} now at v{version}")


if __name__ == "__main__":
    _cli()
