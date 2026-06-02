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


async def _load_parent_theses(pool: asyncpg.Pool, symbol: str) -> list[dict]:
    rows = await pool.fetch(
        """SELECT bt.scope, bt.key, bt.name, bt.state, bt.direction,
                  bt.summary, bt.core_claim, bt.why_now,
                  bt.invalidation_conditions, bt.missing_evidence,
                  bt.open_questions, bt.last_evaluated_at,
                  btt.role, btt.rationale, btt.conviction
             FROM brain_thesis_ticker btt
             JOIN brain_thesis bt ON bt.id = btt.brain_thesis_id
            WHERE btt.symbol = $1
              AND bt.active = true
         ORDER BY CASE bt.scope WHEN 'macro' THEN 0 WHEN 'sector' THEN 1 ELSE 2 END,
                  COALESCE(btt.conviction, 0) DESC,
                  bt.name""",
        symbol,
    )
    out = []
    for row in rows:
        item = {k: row[k] for k in row.keys()}
        if item.get("last_evaluated_at"):
            item["last_evaluated_at"] = item["last_evaluated_at"].isoformat()
        out.append(item)
    return out


async def _load_evidence_items(
    pool: asyncpg.Pool,
    symbol: str,
    limit: int = 60,
) -> list[dict]:
    rows = await pool.fetch(
        """SELECT id, kind, observed_at, source, source_id, source_ref,
                  summary, strength, polarity, url
             FROM evidence_item
            WHERE symbol = $1
              AND NOT (
                  kind = 'product_research'
                  AND source = 'web_research'
                  AND COALESCE((source_ref->'relevance'->>'accepted')::boolean, false) = false
              )
         ORDER BY observed_at DESC, id DESC
            LIMIT $2""",
        symbol,
        limit,
    )
    out = []
    for row in rows:
        source_ref = row["source_ref"]
        if isinstance(source_ref, str):
            source_ref = json.loads(source_ref)
        out.append({
            "id": row["id"],
            "kind": row["kind"],
            "observed_at": row["observed_at"].isoformat(),
            "source": row["source"],
            "source_id": row["source_id"],
            "summary": row["summary"],
            "strength": None if row["strength"] is None else float(row["strength"]),
            "polarity": None if row["polarity"] is None else float(row["polarity"]),
            "url": row["url"],
            "source_ref": source_ref,
        })
    return out


def _evidence_weight(item: dict) -> float | None:
    strength = item.get("strength")
    polarity = item.get("polarity")
    if isinstance(strength, (int, float)):
        return max(0.0, min(1.0, float(strength)))
    if isinstance(polarity, (int, float)):
        return max(0.0, min(1.0, abs(float(polarity))))
    return None


async def _link_thesis_evidence(
    pool: asyncpg.Pool,
    thesis_id: uuid.UUID,
    evidence_items: list[dict],
) -> None:
    rows = [
        (thesis_id, int(item["id"]), _evidence_weight(item), "system")
        for item in evidence_items[:25]
        if item.get("id") is not None
    ]
    if not rows:
        return
    await pool.executemany(
        """INSERT INTO thesis_evidence (thesis_id, evidence_id, weight, added_by)
           VALUES ($1, $2, $3, $4)
           ON CONFLICT (thesis_id, evidence_id) DO UPDATE SET
             weight = GREATEST(
               COALESCE(thesis_evidence.weight, 0.0),
               COALESCE(EXCLUDED.weight, 0.0)
             ),
             added_by = EXCLUDED.added_by""",
        rows,
    )


def _extract_json(content: str) -> dict:
    s = content.strip()
    clean_error = ""
    try:
        return json.loads(s)
    except json.JSONDecodeError as e:
        clean_error = str(e)
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
        try:
            return json.loads(s[start:end + 1])
        except json.JSONDecodeError as e:
            raise ValueError(
                f"could not parse JSON object from LLM response: {e.msg} "
                f"at line {e.lineno} column {e.colno}: {s[:200]}"
            ) from e
    raise ValueError(f"could not parse JSON from LLM response: {clean_error}: {s[:200]}")


