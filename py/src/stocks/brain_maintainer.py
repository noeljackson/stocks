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
import os
import re
from typing import Any

import asyncpg

from . import config
from .context_maintainer import _llm_cfg, _provider_name, _repo_root  # noqa: PLC2701
from .llm import new_provider
from .prompts import AsyncpgRecorder, invoke, load

log = logging.getLogger("brain_maintainer")

SYMBOL_RE = re.compile(r"^(?=.{1,14}$)[A-Z0-9]+(?:[.\-][A-Z0-9]+)*$")
SOURCE_FRESHNESS_MINUTES = 90
COMMODITY_PRICE_REQUIREMENT_PARTS = (
    "commodity_price",
    "commodity price",
    "crop_price",
    "crop price",
    "futures_price",
    "futures price",
)
COMMODITY_FUNDAMENTAL_REQUIREMENT_PARTS = (
    "inventory",
    "inventories",
    "usda",
    "weather",
    "china_demand",
    "china demand",
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
ALLOWED_STATES = {"forming", "active", "weakening", "invalidated", "archived"}
ALLOWED_DIRECTIONS = {"risk_on", "risk_off", "neutral", "bullish", "bearish", "mixed"}
THEME_PROXY_SYMBOLS = {
    "copper_industrial_metals": {"CPER", "XME"},
    "wheat_agriculture_food": {"WEAT"},
}


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


def _default_expression_role(theme_key: Any, symbol: str, fallback: str) -> str:
    """Classify direct tradable proxies separately from operating companies."""
    key = str(theme_key or "")
    if normalize_symbol(symbol) in THEME_PROXY_SYMBOLS.get(key, set()):
        return "proxy"
    return fallback


def _env_int(name: str, default: int) -> int:
    raw = os.getenv(name)
    if raw is None:
        return default
    try:
        return int(raw)
    except ValueError:
        log.warning("invalid integer env %s=%r; using %d", name, raw, default)
        return default


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
    has_proxy = int(metrics.get("proxy_count") or 0) > 0
    has_proxy_price = int(metrics.get("proxy_price_symbols") or 0) > 0
    has_research = int(metrics.get("research_symbols") or 0) > 0

    for item in _baseline_missing(thesis):
        low = item.lower()
        if any(part in low for part in COMMODITY_PRICE_REQUIREMENT_PARTS):
            if not (has_proxy_price if has_proxy else has_price):
                missing.append(item)
            continue
        if any(part in low for part in COMMODITY_FUNDAMENTAL_REQUIREMENT_PARTS):
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
        if not (
            isinstance(item, dict)
            and item.get("generated_by") in {"brain_maintainer", "brain_llm"}
        )
    ]


def _coerce_text(value: Any, fallback: str | None = None, *, max_len: int = 1200) -> str | None:
    if not isinstance(value, str):
        return fallback
    stripped = value.strip()
    if not stripped:
        return fallback
    return stripped[:max_len]


def _coerce_string_list(value: Any, fallback: list[str], *, max_items: int = 12) -> list[str]:
    decoded = _json(value, value)
    if not isinstance(decoded, list):
        return fallback
    out: list[str] = []
    for item in decoded:
        if isinstance(item, str) and item.strip():
            out.append(item.strip()[:240])
        elif isinstance(item, dict):
            text = (
                item.get("name")
                or item.get("claim")
                or item.get("question")
                or item.get("reason")
            )
            if isinstance(text, str) and text.strip():
                out.append(text.strip()[:240])
        if len(out) >= max_items:
            break
    return _dedupe(out) or fallback


def _merge_missing_evidence(
    deterministic_missing: list[str],
    llm_missing: Any,
) -> list[str]:
    """Preserve deterministic gaps while accepting extra LLM-requested gaps."""
    base = list(deterministic_missing)
    llm_items = _coerce_string_list(llm_missing, [], max_items=12)
    deterministic_has_commodity_price = any(
        any(part in item.lower() for part in COMMODITY_PRICE_REQUIREMENT_PARTS)
        for item in base
    )
    for item in llm_items:
        low = item.lower()
        if (
            any(part in low for part in COMMODITY_PRICE_REQUIREMENT_PARTS)
            and not deterministic_has_commodity_price
        ):
            continue
        base.append(item)
    return _dedupe(base)


