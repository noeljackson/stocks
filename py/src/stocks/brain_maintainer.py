"""Maintains first-class macro/sector/theme brain theses.

This is the deterministic first slice of the parent-brain loop. It keeps the
seeded `brain_thesis` rows alive by evaluating source freshness and linked
ticker coverage on the same schedule as ticker cognition. It does not invent a
new macro or sector claim; it records what the system can currently support and
leaves missing parent evidence visible.
"""

from __future__ import annotations

import argparse
import asyncio
import datetime as dt
import hashlib
import json
import logging
import re
from typing import Any

import asyncpg

from . import config

log = logging.getLogger("brain_maintainer")

SYMBOL_RE = re.compile(r"^[A-Z][A-Z0-9.\-]{0,9}$")
SOURCE_FRESHNESS_MINUTES = 90
COMMODITY_REQUIREMENT_PARTS = (
    "commodity",
    "inventory",
    "inventories",
    "usda",
    "weather",
    "crop",
    "china_demand",
    "cot",
)
RESEARCH_REQUIREMENT_PARTS = (
    "research",
    "customer",
    "design",
    "backlog",
    "pricing",
    "capacity",
    "transcript",
)


def _parse_dt(value: Any) -> dt.datetime | None:
    if value is None:
        return None
    if isinstance(value, dt.datetime):
        return value if value.tzinfo else value.replace(tzinfo=dt.UTC)
    if isinstance(value, str):
        try:
            parsed = dt.datetime.fromisoformat(value.replace("Z", "+00:00"))
        except ValueError:
            return None
        return parsed if parsed.tzinfo else parsed.replace(tzinfo=dt.UTC)
    return None


def _json(value: Any, fallback: Any) -> Any:
    if value is None:
        return fallback
    if isinstance(value, str):
        try:
            return json.loads(value)
        except json.JSONDecodeError:
            return fallback
    return value


def _iso(value: Any) -> str | None:
    parsed = _parse_dt(value)
    return parsed.isoformat() if parsed else None


def normalize_symbol(value: Any) -> str | None:
    if not isinstance(value, str):
        return None
    symbol = value.strip().upper().removeprefix("$")
    return symbol if SYMBOL_RE.match(symbol) else None


def symbols_from_json(value: Any) -> list[str]:
    decoded = _json(value, [])
    if not isinstance(decoded, list):
        return []
    out: list[str] = []
    seen: set[str] = set()
    for item in decoded:
        symbol = normalize_symbol(item)
        if symbol and symbol not in seen:
            seen.add(symbol)
            out.append(symbol)
    return out


def _stable_fingerprint(payload: dict[str, Any]) -> str:
    raw = json.dumps(payload, sort_keys=True, separators=(",", ":"), default=str)
    return hashlib.sha256(raw.encode("utf-8")).hexdigest()[:16]


def _source_snapshot(
    source_health: dict[str, dict[str, Any]],
    source: str,
    *,
    now: dt.datetime,
    max_age_minutes: int = SOURCE_FRESHNESS_MINUTES,
) -> dict[str, Any]:
    row = source_health.get(source)
    if not row:
        return {
            "source": source,
            "status": "missing",
            "freshness": "missing",
            "last_checked_at": None,
            "rows_seen": 0,
            "rows_inserted": 0,
        }
    last_checked_at = (
        _parse_dt(row.get("last_success_at"))
        or _parse_dt(row.get("last_started_at"))
        or _parse_dt(row.get("updated_at"))
    )
    if row.get("last_failure_kind") == "rate_limited":
        freshness = "rate_limited"
    elif last_checked_at is None:
        freshness = "missing"
    elif last_checked_at < now - dt.timedelta(minutes=max_age_minutes):
        freshness = "stale"
    else:
        freshness = "fresh"
    return {
        "source": source,
        "status": row.get("last_status") or "unknown",
        "freshness": freshness,
        "last_checked_at": _iso(last_checked_at),
        "failure_kind": row.get("last_failure_kind"),
        "last_error": row.get("last_error"),
        "retry_after_at": _iso(row.get("retry_after_at")),
        "rows_seen": int(row.get("rows_seen") or 0),
        "rows_inserted": int(row.get("rows_inserted") or 0),
    }


def _source_is_usable(snapshot: dict[str, Any]) -> bool:
    return snapshot["freshness"] == "fresh" and snapshot["status"] in {
        "ok",
        "no_new_rows",
        "running",
    }


