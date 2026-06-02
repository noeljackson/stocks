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
DISLOCATION_BUCKETS = ("loved_mania", "ignored_indifference", "hated_avoided")


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


def _round_float(value: Any, digits: int = 4) -> float | None:
    if value is None:
        return None
    try:
        return round(float(value), digits)
    except (TypeError, ValueError):
        return None


def _as_float(value: Any, default: float = 0.0) -> float:
    try:
        if value is None:
            return default
        return float(value)
    except (TypeError, ValueError):
        return default


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
    indicators = (market_state or {}).get("indicators") or {}
    if not isinstance(indicators, dict):
        indicators = {}
    subsector_rs = (market_state or {}).get("subsector_rs") or {}
    if not isinstance(subsector_rs, dict):
        subsector_rs = {}

    def has_market_breadth() -> bool:
        breadth = indicators.get("market_breadth_internals")
        if isinstance(breadth, dict) and breadth.get("symbol_count", 0):
            return True
        return "breadth_pct_above_200d" in indicators

    def has_sector_rs() -> bool:
        rs = indicators.get("sector_relative_strength")
        return bool(subsector_rs) or (isinstance(rs, dict) and bool(rs.get("sectors")))

    def has_earnings_breadth() -> bool:
        breadth = indicators.get("earnings_breadth")
        return isinstance(breadth, dict) and breadth.get("signals", 0) > 0

    def has_credit_internals() -> bool:
        credit = indicators.get("credit_internals_trend")
        return isinstance(credit, dict) and credit.get("latest_hy_oas_pct") is not None

    def requirement_satisfied(item: str) -> bool:
        low = item.lower()
        if low == "fred_macro":
            return _source_is_usable(sources["fred"])
        if low in {"market_breadth", "market_breadth_internals"}:
            return has_market_breadth()
        if low == "credit_spreads":
            return _source_is_usable(sources["fred"])
        if low == "credit_internals_trend":
            return _source_is_usable(sources["fred"]) and has_credit_internals()
        if low in {"sector_relative_strength", "subsector_rs"}:
            return has_sector_rs()
        if low == "earnings_breadth":
            return has_earnings_breadth()
        if low == "sentiment_volatility":
            return _source_is_usable(sources["cboe"])
        return False

    missing: list[str] = []
    fred = sources["fred"]
    cboe = sources["cboe"]
    if not _source_is_usable(fred):
        missing.append("fred_macro")
    if not has_market_breadth():
        missing.append("market_breadth_internals")
    if not _source_is_usable(fred):
        missing.append("credit_spreads")
    if not _source_is_usable(cboe):
        missing.append("sentiment_volatility")
    for item in _baseline_missing(thesis):
        if requirement_satisfied(item):
            continue
        missing.append(item)
    return _dedupe(missing)


def _sector_revision_stats(indicators: dict[str, Any], sector: str) -> dict[str, Any]:
    earnings = indicators.get("earnings_breadth")
    sectors = earnings.get("sectors") if isinstance(earnings, dict) else None
    stats = sectors.get(sector) if isinstance(sectors, dict) else None
    return stats if isinstance(stats, dict) else {}


def _sector_news_stats(indicators: dict[str, Any], sector: str) -> dict[str, Any]:
    news = indicators.get("sector_news_sentiment")
    sectors = news.get("sectors") if isinstance(news, dict) else None
    stats = sectors.get(sector) if isinstance(sectors, dict) else None
    return stats if isinstance(stats, dict) else {}


def _revision_net(stats: dict[str, Any]) -> float:
    signals = _as_float(stats.get("signals"))
    if signals <= 0:
        return 0.0
    return (_as_float(stats.get("up")) - _as_float(stats.get("down"))) / signals