def _coerce_json_list(value: Any, fallback: list[Any], *, max_items: int = 12) -> list[Any]:
    decoded = _json(value, value)
    if not isinstance(decoded, list):
        return fallback
    out: list[Any] = []
    for item in decoded[:max_items]:
        if isinstance(item, dict):
            out.append(item)
        elif isinstance(item, str) and item.strip():
            out.append({"claim": item.strip()[:500]})
    return out or fallback


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
        "commodity_proxies": int(metrics.get("proxy_count") or 0),
        "commodity_proxy_price": int(metrics.get("proxy_price_symbols") or 0),
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
        "deterministic_fingerprint": fingerprint,
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
        "deterministic_fingerprint": fingerprint,
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
               SELECT symbol, role
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
              (SELECT count(*) FROM linked WHERE role = 'proxy') AS proxy_count,
              (SELECT count(DISTINCT symbol) FROM ticker_context
                WHERE symbol IN (SELECT symbol FROM linked)) AS context_symbols,
              (SELECT count(*) FROM latest_thesis) AS open_thesis_symbols,
              (SELECT count(DISTINCT symbol) FROM price_bar
                WHERE symbol IN (SELECT symbol FROM linked)) AS price_symbols,
              (SELECT count(*) FROM linked l
                WHERE l.role = 'proxy'
                  AND EXISTS (
                      SELECT 1 FROM price_bar pb WHERE pb.symbol = l.symbol
                  )) AS proxy_price_symbols,
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


async def _load_parent_context(pool: asyncpg.Pool, thesis: dict[str, Any]) -> dict[str, Any]:
    ticker_rows = await pool.fetch(
        """SELECT btt.symbol, btt.role, btt.rationale, btt.conviction,
                  latest.state AS thesis_state,
                  latest.forecast AS thesis_forecast,
                  latest.edge_rationale AS thesis_edge,
                  latest.updated_at AS thesis_updated_at
             FROM brain_thesis_ticker btt
        LEFT JOIN LATERAL (
                  SELECT th.state, th.forecast, th.edge_rationale, th.updated_at
                    FROM thesis th
                   WHERE th.symbol = btt.symbol
                     AND th.state NOT IN ('closed', 'disqualified')
                ORDER BY th.updated_at DESC, th.created_at DESC
                   LIMIT 1
             ) latest ON TRUE
            WHERE btt.brain_thesis_id = $1
         ORDER BY COALESCE(btt.conviction, 0) DESC, btt.symbol
            LIMIT 40""",
        thesis["id"],
    )
    linked_tickers: list[dict[str, Any]] = []
    symbols: list[str] = []
    for row in ticker_rows:
        symbol = row["symbol"]
        symbols.append(symbol)
        forecast = _json(row["thesis_forecast"], {})
        linked_tickers.append({
            "symbol": symbol,
            "role": row["role"],
            "rationale": row["rationale"],
            "conviction": row["conviction"],
            "thesis_state": row["thesis_state"],
            "thesis_direction": forecast.get("direction") if isinstance(forecast, dict) else None,
            "thesis_edge": row["thesis_edge"],
            "thesis_updated_at": _iso(row["thesis_updated_at"]),
        })

    evidence_items: list[dict[str, Any]] = []
    if symbols:
        evidence_rows = await pool.fetch(
            """SELECT id, symbol, kind, observed_at, source, source_id,
                      summary, strength, polarity, url
                 FROM evidence_item
                WHERE symbol = ANY($1::text[])
                  AND NOT (
                      kind = 'product_research'
                      AND source = 'web_research'
                      AND COALESCE((source_ref->'relevance'->>'accepted')::boolean, false) = false
                  )
             ORDER BY observed_at DESC, id DESC
                LIMIT 80""",
            symbols,
        )
        for row in evidence_rows:
            evidence_items.append({
                "id": row["id"],
                "symbol": row["symbol"],
                "kind": row["kind"],
                "observed_at": _iso(row["observed_at"]),
                "source": row["source"],
                "source_id": row["source_id"],
                "summary": row["summary"],
                "strength": None if row["strength"] is None else float(row["strength"]),
                "polarity": None if row["polarity"] is None else float(row["polarity"]),
                "url": row["url"],
            })

    return {
        "linked_tickers": linked_tickers,
        "evidence_items": evidence_items,
    }


