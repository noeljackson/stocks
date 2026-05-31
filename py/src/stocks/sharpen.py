"""Sharpen pass (#12). Reads a thesis's prose + current conditions, asks
GLM to propose well-formed Condition entries for verifiable claims that
aren't currently tracked. Output persisted to thesis_suggestion table as
status='pending' — user reviews/accepts/dismisses in the UI.

Usage: python -m stocks.sharpen <thesis_id>
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

log = logging.getLogger("sharpen")


class Target(pydantic.BaseModel):
    metric: str
    op: str
    value: float
    unit: str | None = None


class Condition(pydantic.BaseModel):
    type: str
    name: str
    expr: str | None = None
    assertion: str | None = None
    target: Target
    deadline_at: str
    evidence_source: str


class Suggestion(pydantic.BaseModel):
    role: str
    condition: Condition
    rationale: str
    supersedes: str | None = None


class SharpenOutput(pydantic.BaseModel):
    suggestions: list[Suggestion]


async def _load_thesis(pool: asyncpg.Pool, thesis_id: str) -> dict | None:
    row = await pool.fetchrow(
        """SELECT thesis_id, symbol, edge_rationale, bull_case, bear_case,
                  conviction_conditions, trigger_conditions,
                  invalidation_conditions, fulfillment_conditions
             FROM thesis WHERE thesis_id = $1""",
        thesis_id,
    )
    if row is None:
        return None

    def _j(v: Any) -> Any:
        return json.loads(v) if isinstance(v, str) else v

    return {
        "thesis_id": str(row["thesis_id"]),
        "symbol": row["symbol"],
        "edge_rationale": row["edge_rationale"],
        "bull_case": row["bull_case"],
        "bear_case": row["bear_case"],
        "conviction_conditions": _j(row["conviction_conditions"]) or [],
        "trigger_conditions": _j(row["trigger_conditions"]) or [],
        "invalidation_conditions": _j(row["invalidation_conditions"]) or [],
        "fulfillment_conditions": _j(row["fulfillment_conditions"]) or [],
    }


async def sharpen(thesis_id: str) -> SharpenOutput:
    """Run the sharpen pass; persist each suggestion as a thesis_suggestion
    row with status='pending'. Returns the parsed output."""
    cfg = config.load()
    pool = await asyncpg.create_pool(cfg.database_url, min_size=1, max_size=2)
    assert pool is not None
    try:
        thesis = await _load_thesis(pool, thesis_id)
        if thesis is None:
            raise RuntimeError(f"thesis {thesis_id} not found")

        registry = load(_repo_root() / "prompts")
        prompt = registry.get("sharpen-thesis")
        if prompt is None:
            raise RuntimeError("prompts/sharpen-thesis.md missing")

        today = dt.date.today().isoformat()
        user_msg = json.dumps(
            {"symbol": thesis["symbol"], "today": today, "thesis": thesis},
            default=str, indent=2,
        )
        provider = new_provider(_llm_cfg(cfg))
        provider_name = _provider_name(cfg)
        log.info("sharpen: calling LLM provider=%s prompt=%s@%s",
                 provider_name, prompt.name, prompt.hash[:12])

        out: SharpenOutput = await invoke_typed(
            provider=provider,
            recorder=AsyncpgRecorder(pool),
            prompt=prompt,
            vars={"symbol": thesis["symbol"], "today": today},
            user_message=user_msg,
            provider_name=provider_name,
            model_cls=SharpenOutput,
            model=cfg.model_deep,
            max_tokens=4096,
            max_retries=2,
        )

        # Persist each suggestion.
        for s in out.suggestions:
            await pool.execute(
                """INSERT INTO thesis_suggestion
                     (thesis_id, kind, content, prompt_name, prompt_hash)
                   VALUES ($1, 'condition_suggestion', $2::jsonb, $3, $4)""",
                thesis_id,
                s.model_dump_json(),
                prompt.name,
                prompt.hash,
            )
        log.info("sharpen: persisted %d suggestion(s)", len(out.suggestions))
        return out
    finally:
        await pool.close()


def _cli() -> None:
    parser = argparse.ArgumentParser(prog="sharpen")
    parser.add_argument("thesis_id")
    args = parser.parse_args()
    logging.basicConfig(level=logging.INFO,
                        format="%(asctime)s %(name)s %(levelname)s %(message)s")
    out = asyncio.run(sharpen(args.thesis_id))
    print(out.model_dump_json(indent=2))


if __name__ == "__main__":
    _cli()