def _baseline_missing(thesis: dict[str, Any]) -> list[str]:
    raw = _json(thesis.get("missing_evidence"), [])
    if not isinstance(raw, list):
        return []
    out: list[str] = []
    for item in raw:
        if isinstance(item, str):
            out.append(item)
        elif isinstance(item, dict):
            name = item.get("name") or item.get("source") or item.get("reason")
            if isinstance(name, str):
                out.append(name)
    return out


def _dedupe(values: list[str]) -> list[str]:
    seen: set[str] = set()
    out: list[str] = []
    for value in values:
        if value not in seen:
            seen.add(value)
            out.append(value)
    return out


def _theme_missing_evidence(thesis: dict[str, Any], metrics: dict[str, int]) -> list[str]:
    linked = int(metrics.get("linked_count") or 0)
    missing: list[str] = []
    if linked == 0:
        missing.append("linked_ticker_universe")
    elif int(metrics.get("context_symbols") or 0) == 0:
        missing.append("linked_ticker_context")
    if linked > 0 and int(metrics.get("price_symbols") or 0) == 0:
        missing.append("price_history")
    if linked > 0 and int(metrics.get("news_symbols") or 0) == 0:
        missing.append("recent_news")
    if linked > 0 and int(metrics.get("estimate_symbols") or 0) == 0:
        missing.append("analyst_estimates")
    if linked > 0 and int(metrics.get("opinion_symbols") or 0) == 0:
        missing.append("analyst_opinion")

    has_estimates = int(metrics.get("estimate_symbols") or 0) > 0
    has_opinion = int(metrics.get("opinion_symbols") or 0) > 0
    has_price = int(metrics.get("price_symbols") or 0) > 0
    has_research = int(metrics.get("research_symbols") or 0) > 0

    for item in _baseline_missing(thesis):
        low = item.lower()
        if any(part in low for part in COMMODITY_REQUIREMENT_PARTS):
            missing.append(item)
            continue
        if "estimate" in low or "revision" in low or "target" in low or "rating" in low:
            if not (has_estimates or has_opinion):
                missing.append(item)
            continue
        if "relative_strength" in low or "price" in low or "chart" in low:
            if not has_price:
                missing.append(item)
            continue
        if any(part in low for part in RESEARCH_REQUIREMENT_PARTS):
            if not has_research:
                missing.append(item)
            continue
        missing.append(item)
    return _dedupe(missing)


