"""Thesis engine — drafts a structured thesis from the latest ticker_context
(SPEC §3 + §5.3, issue #8).

Phase A: single-symbol CLI. Reads the latest ticker_context for SYMBOL,
calls GLM-5.1 with prompts/draft-thesis.md, persists into the `thesis` table
at state=forming with version=1 and immutable_original frozen.

Usage:  python -m stocks.thesis_engine SYMBOL
Example: python -m stocks.thesis_engine NVDA

If the LLM judges that the context is substantial but not actionable, it can
persist a neutral monitoring thesis. If the context is too thin, the run is
honestly logged + skipped. Refusing to draft is a valid output.
"""

from __future__ import annotations

import argparse
import asyncio
import datetime as dt
import json
import logging
import uuid

import asyncpg

from . import config
from .context_maintainer import _llm_cfg, _provider_name, _repo_root  # noqa: PLC2701
from .context_maintainer import refresh as refresh_context
from .evidence import load_open_evidence_requirements
from .llm import new_provider
from .prompts import AsyncpgRecorder, invoke, load

log = logging.getLogger("thesis_engine")


async def _load_latest_context(pool: asyncpg.Pool, symbol: str) -> dict | None:
    row = await pool.fetchrow(
        """SELECT version, structural, narrative, market, created_at
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
        "market": _j(row["market"]),
        "as_of": row["created_at"].isoformat(),
    }


async def _load_prior_thesis(pool: asyncpg.Pool, symbol: str) -> dict | None:
    """Latest non-closed thesis for this symbol, if any."""
    row = await pool.fetchrow(
        """SELECT thesis_id, state, version, edge_rationale, bull_case, bear_case,
                  forecast, conviction_conditions, trigger_conditions,
                  invalidation_conditions, fulfillment_conditions,
                  conviction_tier, instrument
             FROM thesis
            WHERE symbol = $1 AND state NOT IN ('closed', 'disqualified')
         ORDER BY updated_at DESC
            LIMIT 1""",
        symbol,
    )
    if row is None:
        return None
    return {k: row[k] for k in row.keys()}


def _extract_json(content: str) -> dict:
    s = content.strip()
    try:
        return json.loads(s)
    except json.JSONDecodeError:
        pass
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
    start = s.find("{")
    end = s.rfind("}")
    if start >= 0 and end > start:
        return json.loads(s[start:end + 1])
    raise ValueError(f"could not parse JSON from LLM response: {s[:200]}")


async def _persist_thesis(
    pool: asyncpg.Pool,
    symbol: str,
    draft: dict,
) -> uuid.UUID:
    """Insert a fresh thesis row at state=forming, v1, with immutable_original
    frozen. Returns the new thesis_id."""
    # Look up cluster from ticker (FK target).
    cluster_id = await pool.fetchval(
        "SELECT cluster_id FROM ticker WHERE symbol = $1", symbol,
    )
    forecast = draft.get("forecast") or {}
    intended_size = (
        {"pct": draft["intended_size_pct"]}
        if draft.get("intended_size_pct") is not None
        else None
    )
    immutable_original = {
        "edge_rationale": draft.get("edge_rationale") or "",
        "invalidation_conditions": draft.get("invalidation_conditions") or [],
        "thesis_kind": draft.get("thesis_kind") or "actionable_edge",
        "no_edge_reason": draft.get("no_edge_reason"),
        "drafted_at": dt.datetime.now(dt.UTC).isoformat(),
    }
    thesis_id = uuid.uuid4()
    await pool.execute(
        """INSERT INTO thesis
             (thesis_id, symbol, cluster_id, cluster_thesis, state,
              bull_case, bear_case, edge_rationale,
              forecast,
              conviction_conditions, trigger_conditions,
              invalidation_conditions, fulfillment_conditions,
              conviction_tier, instrument, intended_size,
              version, immutable_original)
           VALUES ($1, $2, $3, $4, 'forming',
                   $5, $6, $7,
                   $8::jsonb,
                   $9::jsonb, $10::jsonb,
                   $11::jsonb, $12::jsonb,
                   $13, $14, $15::jsonb,
                   1, $16::jsonb)""",
        thesis_id,
        symbol,
        cluster_id,
        draft.get("cluster_thesis"),
        draft.get("bull_case"),
        draft.get("bear_case"),
        draft.get("edge_rationale") or "(LLM declined to draft an edge rationale)",
        json.dumps(forecast),
        json.dumps(draft.get("conviction_conditions") or []),
        json.dumps(draft.get("trigger_conditions") or []),
        json.dumps(draft.get("invalidation_conditions") or []),
        json.dumps(draft.get("fulfillment_conditions") or []),
        draft.get("conviction_tier"),
        draft.get("instrument"),
        json.dumps(intended_size) if intended_size else None,
        json.dumps(immutable_original),
    )
    return thesis_id


async def _resolve_stale_incomplete_attention(
    pool: asyncpg.Pool,
    symbol: str,
    thesis_id: uuid.UUID,
) -> None:
    """A successful draft supersedes prior no-thesis attention for the symbol."""
    await pool.execute(
        """UPDATE attention_item
              SET status = 'resolved',
                  resolved_at = now(),
                  resolution_kind = 'thesis_drafted',
                  source_ref = source_ref || $2::jsonb
            WHERE status = 'open'
              AND kind = 'thesis_incomplete'
              AND symbol = $1""",
        symbol,
        json.dumps({"resolved_by_thesis_id": str(thesis_id)}),
    )


def _context_has_substance(context: dict | None) -> bool:
    if not context:
        return False
    for band in ("structural", "narrative", "market"):
        value = context.get(band)
        if isinstance(value, dict):
            for item in value.values():
                if item not in (None, "", [], {}):
                    return True
        elif value not in (None, "", [], {}):
            return True
    return False


def _draft_kind(parsed: dict, context: dict | None) -> str:
    explicit = parsed.get("thesis_kind")
    if explicit in {"actionable_edge", "monitoring", "decline"}:
        return explicit
    if parsed.get("edge_present"):
        return "actionable_edge"
    return "monitoring" if _context_has_substance(context) else "decline"


def _normalize_monitoring_draft(symbol: str, parsed: dict) -> dict:
    """Make a no-edge but substantial-context result persistable as a thesis.

    This keeps the operator from seeing a blank thesis panel for important
    tracked names while preserving the distinction between "monitoring" and
    "actionable edge".
    """
    reason = parsed.get("no_edge_reason") or "No actionable information-diffusion edge is present."
    out = dict(parsed)
    out["thesis_kind"] = "monitoring"
    out["edge_present"] = False
    out["edge_rationale"] = out.get("edge_rationale") or f"Monitoring thesis for {symbol}: {reason}"
    out["bull_case"] = out.get("bull_case") or (
        "Base case remains constructive if upcoming company updates confirm "
        "that current demand, margin, and competitive-position assumptions are intact."
    )
    out["bear_case"] = out.get("bear_case") or (
        "The monitoring case weakens if new filings, estimates, news, or price action "
        "show that consensus expectations are too high or the competitive setup is deteriorating."
    )
    out["forecast"] = out.get("forecast") or {
        "direction": "neutral",
        "magnitude_rough": "flat to low single digits",
        "horizon_days": 90,
        "horizon_event": "next material company update",
    }
    out["conviction_conditions"] = out.get("conviction_conditions") or []
    out["trigger_conditions"] = out.get("trigger_conditions") or []
    out["invalidation_conditions"] = out.get("invalidation_conditions") or []
    out["fulfillment_conditions"] = out.get("fulfillment_conditions") or []
    out["missing_evidence"] = out.get("missing_evidence") or []
    out["conviction_tier"] = out.get("conviction_tier") or "low"
    out["instrument"] = out.get("instrument") or "equity"
    return out


async def draft(symbol: str) -> dict:
    """Draft a thesis for SYMBOL. Returns the parsed LLM output (and persists
    a thesis row if edge_present=true)."""
    cfg = config.load()
    pool = await asyncpg.create_pool(cfg.database_url, min_size=1, max_size=2)
    assert pool is not None

    try:
        context = await _load_latest_context(pool, symbol)
        if context is None:
            # Auto-synthesize. The thesis pipeline owns its inputs — operators
            # shouldn't see "run make refresh-context" instructions.
            log.info("no context for %s; synthesizing inline", symbol)
            try:
                await refresh_context(symbol)
            except Exception:  # noqa: BLE001
                log.exception("inline context refresh failed for %s", symbol)
            context = await _load_latest_context(pool, symbol)
            if context is None:
                # Honest decline — return a structured no-edge payload instead
                # of raising, so the cognition consumer can persist a
                # thesis_incomplete attention item with a real reason.
                return {
                    "edge_present": False,
                    "no_edge_reason": (
                        f"context synthesis failed for {symbol} "
                        "(insufficient ingested evidence)"
                    ),
                }
        prior = await _load_prior_thesis(pool, symbol)
        missing_evidence = await load_open_evidence_requirements(pool, symbol)
        if prior is not None:
            log.info(
                "found prior thesis %s v%d state=%s — drafting fresh anyway "
                "(version policy in #15)",
                prior["thesis_id"], prior["version"], prior["state"],
            )

        # Render prompt + call LLM.
        registry = load(_repo_root() / "prompts")
        prompt = registry.get("draft-thesis")
        if prompt is None:
            raise RuntimeError("prompts/draft-thesis.md missing")

        today = dt.date.today().isoformat()
        user_msg = json.dumps(
            {
                "symbol": symbol,
                "today": today,
                "context": context,
                "missing_evidence": missing_evidence,
                "cluster_thesis": None,  # populated when #1 cluster-thesis work lands
                "prior_thesis": _summarize_prior(prior) if prior else None,
            },
            default=str,
            indent=2,
        )
        provider = new_provider(_llm_cfg(cfg))
        provider_name = _provider_name(cfg)
        log.info(
            "calling LLM provider=%s model=%s prompt=%s@%s",
            provider_name, cfg.model_deep, prompt.name, prompt.hash[:12],
        )

        resp = await invoke(
            provider=provider,
            recorder=AsyncpgRecorder(pool),
            prompt=prompt,
            vars={"symbol": symbol, "today": today},
            user_message=user_msg,
            provider_name=provider_name,
            model=cfg.model_deep,
            max_tokens=4096,
        )
        parsed = _extract_json(resp.content)

        kind = _draft_kind(parsed, context)
        if kind == "monitoring":
            parsed = _normalize_monitoring_draft(symbol, parsed)
        elif kind == "decline":
            log.warning(
                "LLM declined to draft (edge_present=false): %s",
                parsed.get("no_edge_reason", "(no reason given)"),
            )
            return parsed

        thesis_id = await _persist_thesis(pool, symbol, parsed)
        await _resolve_stale_incomplete_attention(pool, symbol, thesis_id)
        log.info(
            "persisted thesis %s for %s at state=forming (input=%d output=%d)",
            thesis_id, symbol, resp.usage.input_tokens, resp.usage.output_tokens,
        )
        parsed["_thesis_id"] = str(thesis_id)
        return parsed
    finally:
        await pool.close()


def _summarize_prior(prior: dict) -> dict:
    """Pass the prior thesis to the LLM in compact form so it can avoid
    contradicting itself unnecessarily."""
    return {
        "state": prior["state"],
        "version": prior["version"],
        "edge_rationale": prior["edge_rationale"],
        "invalidation_conditions": _maybe_json(prior["invalidation_conditions"]),
    }


def _maybe_json(v):
    if isinstance(v, (str, bytes)):
        try:
            return json.loads(v)
        except Exception:  # noqa: BLE001
            return v
    return v


def _cli() -> None:
    parser = argparse.ArgumentParser(prog="thesis_engine")
    parser.add_argument("symbol", help="ticker symbol, e.g. NVDA")
    args = parser.parse_args()
    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s %(name)s %(levelname)s %(message)s",
    )
    out = asyncio.run(draft(args.symbol.upper()))
    print(json.dumps(out, indent=2))


if __name__ == "__main__":
    _cli()


__all__ = ["draft"]