async def _invoke_draft_json(
    *,
    provider,
    pool: asyncpg.Pool,
    prompt,
    user_msg: str,
    provider_name: str,
    model: str,
    symbol: str,
    today: str,
    max_retries: int = 2,
):
    current_user = user_msg
    last_error: Exception | None = None
    for attempt in range(max_retries + 1):
        resp = await invoke(
            provider=provider,
            recorder=AsyncpgRecorder(pool),
            prompt=prompt,
            vars={"symbol": symbol, "today": today},
            user_message=current_user,
            provider_name=provider_name,
            model=model,
            max_tokens=4096,
        )
        try:
            return _extract_json(resp.content), resp
        except ValueError as e:
            last_error = e
            if attempt >= max_retries:
                raise RuntimeError(
                    "draft-thesis returned invalid JSON after "
                    f"{max_retries + 1} attempts: {e}"
                ) from e
            log.warning(
                "draft-thesis JSON parse failed for %s (attempt %d/%d); retrying: %s",
                symbol,
                attempt + 1,
                max_retries + 1,
                e,
            )
            current_user = (
                f"{user_msg}\n\n"
                "[Previous draft-thesis response was invalid JSON. "
                "Reply ONLY with one complete valid JSON object matching the "
                "draft-thesis prompt contract. Do not include prose or markdown "
                f"fences. JSON parse error: {e}.]\n\n"
                "[Invalid response excerpt]\n"
                f"{resp.content[:3000]}"
            )
    raise RuntimeError(f"draft-thesis retry loop exited unexpectedly: {last_error}")


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
              version, immutable_original, last_evaluated_at)
           VALUES ($1, $2, $3, $4, 'forming',
                   $5, $6, $7,
                   $8::jsonb,
                   $9::jsonb, $10::jsonb,
                   $11::jsonb, $12::jsonb,
                   $13, $14, $15::jsonb,
                   1, $16::jsonb, now())""",
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


def _maybe_json(v):
    if isinstance(v, (str, bytes)):
        try:
            return json.loads(v)
        except Exception:  # noqa: BLE001
            return v
    return v


def _condition_names(value) -> set[str]:
    out = set()
    decoded = _maybe_json(value) or []
    if not isinstance(decoded, list):
        return out
    for item in decoded:
        if isinstance(item, dict) and item.get("name"):
            out.add(str(item["name"]))
    return out


def _forecast_direction(value) -> str | None:
    decoded = _maybe_json(value) or {}
    if isinstance(decoded, dict):
        direction = decoded.get("direction")
        return direction if isinstance(direction, str) else None
    return None


def _tier_rank(value: str | None) -> int:
    return {"low": 0, "medium": 1, "high": 2}.get(value or "", -1)


def classify_reconciliation(prior: dict, draft: dict) -> tuple[str, bool]:
    prior_inv = _condition_names(prior.get("invalidation_conditions"))
    draft_inv = _condition_names(draft.get("invalidation_conditions"))
    dropped_invalidation = bool(prior_inv - draft_inv)
    if dropped_invalidation:
        return "weakened_view", True

    prior_direction = _forecast_direction(prior.get("forecast"))
    draft_direction = _forecast_direction(draft.get("forecast"))
    if prior_direction and draft_direction and prior_direction != draft_direction:
        return "material_change", False

    prior_edge = (prior.get("edge_rationale") or "").strip()
    draft_edge = (draft.get("edge_rationale") or "").strip()
    if prior_edge == draft_edge and prior_direction == draft_direction:
        return "no_change", False

    prior_rank = _tier_rank(prior.get("conviction_tier"))
    draft_rank = _tier_rank(draft.get("conviction_tier"))
    if draft_rank > prior_rank:
        return "strengthened_view", False
    if draft_rank < prior_rank:
        return "weakened_view", False
    return "confirmed_existing_view", False


def _draft_snapshot(draft: dict) -> dict:
    return {
        "thesis_kind": draft.get("thesis_kind"),
        "edge_present": draft.get("edge_present"),
        "edge_rationale": draft.get("edge_rationale"),
        "bull_case": draft.get("bull_case"),
        "bear_case": draft.get("bear_case"),
        "forecast": draft.get("forecast") or {},
        "conviction_conditions": draft.get("conviction_conditions") or [],
        "trigger_conditions": draft.get("trigger_conditions") or [],
        "invalidation_conditions": draft.get("invalidation_conditions") or [],
        "fulfillment_conditions": draft.get("fulfillment_conditions") or [],
        "conviction_tier": draft.get("conviction_tier"),
        "instrument": draft.get("instrument"),
        "missing_evidence": draft.get("missing_evidence") or [],
    }


def _prior_snapshot(prior: dict) -> dict:
    return {
        "thesis_id": str(prior["thesis_id"]),
        "state": prior["state"],
        "version": prior["version"],
        "edge_rationale": prior.get("edge_rationale"),
        "bull_case": prior.get("bull_case"),
        "bear_case": prior.get("bear_case"),
        "forecast": _maybe_json(prior.get("forecast")) or {},
        "conviction_conditions": _maybe_json(prior.get("conviction_conditions")) or [],
        "trigger_conditions": _maybe_json(prior.get("trigger_conditions")) or [],
        "invalidation_conditions": _maybe_json(prior.get("invalidation_conditions")) or [],
        "fulfillment_conditions": _maybe_json(prior.get("fulfillment_conditions")) or [],
        "conviction_tier": prior.get("conviction_tier"),
        "instrument": prior.get("instrument"),
    }


async def _reconcile_existing_thesis(
    pool: asyncpg.Pool,
    prior: dict,
    draft: dict,
    context: dict | None,
) -> uuid.UUID:
    classification, weakens = classify_reconciliation(prior, draft)
    thesis_id = prior["thesis_id"]
    next_version = int(prior["version"] or 1) + 1
    intended_size = (
        {"pct": draft["intended_size_pct"]}
        if draft.get("intended_size_pct") is not None
        else None
    )
    diff = {
        "event": "thesis_reconciliation",
        "classification": classification,
        "operator_action_required": classification
        in {"weakened_view", "material_change", "invalidates_existing_view"},
        "prior": _prior_snapshot(prior),
        "draft": _draft_snapshot(draft),
        "context": {
            "version": context.get("version") if context else None,
            "as_of": context.get("as_of") if context else None,
        },
    }

    async with pool.acquire() as conn:
        async with conn.transaction():
            if classification == "no_change":
                await conn.execute(
                    """UPDATE thesis
                          SET last_evaluated_at = now()
                        WHERE thesis_id = $1""",
                    thesis_id,
                )
                return thesis_id
            await conn.execute(
                """UPDATE thesis
                      SET cluster_thesis = $2,
                          bull_case = $3,
                          bear_case = $4,
                          edge_rationale = $5,
                          forecast = $6::jsonb,
                          conviction_conditions = $7::jsonb,
                          trigger_conditions = $8::jsonb,
                          invalidation_conditions = $9::jsonb,
                          fulfillment_conditions = $10::jsonb,
                          conviction_tier = $11,
                          instrument = $12,
                          intended_size = $13::jsonb,
                          version = $14,
                          updated_at = now(),
                          last_evaluated_at = now()
                    WHERE thesis_id = $1""",
                thesis_id,
                draft.get("cluster_thesis"),
                draft.get("bull_case"),
                draft.get("bear_case"),
                draft.get("edge_rationale") or "(LLM declined to draft an edge rationale)",
                json.dumps(draft.get("forecast") or {}),
                json.dumps(draft.get("conviction_conditions") or []),
                json.dumps(draft.get("trigger_conditions") or []),
                json.dumps(draft.get("invalidation_conditions") or []),
                json.dumps(draft.get("fulfillment_conditions") or []),
                draft.get("conviction_tier"),
                draft.get("instrument"),
                json.dumps(intended_size) if intended_size else None,
                next_version,
            )
            await conn.execute(
                """INSERT INTO thesis_version_history
                     (thesis_id, version, diff, rationale, weakens_invalidation)
                   VALUES ($1, $2, $3::jsonb, $4, $5)""",
                thesis_id,
                next_version,
                json.dumps(diff, default=str),
                f"Reconciled fresh draft against active thesis: {classification}",
                weakens,
            )
    return thesis_id


async def _record_decline_reconciliation(
    pool: asyncpg.Pool,
    prior: dict,
    parsed: dict,
    context: dict | None,
) -> None:
    thesis_id = prior["thesis_id"]
    next_version = int(prior["version"] or 1) + 1
    diff = {
        "event": "thesis_reconciliation",
        "classification": "invalidates_existing_view",
        "operator_action_required": True,
        "prior": _prior_snapshot(prior),
        "decline": {
            "no_edge_reason": parsed.get("no_edge_reason"),
            "missing_evidence": parsed.get("missing_evidence") or [],
        },
        "context": {
            "version": context.get("version") if context else None,
            "as_of": context.get("as_of") if context else None,
        },
    }
    await pool.execute(
        """WITH updated AS (
               UPDATE thesis
                  SET version = $2,
                      updated_at = now(),
                      last_evaluated_at = now()
                WHERE thesis_id = $1
              RETURNING thesis_id
           )
           INSERT INTO thesis_version_history
             (thesis_id, version, diff, rationale, weakens_invalidation)
           SELECT thesis_id, $2, $3::jsonb, $4, true FROM updated""",
        thesis_id,
        next_version,
        json.dumps(diff, default=str),
        "Fresh draft declined against active thesis: invalidates_existing_view",
    )


async def _resolve_stale_incomplete_attention(
    pool: asyncpg.Pool,
    symbol: str,
    thesis_id: uuid.UUID,
) -> None:
    """A successful draft supersedes prior no-thesis attention for the symbol."""
    await pool.execute(
        """WITH matched AS (
               SELECT id, fsm_state
                 FROM attention_item
                WHERE status = 'open'
                  AND kind = 'thesis_incomplete'
                  AND symbol = $1
                FOR UPDATE
           ),
           updated AS (
               UPDATE attention_item ai
                  SET status = 'resolved',
                      fsm_state = 'resolved',
                      owner = 'cognition',
                      resolved_at = now(),
                      resolution_kind = 'thesis_drafted',
                      resolution_ref = $2::jsonb,
                      source_ref = source_ref || $2::jsonb,
                      next_retry_at = NULL,
                      resurface_at = NULL,
                      state_reason = 'thesis_drafted'
                 FROM matched m
                WHERE ai.id = m.id
            RETURNING ai.id,
                      m.fsm_state AS from_state,
                      ai.fsm_state AS to_state,
                      ai.owner,
                      ai.state_reason,
                      ai.next_retry_at,
                      ai.resurface_at,
                      ai.resolution_ref
           ),
           inserted AS (
               INSERT INTO attention_state_history
                    (attention_id, from_state, to_state, owner, reason,
                     next_retry_at, resurface_at, source_ref)
               SELECT id, from_state, to_state, owner, state_reason,
                      next_retry_at, resurface_at, resolution_ref
                 FROM updated
            RETURNING 1
           )
           SELECT count(*) FROM updated""",
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
        parent_theses = await _load_parent_theses(pool, symbol)
        missing_evidence = await load_open_evidence_requirements(pool, symbol)
        evidence_items = await _load_evidence_items(pool, symbol)
        if prior is not None:
            log.info(
                "found prior thesis %s v%d state=%s — drafting reconciliation",
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
                "parent_theses": parent_theses,
                "evidence_items": evidence_items,
                "cluster_thesis": parent_theses[0]["summary"] if parent_theses else None,
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

        parsed, resp = await _invoke_draft_json(
            provider=provider,
            pool=pool,
            prompt=prompt,
            user_msg=user_msg,
            provider_name=provider_name,
            model=cfg.model_deep,
            symbol=symbol,
            today=today,
        )

        kind = _draft_kind(parsed, context)
        if kind == "monitoring":
            parsed = _normalize_monitoring_draft(symbol, parsed)
        elif kind == "decline":
            log.warning(
                "LLM declined to draft (edge_present=false): %s",
                parsed.get("no_edge_reason", "(no reason given)"),
            )
            if prior is not None:
                await _record_decline_reconciliation(pool, prior, parsed, context)
                await _link_thesis_evidence(pool, prior["thesis_id"], evidence_items)
                parsed["_reconciled_existing_thesis"] = True
                parsed["_reconciliation_classification"] = "invalidates_existing_view"
            return parsed

        if prior is not None:
            thesis_id = await _reconcile_existing_thesis(pool, prior, parsed, context)
            parsed["_reconciled_existing_thesis"] = True
            parsed["_reconciliation_classification"] = classify_reconciliation(prior, parsed)[0]
            await _resolve_stale_incomplete_attention(pool, symbol, thesis_id)
        else:
            thesis_id = await _persist_thesis(pool, symbol, parsed)
            await _resolve_stale_incomplete_attention(pool, symbol, thesis_id)
        await _link_thesis_evidence(pool, thesis_id, evidence_items)
        log.info(
            "persisted/reconciled thesis %s for %s (input=%d output=%d)",
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