def _theme_direction(current: str, metrics: dict[str, int]) -> str:
    bullish = int(metrics.get("bullish_theses") or 0)
    bearish = int(metrics.get("bearish_theses") or 0)
    linked = max(1, int(metrics.get("linked_count") or 0))
    resolved = bullish + bearish
    required = max(2, linked // 2)
    if resolved >= required and bullish >= bearish + 2:
        return "bullish"
    if resolved >= required and bearish >= bullish + 2:
        return "bearish"
    if bullish or bearish:
        return "mixed"
    return current if current in {"bullish", "bearish", "mixed", "neutral"} else "mixed"


def _theme_state(current: str, missing: list[str], metrics: dict[str, int]) -> str:
    if current in {"invalidated", "archived"}:
        return current
    if not missing and int(metrics.get("linked_count") or 0) > 0:
        return "active"
    return "forming"


def _generated_evidence(existing: Any) -> list[Any]:
    decoded = _json(existing, [])
    if not isinstance(decoded, list):
        return []
    return [
        item
        for item in decoded
        if not (isinstance(item, dict) and item.get("generated_by") == "brain_maintainer")
    ]


def build_theme_update(
    thesis: dict[str, Any],
    metrics: dict[str, int],
    *,
    now: dt.datetime | None = None,
) -> dict[str, Any]:
    now = now or dt.datetime.now(dt.UTC)
    missing = _theme_missing_evidence(thesis, metrics)
    state = _theme_state(thesis.get("state") or "forming", missing, metrics)
    direction = _theme_direction(thesis.get("direction") or "mixed", metrics)
    coverage = {
        "linked": int(metrics.get("linked_count") or 0),
        "contexts": int(metrics.get("context_symbols") or 0),
        "open_theses": int(metrics.get("open_thesis_symbols") or 0),
        "price": int(metrics.get("price_symbols") or 0),
        "news": int(metrics.get("news_symbols") or 0),
        "estimates": int(metrics.get("estimate_symbols") or 0),
        "analyst_opinion": int(metrics.get("opinion_symbols") or 0),
        "research": int(metrics.get("research_symbols") or 0),
        "nominations": int(metrics.get("open_nominations") or 0),
        "bullish_ticker_theses": int(metrics.get("bullish_theses") or 0),
        "bearish_ticker_theses": int(metrics.get("bearish_theses") or 0),
    }
    fingerprint_payload = {
        "state": state,
        "direction": direction,
        "missing_evidence": missing,
        "coverage": coverage,
    }
    fingerprint = _stable_fingerprint(fingerprint_payload)
    evidence = _generated_evidence(thesis.get("evidence")) + [
        {
            "generated_by": "brain_maintainer",
            "kind": "linked_ticker_coverage",
            "as_of": now.isoformat(),
            "coverage": coverage,
        }
    ]
    source_ref = dict(_json(thesis.get("source_ref"), {}) or {})
    source_ref["maintainer"] = {
        "kind": "theme_coverage",
        "evaluated_at": now.isoformat(),
        "fingerprint": fingerprint,
        "coverage": coverage,
    }
    return {
        "state": state,
        "direction": direction,
        "missing_evidence": missing,
        "evidence": evidence,
        "source_ref": source_ref,
        "fingerprint": fingerprint,
        "diff": fingerprint_payload,
    }


def _macro_missing_evidence(
    thesis: dict[str, Any],
    sources: dict[str, dict[str, Any]],
    market_state: dict[str, Any] | None,
) -> list[str]:
    missing: list[str] = []
    fred = sources["fred"]
    cboe = sources["cboe"]
    if not _source_is_usable(fred):
        missing.append("fred_macro")
    if not market_state:
        missing.append("market_breadth")
    if not _source_is_usable(fred):
        missing.append("credit_spreads")
    if not _source_is_usable(cboe):
        missing.append("sentiment_volatility")
    for item in _baseline_missing(thesis):
        low = item.lower()
        if low == "fred_macro" and _source_is_usable(fred):
            continue
        if low == "market_breadth" and market_state:
            continue
        if low == "credit_spreads" and _source_is_usable(fred):
            continue
        missing.append(item)
    return _dedupe(missing)


def build_macro_update(
    thesis: dict[str, Any],
    source_health: dict[str, dict[str, Any]],
    market_state: dict[str, Any] | None,
    *,
    now: dt.datetime | None = None,
) -> dict[str, Any]:
    now = now or dt.datetime.now(dt.UTC)
    sources = {
        "fred": _source_snapshot(source_health, "fred", now=now),
        "cboe": _source_snapshot(source_health, "cboe", now=now),
    }
    missing = _macro_missing_evidence(thesis, sources, market_state)
    regime = (market_state or {}).get("regime")
    direction = regime if regime in {"risk_on", "risk_off", "neutral"} else "neutral"
    state = "active" if not missing and market_state else "forming"
    fingerprint_payload = {
        "state": state,
        "direction": direction,
        "missing_evidence": missing,
        "sources": {
            key: {
                "status": value["status"],
                "freshness": value["freshness"],
                "rows_seen": value["rows_seen"],
            }
            for key, value in sources.items()
        },
        "market_state_regime": regime,
    }
    fingerprint = _stable_fingerprint(fingerprint_payload)
    evidence = _generated_evidence(thesis.get("evidence")) + [
        {
            "generated_by": "brain_maintainer",
            "kind": "macro_source_freshness",
            "as_of": now.isoformat(),
            "sources": sources,
            "market_state": market_state,
        }
    ]
    source_ref = dict(_json(thesis.get("source_ref"), {}) or {})
    source_ref["maintainer"] = {
        "kind": "macro_coverage",
        "evaluated_at": now.isoformat(),
        "fingerprint": fingerprint,
        "sources": sources,
        "market_state": market_state,
    }
    return {
        "state": state,
        "direction": direction,
        "missing_evidence": missing,
        "evidence": evidence,
        "source_ref": source_ref,
        "fingerprint": fingerprint,
        "diff": fingerprint_payload,
    }


async def _load_source_health(pool: asyncpg.Pool) -> dict[str, dict[str, Any]]:
    rows = await pool.fetch(
        """SELECT source, last_status, last_started_at, last_success_at,
                  last_failure_at, last_failure_kind, last_error, retry_after_at,
                  rows_seen, rows_inserted, symbols_attempted, symbols_failed,
                  updated_at
             FROM source_health""",
    )
    return {row["source"]: dict(row) for row in rows}


async def _load_market_state(pool: asyncpg.Pool) -> dict[str, Any] | None:
    row = await pool.fetchrow(
        """SELECT as_of, regime, capitulation, indicators, subsector_rs
             FROM market_state
         ORDER BY as_of DESC
            LIMIT 1""",
    )
    if row is None:
        return None
    return {
        "as_of": _iso(row["as_of"]),
        "regime": row["regime"],
        "capitulation": bool(row["capitulation"]),
        "indicators": _json(row["indicators"], {}),
        "subsector_rs": _json(row["subsector_rs"], {}),
    }


async def _load_thesis_metrics(pool: asyncpg.Pool, brain_thesis_id: Any) -> dict[str, int]:
    row = await pool.fetchrow(
        """WITH linked AS (
               SELECT symbol
                 FROM brain_thesis_ticker
                WHERE brain_thesis_id = $1
           ), latest_thesis AS (
               SELECT DISTINCT ON (th.symbol)
                      th.symbol,
                      th.forecast->>'direction' AS direction
                 FROM thesis th
                 JOIN linked l ON l.symbol = th.symbol
                WHERE th.state NOT IN ('closed', 'disqualified')
             ORDER BY th.symbol, th.updated_at DESC, th.created_at DESC
           )
           SELECT
              (SELECT count(*) FROM linked) AS linked_count,
              (SELECT count(DISTINCT symbol) FROM ticker_context
                WHERE symbol IN (SELECT symbol FROM linked)) AS context_symbols,
              (SELECT count(*) FROM latest_thesis) AS open_thesis_symbols,
              (SELECT count(DISTINCT symbol) FROM price_bar
                WHERE symbol IN (SELECT symbol FROM linked)) AS price_symbols,
              (SELECT count(DISTINCT symbol) FROM news_article
                WHERE symbol IN (SELECT symbol FROM linked)
                  AND published_at > now() - interval '30 days') AS news_symbols,
              (SELECT count(DISTINCT symbol) FROM estimate_snapshot
                WHERE symbol IN (SELECT symbol FROM linked)) AS estimate_symbols,
              (SELECT count(DISTINCT symbol) FROM analyst_price_target_snapshot
                WHERE symbol IN (SELECT symbol FROM linked)) AS opinion_symbols,
              (SELECT count(DISTINCT symbol) FROM research_evidence
                WHERE symbol IN (SELECT symbol FROM linked)
                  AND retrieved_at > now() - interval '30 days') AS research_symbols,
              (SELECT count(DISTINCT symbol) FROM discovery_candidate
                WHERE symbol IN (SELECT symbol FROM linked)
                  AND status = 'proposed') AS open_nominations,
              (SELECT count(*) FROM latest_thesis
                WHERE direction IN ('up', 'bullish', 'risk_on')) AS bullish_theses,
              (SELECT count(*) FROM latest_thesis
                WHERE direction IN ('down', 'bearish', 'risk_off')) AS bearish_theses""",
        brain_thesis_id,
    )
    if row is None:
        return {}
    return {key: int(row[key] or 0) for key in row.keys()}


async def ensure_brain_ticker_mappings(pool: asyncpg.Pool) -> int:
    """Ensure beneficiary/loser symbols from parent theses are tracked.

    Migrations seeded mappings only for tickers that already existed. The brain
    loop owns its expression universe, so parent thesis beneficiaries should
    become active ticker rows and mappings for downstream data/cognition loops.
    """
    rows = await pool.fetch(
        """SELECT id, name, beneficiaries, losers
             FROM brain_thesis
            WHERE active = true
              AND scope <> 'macro'""",
    )
    inserted = 0
    async with pool.acquire() as conn:
        async with conn.transaction():
            for row in rows:
                groups = [
                    ("beneficiary", symbols_from_json(row["beneficiaries"])),
                    ("hedge", symbols_from_json(row["losers"])),
                ]
                for role, symbols in groups:
                    for symbol in symbols:
                        await conn.execute(
                            """INSERT INTO ticker(symbol, tier, status)
                               VALUES ($1, 3, 'active')
                               ON CONFLICT (symbol) DO NOTHING""",
                            symbol,
                        )
                        result = await conn.execute(
                            """INSERT INTO brain_thesis_ticker
                                 (brain_thesis_id, symbol, role, rationale, conviction)
                               VALUES ($1, $2, $3, $4, 45)
                               ON CONFLICT (brain_thesis_id, symbol) DO NOTHING""",
                            row["id"],
                            symbol,
                            role,
                            f"Derived from {row['name']} parent thesis expressions.",
                        )
                        if result.endswith("1"):
                            inserted += 1
    return inserted


async def _persist_update(
    pool: asyncpg.Pool,
    thesis: dict[str, Any],
    update: dict[str, Any],
) -> bool:
    brain_thesis_id = thesis["id"]
    async with pool.acquire() as conn:
        async with conn.transaction():
            locked = await conn.fetchrow(
                """SELECT version, source_ref
                     FROM brain_thesis
                    WHERE id = $1
                    FOR UPDATE""",
                brain_thesis_id,
            )
            if locked is None:
                return False
            prior_source_ref = _json(locked["source_ref"], {}) or {}
            prior_fingerprint = (
                prior_source_ref.get("maintainer", {}).get("fingerprint")
                if isinstance(prior_source_ref, dict)
                else None
            )
            material = prior_fingerprint != update["fingerprint"]
            next_version = int(locked["version"] or 1) + 1 if material else locked["version"]
            await conn.execute(
                """UPDATE brain_thesis
                      SET state = $2,
                          direction = $3,
                          evidence = $4::jsonb,
                          missing_evidence = $5::jsonb,
                          source_ref = $6::jsonb,
                          last_evaluated_at = now(),
                          version = $7,
                          updated_at = CASE WHEN $8 THEN now() ELSE updated_at END
                    WHERE id = $1""",
                brain_thesis_id,
                update["state"],
                update["direction"],
                json.dumps(update["evidence"], default=str),
                json.dumps(update["missing_evidence"], default=str),
                json.dumps(update["source_ref"], default=str),
                next_version,
                material,
            )
            if material:
                await conn.execute(
                    """INSERT INTO brain_thesis_version_history
                         (brain_thesis_id, version, diff, rationale)
                       VALUES ($1, $2, $3::jsonb, $4)""",
                    brain_thesis_id,
                    next_version,
                    json.dumps(
                        {
                            "event": "brain_thesis_evaluation",
                            "previous_fingerprint": prior_fingerprint,
                            **update["diff"],
                        },
                        default=str,
                    ),
                    "Parent thesis evaluated from source freshness and linked ticker coverage.",
                )
            return material


async def refresh(pool: asyncpg.Pool, *, limit: int = 50) -> int:
    inserted_mappings = await ensure_brain_ticker_mappings(pool)
    if inserted_mappings:
        log.info("brain maintainer linked %d parent expression ticker(s)", inserted_mappings)

    rows = await pool.fetch(
        """SELECT id, scope, key, name, state, direction, evidence,
                  missing_evidence, source_ref, beneficiaries, losers
             FROM brain_thesis
            WHERE active = true
         ORDER BY last_evaluated_at ASC NULLS FIRST, updated_at ASC
            LIMIT $1""",
        limit,
    )
    if not rows:
        return 0

    source_health = await _load_source_health(pool)
    market_state = await _load_market_state(pool)
    updated = 0
    for row in rows:
        thesis = dict(row)
        if thesis["scope"] == "macro":
            update = build_macro_update(thesis, source_health, market_state)
        else:
            metrics = await _load_thesis_metrics(pool, thesis["id"])
            update = build_theme_update(thesis, metrics)
        material = await _persist_update(pool, thesis, update)
        updated += 1
        log.info(
            "brain thesis evaluated key=%s material=%s state=%s direction=%s missing=%d",
            thesis["key"],
            material,
            update["state"],
            update["direction"],
            len(update["missing_evidence"]),
        )
    return updated


async def _run_cli(limit: int) -> None:
    cfg = config.load()
    pool = await asyncpg.create_pool(cfg.database_url, min_size=1, max_size=2)
    assert pool is not None
    try:
        count = await refresh(pool, limit=limit)
        print(f"brain_thesis evaluated: {count}")
    finally:
        await pool.close()


def _cli() -> None:
    parser = argparse.ArgumentParser(prog="brain_maintainer")
    parser.add_argument("--limit", type=int, default=50)
    args = parser.parse_args()
    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s %(name)s %(levelname)s %(message)s",
    )
    asyncio.run(_run_cli(args.limit))


if __name__ == "__main__":
    _cli()


__all__ = [
    "build_macro_update",
    "build_theme_update",
    "ensure_brain_ticker_mappings",
    "normalize_symbol",
    "refresh",
    "symbols_from_json",
]
