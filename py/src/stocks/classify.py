"""Discovery classifier (#55). Reads pending discovery_candidate rows + the
user's existing watchlists, asks GLM which list(s) the candidate fits,
persists the proposal into discovery_classification. The UI surfaces it
on the candidate card with a 1-click "confirm to these lists" button.

Usage: python -m stocks.classify [--all | --candidate-id N]
       (--all classifies every candidate that doesn't have a row in
       discovery_classification yet.)
"""

from __future__ import annotations

import argparse
import asyncio
import datetime as dt
import json
import logging
from typing import Any

import asyncpg
import pydantic

from . import config
from .context_maintainer import _llm_cfg, _provider_name, _repo_root  # noqa: PLC2701
from .llm import new_provider
from .prompts import AsyncpgRecorder, invoke_typed, load

log = logging.getLogger("classify")


class ProposedList(pydantic.BaseModel):
    watchlist_id: str | None = None
    watchlist_name: str
    confidence: str
    rationale: str


class SuggestedNewList(pydantic.BaseModel):
    name: str
    description: str
    rationale: str


class ClassifyOutput(pydantic.BaseModel):
    proposed_lists: list[ProposedList]
    suggested_new_list: SuggestedNewList | None = None


async def _load_candidate(pool: asyncpg.Pool, candidate_id: int) -> dict[str, Any] | None:
    row = await pool.fetchrow(
        """SELECT id, symbol, signal_name, signal_value, reasoning, status
             FROM discovery_candidate WHERE id = $1""",
        candidate_id,
    )
    return dict(row) if row else None


async def _load_unclassified(pool: asyncpg.Pool) -> list[dict[str, Any]]:
    rows = await pool.fetch(
        """SELECT dc.id, dc.symbol, dc.signal_name, dc.signal_value, dc.reasoning
             FROM discovery_candidate dc
             LEFT JOIN discovery_classification dcl ON dcl.candidate_id = dc.id
            WHERE dc.status = 'proposed' AND dcl.id IS NULL
         ORDER BY dc.proposed_at DESC
            LIMIT 50""",
    )
    return [dict(r) for r in rows]


async def _load_watchlists(pool: asyncpg.Pool) -> list[dict[str, Any]]:
    rows = await pool.fetch(
        """SELECT w.id::text AS id, w.name, w.description, w.is_system,
                  COUNT(m.symbol) AS member_count
             FROM watchlist w
             LEFT JOIN watchlist_member m ON m.watchlist_id = w.id
         GROUP BY w.id
         ORDER BY w.is_system DESC, w.name ASC""",
    )
    return [dict(r) for r in rows]


async def _load_latest_context(pool: asyncpg.Pool, symbol: str) -> dict | None:
    row = await pool.fetchrow(
        """SELECT version, structural, narrative
             FROM ticker_context WHERE symbol = $1
         ORDER BY version DESC LIMIT 1""",
        symbol,
    )
    if row is None:
        return None

    def _j(v: Any) -> Any:
        return json.loads(v) if isinstance(v, str) else v

    return {
        "version": row["version"],
        "structural": _j(row["structural"]),
        "narrative": _j(row["narrative"]),
    }


async def _load_cluster(pool: asyncpg.Pool, symbol: str) -> str | None:
    return await pool.fetchval("SELECT cluster_id FROM ticker WHERE symbol = $1", symbol)


async def classify_one(pool: asyncpg.Pool, candidate: dict[str, Any]) -> ClassifyOutput:
    """Classify a single candidate. Persists result; returns parsed output."""
    cfg = config.load()
    registry = load(_repo_root() / "prompts")
    prompt = registry.get("classify-watchlist")
    if prompt is None:
        raise RuntimeError("prompts/classify-watchlist.md missing")

    cluster = await _load_cluster(pool, candidate["symbol"])
    ctx = await _load_latest_context(pool, candidate["symbol"])
    watchlists = await _load_watchlists(pool)

    today = dt.date.today().isoformat()
    sv = candidate.get("signal_value")
    user_msg = json.dumps(
        {
            "today": today,
            "candidate": {
                "symbol": candidate["symbol"],
                "signal_name": candidate["signal_name"],
                "signal_value": float(sv) if sv is not None else None,
                "reasoning": candidate["reasoning"],
            },
            "cluster": cluster,
            "latest_context": ctx,
            "watchlists": watchlists,
        },
        default=str, indent=2,
    )
    provider = new_provider(_llm_cfg(cfg))
    provider_name = _provider_name(cfg)
    log.info("classify: candidate=%d symbol=%s provider=%s prompt=%s@%s",
             candidate["id"], candidate["symbol"],
             provider_name, prompt.name, prompt.hash[:12])

    out: ClassifyOutput = await invoke_typed(
        provider=provider,
        recorder=AsyncpgRecorder(pool),
        prompt=prompt,
        vars={"today": today},
        user_message=user_msg,
        provider_name=provider_name,
        model_cls=ClassifyOutput,
        model=cfg.model_routine,
        max_tokens=2048,
        max_retries=2,
    )

    await pool.execute(
        """INSERT INTO discovery_classification
             (candidate_id, proposed_lists, suggested_new_list, prompt_name, prompt_hash)
           VALUES ($1, $2::jsonb, $3::jsonb, $4, $5)""",
        candidate["id"],
        json.dumps([p.model_dump() for p in out.proposed_lists]),
        out.suggested_new_list.model_dump_json() if out.suggested_new_list else None,
        prompt.name,
        prompt.hash,
    )
    log.info("classify: persisted %d proposed list(s)%s",
             len(out.proposed_lists),
             " + new-list suggestion" if out.suggested_new_list else "")
    return out


async def classify_all() -> int:
    cfg = config.load()
    pool = await asyncpg.create_pool(cfg.database_url, min_size=1, max_size=2)
    assert pool is not None
    try:
        candidates = await _load_unclassified(pool)
        log.info("classify_all: %d unclassified candidates", len(candidates))
        for c in candidates:
            try:
                await classify_one(pool, c)
            except Exception:  # noqa: BLE001
                log.exception("classify failed for candidate %s", c["id"])
        return len(candidates)
    finally:
        await pool.close()


async def classify_candidate(candidate_id: int) -> ClassifyOutput | None:
    cfg = config.load()
    pool = await asyncpg.create_pool(cfg.database_url, min_size=1, max_size=2)
    assert pool is not None
    try:
        c = await _load_candidate(pool, candidate_id)
        if c is None:
            raise RuntimeError(f"candidate {candidate_id} not found")
        return await classify_one(pool, c)
    finally:
        await pool.close()


def _cli() -> None:
    parser = argparse.ArgumentParser(prog="classify")
    g = parser.add_mutually_exclusive_group(required=True)
    g.add_argument("--all", action="store_true", help="classify every unclassified candidate")
    g.add_argument("--candidate-id", type=int, help="classify one specific candidate")
    args = parser.parse_args()
    logging.basicConfig(level=logging.INFO,
                        format="%(asctime)s %(name)s %(levelname)s %(message)s")
    if args.all:
        n = asyncio.run(classify_all())
        print(f"classified {n} candidate(s)")
    else:
        out = asyncio.run(classify_candidate(args.candidate_id))
        if out:
            print(out.model_dump_json(indent=2))


if __name__ == "__main__":
    _cli()