def _brain_llm_enabled(cfg: config.Config) -> bool:
    raw = os.getenv("BRAIN_THESIS_LLM_ENABLED")
    if raw is not None:
        return raw.strip().lower() in {"1", "true", "yes", "on"}
    if cfg.llm_provider == "mock":
        return False
    return bool(cfg.anthropic_api_key or cfg.openai_api_key)


def _brain_llm_due(
    thesis: dict[str, Any],
    update: dict[str, Any],
    *,
    now: dt.datetime,
    max_age_minutes: int,
) -> bool:
    source_ref = _json(thesis.get("source_ref"), {}) or {}
    maintainer = source_ref.get("maintainer", {}) if isinstance(source_ref, dict) else {}
    prior_deterministic = (
        maintainer.get("deterministic_fingerprint")
        or maintainer.get("fingerprint")
    )
    if prior_deterministic != update["fingerprint"]:
        return True
    llm_ref = source_ref.get("llm", {}) if isinstance(source_ref, dict) else {}
    evaluated_at = _parse_dt(llm_ref.get("evaluated_at")) if isinstance(llm_ref, dict) else None
    return evaluated_at is None or evaluated_at < now - dt.timedelta(minutes=max_age_minutes)


def merge_llm_update(
    thesis: dict[str, Any],
    update: dict[str, Any],
    parsed: dict[str, Any],
    *,
    prompt_hash: str,
    now: dt.datetime | None = None,
) -> dict[str, Any]:
    now = now or dt.datetime.now(dt.UTC)
    merged = dict(update)
    source_ref = dict(_json(update.get("source_ref"), {}) or {})
    prior_source_ref = _json(thesis.get("source_ref"), {}) or {}
    prior_llm = prior_source_ref.get("llm", {}) if isinstance(prior_source_ref, dict) else {}

    state = _coerce_text(parsed.get("state"), update["state"], max_len=40)
    direction = _coerce_text(parsed.get("direction"), update["direction"], max_len=40)
    merged["state"] = state if state in ALLOWED_STATES else update["state"]
    merged["direction"] = direction if direction in ALLOWED_DIRECTIONS else update["direction"]
    merged["summary"] = _coerce_text(parsed.get("summary"), thesis.get("summary"), max_len=900)
    merged["core_claim"] = _coerce_text(
        parsed.get("core_claim"),
        thesis.get("core_claim"),
        max_len=1200,
    )
    merged["why_now"] = _coerce_text(parsed.get("why_now"), thesis.get("why_now"), max_len=900)
    merged["missing_evidence"] = _merge_missing_evidence(
        update.get("missing_evidence", []),
        parsed.get("missing_evidence"),
    )
    merged["open_questions"] = _coerce_string_list(
        parsed.get("open_questions"),
        _json(thesis.get("open_questions"), []),
    )
    merged["beneficiaries"] = _coerce_string_list(
        parsed.get("beneficiaries"),
        symbols_from_json(thesis.get("beneficiaries")),
    )
    merged["losers"] = _coerce_string_list(
        parsed.get("losers"),
        symbols_from_json(thesis.get("losers")),
    )
    merged["invalidation_conditions"] = _coerce_json_list(
        parsed.get("invalidation_conditions"),
        _json(thesis.get("invalidation_conditions"), []),
    )

    llm_evidence = _coerce_json_list(parsed.get("evidence"), [], max_items=12)
    llm_evidence = [
        {
            "generated_by": "brain_llm",
            "as_of": now.isoformat(),
            **item,
        }
        for item in llm_evidence
    ]
    maintainer_evidence = [
        item
        for item in update.get("evidence", [])
        if isinstance(item, dict) and item.get("generated_by") == "brain_maintainer"
    ]
    merged["evidence"] = (
        _generated_evidence(thesis.get("evidence"))
        + llm_evidence
        + maintainer_evidence
    )

    llm_fingerprint_payload = {
        key: merged.get(key)
        for key in (
            "state",
            "direction",
            "summary",
            "core_claim",
            "why_now",
            "missing_evidence",
            "open_questions",
            "beneficiaries",
            "losers",
            "invalidation_conditions",
            "evidence",
        )
    }
    llm_fingerprint = _stable_fingerprint(llm_fingerprint_payload)
    source_ref["llm"] = {
        "kind": "parent_thesis_update",
        "evaluated_at": now.isoformat(),
        "prompt_name": "update-brain-thesis",
        "prompt_hash": prompt_hash,
        "fingerprint": llm_fingerprint,
        "material_change_reason": _coerce_text(
            parsed.get("material_change_reason"),
            None,
            max_len=500,
        ),
    }
    merged["source_ref"] = source_ref
    merged["llm_material"] = prior_llm.get("fingerprint") != llm_fingerprint
    merged["diff"] = {
        **update.get("diff", {}),
        "llm": {
            "fingerprint": llm_fingerprint,
            "material": merged["llm_material"],
            "material_change_reason": source_ref["llm"]["material_change_reason"],
        },
    }
    return merged


