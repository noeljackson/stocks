"""Challenge pass (#13). Adversarial reading of a thesis — surfaces specific
weak spots as `flag` rows in thesis_suggestion. Never blocks promotion;
the user reviews and either addresses or dismisses each flag.

Usage: python -m stocks.challenge <thesis_id>
"""

from __future__ import annotations

import argparse
import asyncio
import datetime as dt
import json
import logging

import asyncpg
import pydantic

from . import config
from .context_maintainer import _llm_cfg, _provider_name, _repo_root  # noqa: PLC2701
from .llm import new_provider
from .prompts import AsyncpgRecorder, invoke_typed, load
from .sharpen import _load_thesis

log = logging.getLogger("challenge")


class Flag(pydantic.BaseModel):
    kind: str
    claim: str
    why: str
    suggested_fix: str


class ChallengeOutput(pydantic.BaseModel):
    flags: list[Flag]


async def challenge(thesis_id: str) -> ChallengeOutput:
    cfg = config.load()
    pool = await asyncpg.create_pool(cfg.database_url, min_size=1, max_size=2)
    assert pool is not None
    try:
        thesis = await _load_thesis(pool, thesis_id)
        if thesis is None:
            raise RuntimeError(f"thesis {thesis_id} not found")

        registry = load(_repo_root() / "prompts")
        prompt = registry.get("challenge-thesis")
        if prompt is None:
            raise RuntimeError("prompts/challenge-thesis.md missing")

        today = dt.date.today().isoformat()
        user_msg = json.dumps(
            {"symbol": thesis["symbol"], "today": today, "thesis": thesis},
            default=str, indent=2,
        )
        provider = new_provider(_llm_cfg(cfg))
        provider_name = _provider_name(cfg)
        log.info("challenge: calling LLM provider=%s prompt=%s@%s",
                 provider_name, prompt.name, prompt.hash[:12])

        out: ChallengeOutput = await invoke_typed(
            provider=provider,
            recorder=AsyncpgRecorder(pool),
            prompt=prompt,
            vars={"symbol": thesis["symbol"], "today": today},
            user_message=user_msg,
            provider_name=provider_name,
            model_cls=ChallengeOutput,
            model=cfg.model_deep,
            max_tokens=4096,
            max_retries=2,
        )

        for f in out.flags:
            await pool.execute(
                """INSERT INTO thesis_suggestion
                     (thesis_id, kind, content, prompt_name, prompt_hash)
                   VALUES ($1, 'flag', $2::jsonb, $3, $4)""",
                thesis_id,
                f.model_dump_json(),
                prompt.name,
                prompt.hash,
            )
        log.info("challenge: persisted %d flag(s)", len(out.flags))
        return out
    finally:
        await pool.close()


def _cli() -> None:
    parser = argparse.ArgumentParser(prog="challenge")
    parser.add_argument("thesis_id")
    args = parser.parse_args()
    logging.basicConfig(level=logging.INFO,
                        format="%(asctime)s %(name)s %(levelname)s %(message)s")
    out = asyncio.run(challenge(args.thesis_id))
    print(out.model_dump_json(indent=2))


if __name__ == "__main__":
    _cli()
