"""Context maintainer service (SPEC §3, §5.2).

Phase A (#7): single-symbol CLI that reads a ticker's event, fundamental,
price, news, and estimate-revision evidence, calls GLM-5.1 with
prompts/synthesize-context.md, and persists a new ticker_context row.

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
from .evidence import load_evidence_counts, load_source_health, sync_evidence_requirements
from .llm import TransportConfig, detect, new_provider
from .prompts import AsyncpgRecorder, invoke, load
from .research import load_research_evidence, refresh_research_evidence

log = logging.getLogger("context_maintainer")


class BlockingEvidenceMissing(RuntimeError):
    def __init__(self, symbol: str, missing: list[dict]) -> None:
        self.symbol = symbol
        self.missing = missing
        keys = ", ".join(r["requirement_key"] for r in missing)
        super().__init__(f"{symbol} missing blocking evidence: {keys}")


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


async def _load_company_facts(pool: asyncpg.Pool, symbol: str) -> list[dict]:
    """Latest 2 observations per concept from company_fact (#32). Gives the
    LLM real financial metrics to use in the structural band's fundamentals."""
    rows = await pool.fetch(
        """SELECT concept, period_end, period_start, value, unit,
                  form, fiscal_year, fiscal_period, filed_at
             FROM (
                 SELECT *,
                        row_number() OVER (
                            PARTITION BY concept
                            ORDER BY period_end DESC, filed_at DESC
                        ) AS rn
                   FROM company_fact
                  WHERE symbol = $1
             ) sub
            WHERE rn <= 2
         ORDER BY concept, period_end DESC""",
        symbol,
    )
    out = []
    for r in rows:
        out.append({
            "concept": r["concept"],
            "period_end": r["period_end"].isoformat() if r["period_end"] else None,
            "period_start": r["period_start"].isoformat() if r["period_start"] else None,
            "value": float(r["value"]),
            "unit": r["unit"],
            "form": r["form"],
            "fiscal_year": r["fiscal_year"],
            "fiscal_period": r["fiscal_period"],
            "filed_at": r["filed_at"].isoformat() if r["filed_at"] else None,
        })
    return out


def _f(value) -> float | None:
    return None if value is None else float(value)


def _round(value: float | None, places: int = 2) -> float | None:
    return None if value is None else round(value, places)


def _build_price_snapshot(rows) -> dict | None:
    """Summarize daily price state from oldest-to-newest rows.

    The SMA ribbon and context both mean trading-day moving averages. A 200-day
    SMA must be computed from 200 daily closes even when the current chart range
    is much shorter.
    """
    if not rows:
        return None
    ordered = sorted(rows, key=lambda r: r["ts"])
    latest = ordered[-1]
    closes = [_f(r["close"]) for r in ordered if r["close"] is not None]
    volumes = [_f(r["volume"]) for r in ordered if r["volume"] is not None]
    if not closes:
        return None

    def sma(window: int) -> float | None:
        if len(closes) < window:
            return None
        return sum(closes[-window:]) / window

    def pct_vs(value: float | None) -> float | None:
        if value in (None, 0):
            return None
        return ((closes[-1] - value) / value) * 100.0

    highs = [_f(r["high"]) for r in ordered if r["high"] is not None]
    high_window = highs[-252:] if highs else []
    high_252d = max(high_window) if high_window else None
    volume_avg_20 = sum(volumes[-20:]) / 20 if len(volumes) >= 20 else None
    latest_volume = _f(latest["volume"])
    volume_vs_20d = (
        latest_volume / volume_avg_20
        if latest_volume is not None and volume_avg_20 not in (None, 0)
        else None
    )
    sma_20 = sma(20)
    sma_50 = sma(50)
    sma_100 = sma(100)
    sma_200 = sma(200)
    return {
        "as_of": latest["ts"].isoformat(),
        "close": _round(closes[-1]),
        "sma_20": _round(sma_20),
        "sma_50": _round(sma_50),
        "sma_100": _round(sma_100),
        "sma_200": _round(sma_200),
        "pct_vs_sma_20": _round(pct_vs(sma_20)),
        "pct_vs_sma_50": _round(pct_vs(sma_50)),
        "pct_vs_sma_100": _round(pct_vs(sma_100)),
        "pct_vs_sma_200": _round(pct_vs(sma_200)),
        "available_window_high": _round(high_252d),
        "pct_vs_available_window_high": _round(pct_vs(high_252d)),
        "volume": _round(latest_volume, 0),
        "volume_vs_20d_avg": _round(volume_vs_20d),
        "bars_used": len(closes),
    }


async def _load_price_snapshot(pool: asyncpg.Pool, symbol: str) -> dict | None:
    rows = await pool.fetch(
        """SELECT ts, open, high, low, close, volume
             FROM price_bar
            WHERE symbol = $1
         ORDER BY ts DESC
            LIMIT 260""",
        symbol,
    )
    return _build_price_snapshot(rows)


async def _load_recent_news(
    pool: asyncpg.Pool,
    symbol: str,
    since: dt.datetime | None,
    limit: int = 20,
) -> list[dict]:
    if since is None:
        rows = await pool.fetch(
            """SELECT id, title, publisher, published_at, source, sentiment,
                      sentiment_polarity, sentiment_confidence, sentiment_rationale, url
                 FROM news_article
                WHERE symbol = $1
             ORDER BY published_at DESC
                LIMIT $2""",
            symbol,
            limit,
        )
    else:
        rows = await pool.fetch(
            """SELECT id, title, publisher, published_at, source, sentiment,
                      sentiment_polarity, sentiment_confidence, sentiment_rationale, url
                 FROM news_article
                WHERE symbol = $1 AND published_at > $2
             ORDER BY published_at DESC
                LIMIT $3""",
            symbol,
            since,
            limit,
        )
    return [
        {
            "id": r["id"],
            "title": r["title"],
            "publisher": r["publisher"],
            "published_at": r["published_at"].isoformat(),
            "source": r["source"],
            "sentiment": r["sentiment"],
            "sentiment_polarity": _f(r["sentiment_polarity"]),
            "sentiment_confidence": r["sentiment_confidence"],
            "sentiment_rationale": r["sentiment_rationale"],
            "url": r["url"],
        }
        for r in rows
    ]


async def _load_estimate_revisions(
    pool: asyncpg.Pool,
    symbol: str,
    since: dt.datetime | None,
    limit: int = 20,
) -> list[dict]:
    if since is None:
        rows = await pool.fetch(
            """SELECT id, fiscal_period_end, period_kind, eps_delta, eps_delta_pct,
                      revenue_delta, revenue_delta_pct, direction, detected_at
                 FROM estimate_revision
                WHERE symbol = $1
             ORDER BY detected_at DESC
                LIMIT $2""",
            symbol,
            limit,
        )
    else:
        rows = await pool.fetch(
            """SELECT id, fiscal_period_end, period_kind, eps_delta, eps_delta_pct,
                      revenue_delta, revenue_delta_pct, direction, detected_at
                 FROM estimate_revision
                WHERE symbol = $1 AND detected_at > $2
             ORDER BY detected_at DESC
                LIMIT $3""",
            symbol,
            since,
            limit,
        )
    return [
        {
            "id": r["id"],
            "fiscal_period_end": r["fiscal_period_end"].isoformat(),
            "period_kind": r["period_kind"],
            "direction": r["direction"],
            "eps_delta": _f(r["eps_delta"]),
            "eps_delta_pct": _f(r["eps_delta_pct"]),
            "revenue_delta": _f(r["revenue_delta"]),
            "revenue_delta_pct": _f(r["revenue_delta_pct"]),
            "detected_at": r["detected_at"].isoformat(),
        }
        for r in rows
    ]


async def _load_analyst_opinion(pool: asyncpg.Pool, symbol: str) -> dict:
    target = await pool.fetchrow(
        """SELECT target_high, target_low, target_consensus, target_median, snapshot_at
             FROM analyst_price_target_snapshot
            WHERE symbol = $1
         ORDER BY snapshot_at DESC
            LIMIT 1""",
        symbol,
    )
    recommendation = await pool.fetchrow(
        """SELECT as_of_date, strong_buy, buy, hold, sell, strong_sell, snapshot_at
             FROM analyst_recommendation_snapshot
            WHERE symbol = $1
         ORDER BY snapshot_at DESC
            LIMIT 1""",
        symbol,
    )
    event_rows = await pool.fetch(
        """SELECT published_at, news_title, news_url, analyst_company,
                  price_target, adj_price_target, price_when_posted, news_publisher
             FROM analyst_price_target_event
            WHERE symbol = $1
         ORDER BY published_at DESC
            LIMIT 10""",
        symbol,
    )
    return {
        "price_target_consensus": None if target is None else {
            "target_high": _f(target["target_high"]),
            "target_low": _f(target["target_low"]),
            "target_consensus": _f(target["target_consensus"]),
            "target_median": _f(target["target_median"]),
            "snapshot_at": target["snapshot_at"].isoformat(),
        },
        "recommendation_mix": None if recommendation is None else {
            "as_of_date": recommendation["as_of_date"].isoformat()
            if recommendation["as_of_date"]
            else None,
            "strong_buy": recommendation["strong_buy"],
            "buy": recommendation["buy"],
            "hold": recommendation["hold"],
            "sell": recommendation["sell"],
            "strong_sell": recommendation["strong_sell"],
            "snapshot_at": recommendation["snapshot_at"].isoformat(),
        },
        "recent_price_target_events": [
            {
                "published_at": r["published_at"].isoformat(),
                "title": r["news_title"],
                "url": r["news_url"],
                "analyst_company": r["analyst_company"],
                "price_target": _f(r["price_target"]),
                "adj_price_target": _f(r["adj_price_target"]),
                "price_when_posted": _f(r["price_when_posted"]),
                "publisher": r["news_publisher"],
            }
            for r in event_rows
        ],
    }


async def _load_evidence_items(
    pool: asyncpg.Pool,
    symbol: str,
    since: dt.datetime | None,
    limit: int = 50,
) -> list[dict]:
    if since is None:
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
    else:
        rows = await pool.fetch(
            """SELECT id, kind, observed_at, source, source_id, source_ref,
                      summary, strength, polarity, url
                 FROM evidence_item
                WHERE symbol = $1 AND observed_at > $2
                  AND NOT (
                      kind = 'product_research'
                      AND source = 'web_research'
                      AND COALESCE((source_ref->'relevance'->>'accepted')::boolean, false) = false
                  )
             ORDER BY observed_at DESC, id DESC
                LIMIT $3""",
            symbol,
            since,
            limit,
        )
    out = []
    for r in rows:
        source_ref = r["source_ref"]
        if isinstance(source_ref, str):
            source_ref = json.loads(source_ref)
        out.append({
            "id": r["id"],
            "kind": r["kind"],
            "observed_at": r["observed_at"].isoformat(),
            "source": r["source"],
            "source_id": r["source_id"],
            "summary": r["summary"],
            "strength": _f(r["strength"]),
            "polarity": _f(r["polarity"]),
            "url": r["url"],
            "source_ref": source_ref,
        })
    return out


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


def _build_user_message(
    symbol: str,
    prior: dict | None,
    events: list[dict],
    facts: list[dict],
    price_snapshot: dict | None,
    news: list[dict],
    estimate_revisions: list[dict],
    analyst_opinion: dict,
    research_evidence: list[dict],
    evidence_items: list[dict],
    evidence_counts: dict[str, object],
    missing_evidence: list[dict],
    today: str,
) -> str:
    """The system prompt is the rendered template. This user message carries
    the actual data the LLM should reason over."""
    return json.dumps(
        {
            "symbol": symbol,
            "today": today,
            "prior_context": prior,
            "new_events": events,
            "company_facts": facts,
            "price_snapshot": price_snapshot,
            "recent_news": news,
            "estimate_revisions": estimate_revisions,
            "analyst_opinion": analyst_opinion,
            "research_evidence": research_evidence,
            "evidence_items": evidence_items,
            "evidence_counts": evidence_counts,
            "missing_evidence": missing_evidence,
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
    market: dict,
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
           VALUES ($1, $2, $3::jsonb, $4, $5::jsonb, $4, $6::jsonb, $4, $4)""",
        symbol,
        new_version,
        json.dumps(structural),
        now,
        json.dumps(narrative),
        json.dumps(market),
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

        # 2. Load prior context (if any), new events, AND structured facts.
        prior = await _load_prior_context(pool, symbol)
        since = dt.datetime.fromisoformat(prior["as_of"]) if prior else None
        events = await _load_events(pool, symbol, since, limit)
        facts = await _load_company_facts(pool, symbol)
        price_snapshot = await _load_price_snapshot(pool, symbol)
        news = await _load_recent_news(pool, symbol, since)
        estimate_revisions = await _load_estimate_revisions(pool, symbol, since)
        analyst_opinion = await _load_analyst_opinion(pool, symbol)
        evidence_items = await _load_evidence_items(pool, symbol, since)
        await refresh_research_evidence(
            pool,
            symbol,
            context=prior,
        )
        research_evidence = await load_research_evidence(pool, symbol)
        evidence_counts = await load_evidence_counts(pool, symbol)
        source_health = await load_source_health(pool)
        missing_evidence = await sync_evidence_requirements(
            pool, symbol, evidence_counts, source_health,
        )
        log.info(
            "symbol=%s prior=%s events_count=%d facts_count=%d "
            "news_count=%d revisions_count=%d analyst_opinion_events=%d "
            "evidence_items=%d research_count=%d "
            "price=%s missing_evidence=%d",
            symbol,
            f"v{prior['version']}" if prior else "none",
            len(events),
            len(facts),
            len(news),
            len(estimate_revisions),
            len(analyst_opinion.get("recent_price_target_events", [])),
            len(evidence_items),
            len(research_evidence),
            "yes" if price_snapshot else "no",
            len(missing_evidence),
        )
        blocking_evidence = [r for r in missing_evidence if r["priority"] == "blocking"]
        if blocking_evidence:
            log.info(
                "symbol=%s blocking_evidence=%s; skipping context LLM",
                symbol,
                [r["requirement_key"] for r in blocking_evidence],
            )
            raise BlockingEvidenceMissing(symbol, blocking_evidence)

        # Skip only when there is no newly timestamped signal and the prior row
        # already has the price-aware market band. Legacy contexts need one
        # refresh so they stop looking like blank slates.
        prior_has_market = bool(prior and prior.get("market"))
        if (
            not events
            and not facts
            and not news
            and not estimate_revisions
            and not evidence_items
            and not research_evidence
            and prior is not None
            and prior_has_market
        ):
            log.info("no new signal since v%d — skipping refresh", prior["version"])
            return prior["version"]

        # 3. Build prompt + call LLM with audit recorder.
        registry = load(_repo_root() / "prompts")
        prompt = registry.get("synthesize-context")
        if prompt is None:
            raise RuntimeError("prompts/synthesize-context.md missing")

        today = dt.date.today().isoformat()
        user_msg = _build_user_message(
            symbol,
            prior,
            events,
            facts,
            price_snapshot,
            news,
            estimate_revisions,
            analyst_opinion,
            research_evidence,
            evidence_items,
            evidence_counts,
            missing_evidence,
            today,
        )
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
        market = parsed.get("market", {})
        new_version = await _persist_context(
            pool, symbol, structural, narrative, market, prior["version"] if prior else None
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