def _extract_json_dict(content: str) -> dict[str, Any]:
    from .prompts import _extract_json as extract_json_text  # noqa: PLC0415

    raw = extract_json_text(content)
    try:
        parsed = json.loads(raw)
    except json.JSONDecodeError as e:
        raise ValueError(
            f"could not parse parent brain JSON: {e.msg} at line {e.lineno} column {e.colno}"
        ) from e
    if not isinstance(parsed, dict):
        raise ValueError("parent brain response must be a JSON object")
    return parsed


async def _invoke_parent_update(
    *,
    pool: asyncpg.Pool,
    provider,
    prompt,
    provider_name: str,
    model: str,
    thesis: dict[str, Any],
    deterministic_update: dict[str, Any],
    parent_context: dict[str, Any],
    max_retries: int = 2,
) -> dict[str, Any]:
    today = dt.date.today().isoformat()
    payload = {
        "today": today,
        "brain_thesis": {
            "scope": thesis.get("scope"),
            "key": thesis.get("key"),
            "name": thesis.get("name"),
            "state": thesis.get("state"),
            "direction": thesis.get("direction"),
            "summary": thesis.get("summary"),
            "core_claim": thesis.get("core_claim"),
            "why_now": thesis.get("why_now"),
            "evidence": _json(thesis.get("evidence"), []),
            "invalidation_conditions": _json(thesis.get("invalidation_conditions"), []),
            "beneficiaries": _json(thesis.get("beneficiaries"), []),
            "losers": _json(thesis.get("losers"), []),
            "open_questions": _json(thesis.get("open_questions"), []),
            "missing_evidence": _json(thesis.get("missing_evidence"), []),
        },
        "deterministic_update": deterministic_update.get("diff", {}),
        "source_ref": deterministic_update.get("source_ref", {}),
        "parent_context": parent_context,
    }
    user_msg = json.dumps(payload, default=str, indent=2)
    current_user = user_msg
    last_error: Exception | None = None
    for attempt in range(max_retries + 1):
        resp = await invoke(
            provider=provider,
            recorder=AsyncpgRecorder(pool),
            prompt=prompt,
            vars={
                "today": today,
                "name": str(thesis.get("name") or ""),
                "scope": str(thesis.get("scope") or ""),
            },
            user_message=current_user,
            provider_name=provider_name,
            model=model,
            max_tokens=3072,
        )
        try:
            return _extract_json_dict(resp.content)
        except ValueError as e:
            last_error = e
            if attempt >= max_retries:
                raise RuntimeError(
                    "update-brain-thesis returned invalid JSON after "
                    f"{max_retries + 1} attempts: {e}"
                ) from e
            log.warning(
                "parent brain JSON parse failed key=%s attempt=%d/%d; retrying: %s",
                thesis.get("key"),
                attempt + 1,
                max_retries + 1,
                e,
            )
            current_user = (
                f"{user_msg}\n\n"
                "[Previous update-brain-thesis response was invalid JSON. "
                "Reply ONLY with one complete valid JSON object matching the prompt contract. "
                f"JSON parse error: {e}.]\n\n"
                "[Invalid response excerpt]\n"
                f"{resp.content[:2500]}"
            )
    raise RuntimeError(f"parent brain retry loop exited unexpectedly: {last_error}")