def _sector_dislocation_item(
    sector_row: dict[str, Any],
    indicators: dict[str, Any],
    *,
    rank_20d: int,
    sector_count: int,
) -> dict[str, Any]:
    sector = str(sector_row.get("sector") or "Unknown")
    return_20d = _as_float(sector_row.get("return_20d"))
    return_60d = _as_float(sector_row.get("return_60d"))
    return_120d = _as_float(sector_row.get("return_120d"))
    revision_stats = _sector_revision_stats(indicators, sector)
    news_stats = _sector_news_stats(indicators, sector)
    revision_net = _revision_net(revision_stats)
    news_polarity = _as_float(news_stats.get("avg_polarity"))
    articles_14d = int(_as_float(news_stats.get("articles_14d")))
    attention_ratio = _as_float(news_stats.get("attention_ratio"), 1.0)

    loved_reasons: list[str] = []
    ignored_reasons: list[str] = []
    hated_reasons: list[str] = []
    loved = 0.0
    ignored = 0.0
    hated = 0.0

    if rank_20d <= max(1, sector_count // 4):
        loved += 18.0
        loved_reasons.append("top-quartile 20d sector relative strength")
    if return_20d >= 0.08:
        loved += 18.0
        loved_reasons.append(f"20d sector return {_round_float(return_20d * 100, 1)}%")
    if return_60d >= 0.16:
        loved += 14.0
        loved_reasons.append(f"60d sector return {_round_float(return_60d * 100, 1)}%")
    if attention_ratio >= 1.5 or articles_14d >= 25:
        loved += 14.0
        loved_reasons.append("news attention is elevated")
    if news_polarity >= 0.2:
        loved += 10.0
        loved_reasons.append("news tone is positive")

    if -0.03 <= return_20d <= 0.06 and return_60d >= -0.03:
        ignored += 16.0
        ignored_reasons.append("price action is not chased")
    if revision_net >= 0.12:
        ignored += 20.0
        ignored_reasons.append("estimate revision breadth is improving")
    if attention_ratio <= 0.8 and articles_14d <= 12:
        ignored += 18.0
        ignored_reasons.append("news attention is low")
    if return_120d >= -0.02:
        ignored += 8.0
        ignored_reasons.append("longer-window trend is not broken")

    if return_60d <= -0.10:
        hated += 16.0
        hated_reasons.append(f"60d sector return {_round_float(return_60d * 100, 1)}%")
    if return_20d <= -0.05:
        hated += 12.0
        hated_reasons.append(f"20d sector return {_round_float(return_20d * 100, 1)}%")
    if news_polarity <= -0.2:
        hated += 14.0
        hated_reasons.append("news tone is negative")
    if revision_net >= 0.08 or return_20d > return_60d:
        hated += 14.0
        hated_reasons.append("evidence is less bad than price/sentiment")

    scores = {
        "loved_mania": loved,
        "ignored_indifference": ignored,
        "hated_avoided": hated,
    }
    classification = max(scores, key=scores.get)
    score = scores[classification]
    if score < 18.0:
        classification = "neutral"
    reasons_by_bucket = {
        "loved_mania": loved_reasons,
        "ignored_indifference": ignored_reasons,
        "hated_avoided": hated_reasons,
        "neutral": [],
    }
    bucket_reason = {
        "loved_mania": (
            "Loved/mania: strong attention or momentum can make true stories poor entries."
        ),
        "ignored_indifference": (
            "Ignored/indifference: improving evidence is not yet receiving much attention."
        ),
        "hated_avoided": (
            "Hated/avoided: weak sentiment or price action may be masking an improving setup."
        ),
        "neutral": "No clear love/hate/indifference dislocation from current internals.",
    }[classification]
    return {
        "scope": "sector",
        "name": sector,
        "classification": classification,
        "score": _round_float(score, 1),
        "metrics": {
            "rank_20d": rank_20d,
            "sector_count": sector_count,
            "return_20d": _round_float(return_20d),
            "return_60d": _round_float(return_60d),
            "return_120d": _round_float(return_120d),
            "revision_net": _round_float(revision_net),
            "articles_14d": articles_14d,
            "news_attention_ratio": _round_float(attention_ratio),
            "news_polarity": _round_float(news_polarity),
        },
        "reasons": _dedupe(reasons_by_bucket[classification])[:6],
        "interpretation": bucket_reason,
    }


def build_dislocation_map(market_state: dict[str, Any] | None) -> dict[str, Any] | None:
    indicators = (market_state or {}).get("indicators") or {}
    if not isinstance(indicators, dict):
        return None
    sector_rs = indicators.get("sector_relative_strength")
    sector_rows = sector_rs.get("sectors") if isinstance(sector_rs, dict) else None
    if not isinstance(sector_rows, list) or not sector_rows:
        return None
    sectors = [
        item
        for item in sector_rows
        if isinstance(item, dict) and item.get("sector")
    ]
    sectors.sort(key=lambda item: _as_float(item.get("return_20d")), reverse=True)
    if not sectors:
        return None
    classified = [
        _sector_dislocation_item(
            item,
            indicators,
            rank_20d=index + 1,
            sector_count=len(sectors),
        )
        for index, item in enumerate(sectors)
    ]
    buckets: dict[str, list[dict[str, Any]]] = {bucket: [] for bucket in DISLOCATION_BUCKETS}
    sector_classifications: dict[str, dict[str, Any]] = {}
    for item in classified:
        name = str(item["name"])
        sector_classifications[name] = item
        classification = item["classification"]
        if classification in buckets:
            buckets[classification].append(item)
    for bucket in buckets:
        buckets[bucket] = sorted(
            buckets[bucket],
            key=lambda item: _as_float(item.get("score")),
            reverse=True,
        )[:5]
    return {
        "as_of": (market_state or {}).get("as_of"),
        "source": "price_bar+estimate_revision+news_article",
        "buckets": buckets,
        "sector_classifications": sector_classifications,
        "watch": [
            "Bullish thesis does not mean good entry when the sector is loved/extended.",
            (
                "Prioritize ignored or hated groups when revisions or relative strength "
                "begin improving."
            ),
            (
                "Require ticker-specific evidence before promoting a parent dislocation "
                "into a trade thesis."
            ),
        ],
        "counts": {
            "sectors": len(classified),
            **{bucket: len(buckets[bucket]) for bucket in DISLOCATION_BUCKETS},
        },
    }


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
    dislocation_map = build_dislocation_map(market_state)
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
        "dislocation_map": {
            "counts": (dislocation_map or {}).get("counts"),
            "sector_classifications": {
                key: {
                    "classification": value.get("classification"),
                    "score": value.get("score"),
                }
                for key, value in (
                    (dislocation_map or {}).get("sector_classifications") or {}
                ).items()
            },
        },
    }
    fingerprint = _stable_fingerprint(fingerprint_payload)
    generated_evidence = [
        {
            "generated_by": "brain_maintainer",
            "kind": "macro_source_freshness",
            "as_of": now.isoformat(),
            "sources": sources,
            "market_state": market_state,
        }
    ]
    if dislocation_map:
        generated_evidence.append({
            "generated_by": "brain_maintainer",
            "kind": "macro_dislocation_map",
            "as_of": now.isoformat(),
            "dislocation_map": dislocation_map,
        })
    evidence = _generated_evidence(thesis.get("evidence")) + generated_evidence
    source_ref = dict(_json(thesis.get("source_ref"), {}) or {})
    source_ref["maintainer"] = {
        "kind": "macro_coverage",
        "evaluated_at": now.isoformat(),
        "fingerprint": fingerprint,
        "deterministic_fingerprint": fingerprint,
        "sources": sources,
        "market_state": market_state,
        "dislocation_map": dislocation_map,
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


async def _load_market_breadth_internals(pool: asyncpg.Pool) -> dict[str, Any] | None:
    row = await pool.fetchrow(
        """WITH ranked AS (
               SELECT symbol, ts, close::double precision AS close,
                      row_number() OVER (PARTITION BY symbol ORDER BY ts DESC) AS rn
                 FROM price_bar
                WHERE close IS NOT NULL
           ), per_symbol AS (
               SELECT symbol,
                      max(ts) FILTER (WHERE rn = 1) AS as_of,
                      max(close) FILTER (WHERE rn = 1) AS latest_close,
                      max(close) FILTER (WHERE rn = 2) AS prev_close,
                      avg(close) FILTER (WHERE rn <= 50) AS sma50,
                      avg(close) FILTER (WHERE rn <= 200) AS sma200,
                      max(close) FILTER (WHERE rn <= 252) AS high252,
                      min(close) FILTER (WHERE rn <= 252) AS low252,
                      count(*) AS bars
                 FROM ranked
                WHERE rn <= 252
             GROUP BY symbol
           )
           SELECT max(as_of) AS as_of,
                  count(*) FILTER (WHERE bars >= 50) AS symbol_count,
                  count(*) FILTER (WHERE bars >= 50 AND latest_close > prev_close) AS advancers,
                  count(*) FILTER (WHERE bars >= 50 AND latest_close < prev_close) AS decliners,
                  count(*) FILTER (WHERE bars >= 50 AND latest_close >= sma50) AS above_50d,
                  count(*) FILTER (WHERE bars >= 200 AND latest_close >= sma200) AS above_200d,
                  count(*) FILTER (
                    WHERE bars >= 252 AND latest_close >= high252 * 0.995
                  ) AS new_highs_252d,
                  count(*) FILTER (
                    WHERE bars >= 252 AND latest_close <= low252 * 1.005
                  ) AS new_lows_252d
             FROM per_symbol
            WHERE latest_close IS NOT NULL""",
    )
    if row is None or int(row["symbol_count"] or 0) == 0:
        return None
    symbol_count = int(row["symbol_count"] or 0)
    advancers = int(row["advancers"] or 0)
    decliners = int(row["decliners"] or 0)
    above_50d = int(row["above_50d"] or 0)
    above_200d = int(row["above_200d"] or 0)
    new_highs = int(row["new_highs_252d"] or 0)
    new_lows = int(row["new_lows_252d"] or 0)
    return {
        "as_of": _iso(row["as_of"]),
        "symbol_count": symbol_count,
        "advancers": advancers,
        "decliners": decliners,
        "advance_decline_ratio": _round_float(
            advancers / decliners if decliners else float(advancers)
        ),
        "pct_above_50d": _round_float(above_50d / symbol_count),
        "pct_above_200d": _round_float(above_200d / symbol_count),
        "new_highs_252d": new_highs,
        "new_lows_252d": new_lows,
        "new_high_low_spread": new_highs - new_lows,
        "source": "price_bar",
    }


async def _load_sector_relative_strength(pool: asyncpg.Pool) -> dict[str, Any] | None:
    rows = await pool.fetch(
        """WITH universe AS (
               SELECT DISTINCT symbol, COALESCE(NULLIF(sector, ''), 'Unknown') AS sector
                 FROM discovery_pool
                WHERE dropped_at IS NULL
           ), ranked AS (
               SELECT pb.symbol, u.sector, pb.ts, pb.close::double precision AS close,
                      row_number() OVER (PARTITION BY pb.symbol ORDER BY pb.ts DESC) AS rn
                 FROM price_bar pb
                 JOIN universe u ON u.symbol = pb.symbol
                WHERE pb.close IS NOT NULL
           ), per_symbol AS (
               SELECT symbol, sector,
                      max(ts) FILTER (WHERE rn = 1) AS as_of,
                      max(close) FILTER (WHERE rn = 1) AS latest_close,
                      max(close) FILTER (WHERE rn = 21) AS close_20d,
                      max(close) FILTER (WHERE rn = 61) AS close_60d,
                      max(close) FILTER (WHERE rn = 121) AS close_120d,
                      count(*) AS bars
                 FROM ranked
                WHERE rn <= 121
             GROUP BY symbol, sector
           )
           SELECT sector,
                  max(as_of) AS as_of,
                  count(*) FILTER (WHERE bars >= 21) AS symbol_count,
                  avg((latest_close / NULLIF(close_20d, 0)) - 1)
                    FILTER (WHERE bars >= 21 AND close_20d IS NOT NULL) AS return_20d,
                  avg((latest_close / NULLIF(close_60d, 0)) - 1)
                    FILTER (WHERE bars >= 61 AND close_60d IS NOT NULL) AS return_60d,
                  avg((latest_close / NULLIF(close_120d, 0)) - 1)
                    FILTER (WHERE bars >= 121 AND close_120d IS NOT NULL) AS return_120d
             FROM per_symbol
            WHERE latest_close IS NOT NULL
         GROUP BY sector
           HAVING count(*) FILTER (WHERE bars >= 21) >= 3
         ORDER BY return_20d DESC NULLS LAST, sector""",
    )
    sectors = []
    for row in rows:
        sectors.append({
            "sector": row["sector"],
            "as_of": _iso(row["as_of"]),
            "symbol_count": int(row["symbol_count"] or 0),
            "return_20d": _round_float(row["return_20d"]),
            "return_60d": _round_float(row["return_60d"]),
            "return_120d": _round_float(row["return_120d"]),
        })
    if not sectors:
        return None
    return {
        "as_of": sectors[0]["as_of"],
        "sectors": sectors,
        "leaders_20d": [item["sector"] for item in sectors[:3]],
        "laggards_20d": [item["sector"] for item in sectors[-3:]],
        "source": "price_bar+discovery_pool",
    }


async def _load_earnings_breadth(pool: asyncpg.Pool) -> dict[str, Any] | None:
    row = await pool.fetchrow(
        """WITH recent AS (
               SELECT er.symbol, er.direction,
                      COALESCE(NULLIF(dp.sector, ''), 'Unknown') AS sector
                 FROM estimate_revision er
            LEFT JOIN discovery_pool dp ON dp.symbol = er.symbol AND dp.dropped_at IS NULL
                WHERE er.detected_at > now() - interval '30 days'
           ), sector_stats AS (
               SELECT sector,
                      jsonb_build_object(
                        'signals', count(*),
                        'symbols', count(DISTINCT symbol),
                        'up', count(*) FILTER (WHERE direction = 'up'),
                        'down', count(*) FILTER (WHERE direction = 'down'),
                        'mixed', count(*) FILTER (WHERE direction = 'mixed'),
                        'coverage_change', count(*) FILTER (WHERE direction = 'coverage_change')
                      ) AS stats
                 FROM recent
             GROUP BY sector
           ), totals AS (
               SELECT count(*) AS signals,
                      count(DISTINCT symbol) AS symbol_count,
                      count(*) FILTER (WHERE direction = 'up') AS up,
                      count(*) FILTER (WHERE direction = 'down') AS down,
                      count(*) FILTER (WHERE direction = 'mixed') AS mixed,
                      count(*) FILTER (WHERE direction = 'coverage_change') AS coverage_change
                 FROM recent
           )
           SELECT totals.*,
                  (
                    SELECT jsonb_object_agg(sector, stats ORDER BY sector)
                      FROM sector_stats
                  ) AS sectors
             FROM totals""",
    )
    if row is None or int(row["signals"] or 0) == 0:
        return None
    signals = int(row["signals"] or 0)
    up = int(row["up"] or 0)
    down = int(row["down"] or 0)
    mixed = int(row["mixed"] or 0)
    coverage_change = int(row["coverage_change"] or 0)
    return {
        "window_days": 30,
        "signals": signals,
        "symbol_count": int(row["symbol_count"] or 0),
        "up": up,
        "down": down,
        "mixed": mixed,
        "coverage_change": coverage_change,
        "net_revision_breadth": _round_float((up - down) / signals),
        "sectors": _json(row["sectors"], {}) or {},
        "source": "estimate_revision",
    }


async def _load_sector_news_sentiment(pool: asyncpg.Pool) -> dict[str, Any] | None:
    rows = await pool.fetch(
        """WITH universe AS (
               SELECT DISTINCT symbol, COALESCE(NULLIF(sector, ''), 'Unknown') AS sector
                 FROM discovery_pool
                WHERE dropped_at IS NULL
           ), recent AS (
               SELECT u.sector,
                      count(*) AS articles_14d,
                      avg(na.sentiment_polarity) FILTER (
                        WHERE na.sentiment_polarity IS NOT NULL
                      ) AS avg_polarity
                 FROM news_article na
                 JOIN universe u ON u.symbol = na.symbol
                WHERE na.published_at > now() - interval '14 days'
             GROUP BY u.sector
           ), prior AS (
               SELECT u.sector,
                      count(*) AS articles_60d_prior
                 FROM news_article na
                 JOIN universe u ON u.symbol = na.symbol
                WHERE na.published_at > now() - interval '74 days'
                  AND na.published_at <= now() - interval '14 days'
             GROUP BY u.sector
           )
           SELECT COALESCE(recent.sector, prior.sector) AS sector,
                  COALESCE(recent.articles_14d, 0) AS articles_14d,
                  recent.avg_polarity,
                  COALESCE(prior.articles_60d_prior, 0) AS articles_60d_prior
             FROM recent
        FULL JOIN prior ON prior.sector = recent.sector
         ORDER BY sector""",
    )
    sectors: dict[str, dict[str, Any]] = {}
    for row in rows:
        sector = row["sector"]
        if not sector:
            continue
        recent = int(row["articles_14d"] or 0)
        prior = int(row["articles_60d_prior"] or 0)
        baseline_14d = prior / (60.0 / 14.0) if prior > 0 else 0.0
        attention_ratio = recent / baseline_14d if baseline_14d > 0 else (1.0 if recent else 0.0)
        sectors[sector] = {
            "articles_14d": recent,
            "articles_60d_prior": prior,
            "avg_polarity": _round_float(row["avg_polarity"]),
            "attention_ratio": _round_float(attention_ratio),
        }
    if not sectors:
        return None
    return {
        "window_days": 14,
        "baseline_days": 60,
        "sectors": sectors,
        "source": "news_article",
    }


async def _load_credit_internals_trend(pool: asyncpg.Pool) -> dict[str, Any] | None:
    row = await pool.fetchrow(
        """WITH obs AS (
               SELECT source_ts,
                      NULLIF(payload->>'value', '.')::double precision AS value,
                      row_number() OVER (ORDER BY source_ts DESC) AS rn
                 FROM ingest_event
                WHERE source = 'fred'
                  AND payload->>'series' = 'BAMLH0A0HYM2'
                  AND NULLIF(payload->>'value', '.') IS NOT NULL
           )
           SELECT max(source_ts) FILTER (WHERE rn = 1) AS as_of,
                  max(value) FILTER (WHERE rn = 1) AS latest,
                  max(value) FILTER (WHERE rn = 5) AS prior_5_obs,
                  max(value) FILTER (WHERE rn = 20) AS prior_20_obs,
                  count(*) AS observations
             FROM obs
            WHERE rn <= 20""",
    )
    if row is None or row["latest"] is None:
        return None
    latest = float(row["latest"])
    prior_5 = None if row["prior_5_obs"] is None else float(row["prior_5_obs"])
    prior_20 = None if row["prior_20_obs"] is None else float(row["prior_20_obs"])
    delta_5 = None if prior_5 is None else latest - prior_5
    delta_20 = None if prior_20 is None else latest - prior_20
    trend = "stable"
    if delta_5 is not None:
        if delta_5 <= -0.15:
            trend = "tightening"
        elif delta_5 >= 0.15:
            trend = "widening"
    return {
        "as_of": _iso(row["as_of"]),
        "latest_hy_oas_pct": _round_float(latest),
        "delta_5_obs_pct": _round_float(delta_5),
        "delta_20_obs_pct": _round_float(delta_20),
        "trend": trend,
        "observations": int(row["observations"] or 0),
        "source": "fred:BAMLH0A0HYM2",
    }


async def _load_macro_internals(pool: asyncpg.Pool) -> dict[str, Any]:
    breadth, sector_rs, earnings, news, credit = await asyncio.gather(
        _load_market_breadth_internals(pool),
        _load_sector_relative_strength(pool),
        _load_earnings_breadth(pool),
        _load_sector_news_sentiment(pool),
        _load_credit_internals_trend(pool),
    )
    out: dict[str, Any] = {}
    if breadth:
        out["market_breadth_internals"] = breadth
    if sector_rs:
        out["sector_relative_strength"] = sector_rs
    if earnings:
        out["earnings_breadth"] = earnings
    if news:
        out["sector_news_sentiment"] = news
    if credit:
        out["credit_internals_trend"] = credit
    return out


async def _load_market_state(pool: asyncpg.Pool) -> dict[str, Any] | None:
    row = await pool.fetchrow(
        """SELECT as_of, regime, capitulation, indicators, subsector_rs
             FROM market_state
         ORDER BY as_of DESC
            LIMIT 1""",
    )
    if row is None:
        return None
    internals = await _load_macro_internals(pool)
    indicators = _json(row["indicators"], {})
    if not isinstance(indicators, dict):
        indicators = {}
    indicators = {**indicators, **internals}
    subsector_rs = _json(row["subsector_rs"], {})
    if not isinstance(subsector_rs, dict):
        subsector_rs = {}
    sector_rs = internals.get("sector_relative_strength")
    if isinstance(sector_rs, dict):
        subsector_rs = {
            item["sector"]: item
            for item in sector_rs.get("sectors", [])
            if isinstance(item, dict) and item.get("sector")
        }
    return {
        "as_of": _iso(row["as_of"]),
        "regime": row["regime"],
        "capitulation": bool(row["capitulation"]),
        "indicators": indicators,
        "subsector_rs": subsector_rs,
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
                      summary, strength, polarity, url, updated_at
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
                "updated_at": _iso(row["updated_at"]),
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