async def ensure_brain_ticker_mappings(pool: asyncpg.Pool) -> int:
    """Ensure beneficiary/loser symbols from parent theses are tracked.

    Migrations seeded mappings only for tickers that already existed. The brain
    loop owns its expression universe, so parent thesis beneficiaries should
    become active ticker rows and mappings for downstream data/cognition loops.
    """
    rows = await pool.fetch(
        """SELECT id, key, name, beneficiaries, losers
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
                for default_role, symbols in groups:
                    for symbol in symbols:
                        role = _default_expression_role(row["key"], symbol, default_role)
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
            material = (
                prior_fingerprint != update["fingerprint"]
                or bool(update.get("llm_material"))
            )
            next_version = int(locked["version"] or 1) + 1 if material else locked["version"]
            await conn.execute(
                """UPDATE brain_thesis
                      SET state = $2,
                          direction = $3,
                          summary = COALESCE($9, summary),
                          core_claim = COALESCE($10, core_claim),
                          why_now = COALESCE($11, why_now),
                          evidence = $4::jsonb,
                          missing_evidence = $5::jsonb,
                          source_ref = $6::jsonb,
                          open_questions = COALESCE($12::jsonb, open_questions),
                          invalidation_conditions = COALESCE($13::jsonb, invalidation_conditions),
                          beneficiaries = COALESCE($14::jsonb, beneficiaries),
                          losers = COALESCE($15::jsonb, losers),
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
                update.get("summary"),
                update.get("core_claim"),
                update.get("why_now"),
                json.dumps(update["open_questions"], default=str)
                if "open_questions" in update
                else None,
                json.dumps(update["invalidation_conditions"], default=str)
                if "invalidation_conditions" in update
                else None,
                json.dumps(update["beneficiaries"], default=str)
                if "beneficiaries" in update
                else None,
                json.dumps(update["losers"], default=str) if "losers" in update else None,
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
                    "Parent thesis evaluated from source freshness, linked ticker "
                    "coverage, and parent cognition.",
                )
            return material


async def refresh(pool: asyncpg.Pool, *, limit: int = 50) -> int:
    inserted_mappings = await ensure_brain_ticker_mappings(pool)
    if inserted_mappings:
        log.info("brain maintainer linked %d parent expression ticker(s)", inserted_mappings)

    cfg = config.load()
    now = dt.datetime.now(dt.UTC)
    llm_enabled = _brain_llm_enabled(cfg)
    llm_budget = max(0, _env_int("BRAIN_THESIS_LLM_MAX_PER_SWEEP", 2))
    llm_max_age_minutes = max(30, _env_int("BRAIN_THESIS_LLM_MAX_AGE_MINUTES", 720))
    llm_provider = None
    llm_prompt = None
    llm_provider_name = ""
    if llm_enabled and llm_budget > 0:
        registry = load(_repo_root() / "prompts")
        llm_prompt = registry.get("update-brain-thesis")
        if llm_prompt is None:
            log.warning("prompts/update-brain-thesis.md missing; parent LLM pass disabled")
        else:
            llm_provider = new_provider(_llm_cfg(cfg))
            llm_provider_name = _provider_name(cfg)

    rows = await pool.fetch(
        """SELECT id, scope, key, name, state, direction, evidence,
                  summary, core_claim, why_now,
                  invalidation_conditions, beneficiaries, losers,
                  open_questions, missing_evidence, source_ref
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
            update = build_macro_update(thesis, source_health, market_state, now=now)
        else:
            metrics = await _load_thesis_metrics(pool, thesis["id"])
            update = build_theme_update(thesis, metrics, now=now)
        if (
            llm_provider is not None
            and llm_prompt is not None
            and llm_budget > 0
            and _brain_llm_due(
                thesis,
                update,
                now=now,
                max_age_minutes=llm_max_age_minutes,
            )
        ):
            try:
                parent_context = await _load_parent_context(pool, thesis)
                parsed = await _invoke_parent_update(
                    pool=pool,
                    provider=llm_provider,
                    prompt=llm_prompt,
                    provider_name=llm_provider_name,
                    model=cfg.model_routine,
                    thesis=thesis,
                    deterministic_update=update,
                    parent_context=parent_context,
                )
                update = merge_llm_update(
                    thesis,
                    update,
                    parsed,
                    prompt_hash=llm_prompt.hash,
                    now=now,
                )
                llm_budget -= 1
            except Exception:  # noqa: BLE001
                log.exception("parent brain LLM update failed key=%s", thesis["key"])
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
    "merge_llm_update",
    "normalize_symbol",
    "refresh",
    "symbols_from_json",
]
