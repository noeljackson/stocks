"""Evidence acquisition state shared by cognition services."""

from __future__ import annotations

import json

import asyncpg

EVIDENCE_REQUIREMENTS = {
    "price_history": {
        "source_type": "price",
        "priority": "blocking",
        "reason": "Need daily OHLCV bars before evaluating technical setup or context freshness.",
        "fetch_actions": ["fmp_price_backfill"],
    },
    "company_facts": {
        "source_type": "fundamentals",
        "priority": "high",
        "reason": "Need SEC/XBRL company facts before making fundamental claims.",
        "fetch_actions": ["sec_company_tickers_cik_lookup", "sec_companyfacts_xbrl"],
    },
    "recent_news": {
        "source_type": "news",
        "priority": "high",
        "reason": (
            "Need recent narrative evidence before deciding whether the market has "
            "new information."
        ),
        "fetch_actions": ["fmp_news", "massive_news", "llm_sentiment_scoring"],
    },
    "analyst_estimates": {
        "source_type": "estimates",
        "priority": "high",
        "reason": "Need analyst estimate snapshots before evaluating revision/consensus drift.",
        "fetch_actions": ["fmp_analyst_estimates"],
    },
}


async def load_evidence_counts(pool: asyncpg.Pool, symbol: str) -> dict[str, int]:
    row = await pool.fetchrow(
        """SELECT
              (SELECT count(*) FROM price_bar WHERE symbol = $1) AS price_bars,
              (SELECT count(*) FROM company_fact WHERE symbol = $1) AS company_facts,
              (SELECT count(*) FROM news_article
                WHERE symbol = $1
                  AND published_at > now() - interval '30 days') AS recent_news,
              (SELECT count(*) FROM estimate_snapshot WHERE symbol = $1) AS estimate_snapshots
        """,
        symbol,
    )
    if row is None:
        return {}
    return {k: int(row[k] or 0) for k in row.keys()}


def assess_evidence_requirements(evidence_counts: dict[str, int]) -> list[dict]:
    missing = []
    checks = {
        "price_history": evidence_counts.get("price_bars", 0) > 0,
        "company_facts": evidence_counts.get("company_facts", 0) > 0,
        "recent_news": evidence_counts.get("recent_news", 0) > 0,
        "analyst_estimates": evidence_counts.get("estimate_snapshots", 0) > 0,
    }
    for key, satisfied in checks.items():
        if satisfied:
            continue
        spec = EVIDENCE_REQUIREMENTS[key]
        missing.append({
            "requirement_key": key,
            "source_type": spec["source_type"],
            "priority": spec["priority"],
            "reason": spec["reason"],
            "fetch_actions": spec["fetch_actions"],
            "source_ref": {
                "counts": evidence_counts,
                "fetch_actions": spec["fetch_actions"],
            },
        })
    return missing


async def sync_evidence_requirements(
    pool: asyncpg.Pool,
    symbol: str,
    evidence_counts: dict[str, int],
) -> list[dict]:
    missing = assess_evidence_requirements(evidence_counts)
    missing_by_key = {r["requirement_key"]: r for r in missing}
    now_ref = json.dumps({"counts": evidence_counts})

    for key, spec in EVIDENCE_REQUIREMENTS.items():
        if key in missing_by_key:
            req = missing_by_key[key]
            await pool.execute(
                """INSERT INTO evidence_requirement
                     (symbol, requirement_key, source_type, reason, priority,
                      blocking_state, next_retry_at, source_ref)
                   VALUES ($1, $2, $3, $4, $5, 'missing', now() + interval '30 minutes', $6::jsonb)
                   ON CONFLICT (symbol, requirement_key) DO UPDATE SET
                     source_type = EXCLUDED.source_type,
                     reason = EXCLUDED.reason,
                     priority = EXCLUDED.priority,
                     blocking_state = CASE
                         WHEN evidence_requirement.blocking_state = 'fetching' THEN 'fetching'
                         ELSE 'missing'
                     END,
                     attempts = CASE
                         WHEN evidence_requirement.next_retry_at IS NOT NULL
                          AND evidence_requirement.next_retry_at <= now()
                         THEN evidence_requirement.attempts + 1
                         ELSE evidence_requirement.attempts
                     END,
                     next_retry_at = CASE
                         WHEN evidence_requirement.next_retry_at IS NULL
                           OR evidence_requirement.next_retry_at <= now()
                         THEN EXCLUDED.next_retry_at
                         ELSE evidence_requirement.next_retry_at
                     END,
                     source_ref = EXCLUDED.source_ref,
                     satisfied_at = NULL,
                     updated_at = now()""",
                symbol,
                key,
                req["source_type"],
                req["reason"],
                req["priority"],
                json.dumps(req["source_ref"]),
            )
        else:
            await pool.execute(
                """INSERT INTO evidence_requirement
                     (symbol, requirement_key, source_type, reason, priority,
                      blocking_state, source_ref, satisfied_at)
                   VALUES ($1, $2, $3, $4, $5, 'satisfied', $6::jsonb, now())
                   ON CONFLICT (symbol, requirement_key) DO UPDATE SET
                     blocking_state = 'satisfied',
                     source_ref = EXCLUDED.source_ref,
                     satisfied_at = COALESCE(evidence_requirement.satisfied_at, now()),
                     next_retry_at = NULL,
                     last_error = NULL,
                     updated_at = now()""",
                symbol,
                key,
                spec["source_type"],
                spec["reason"],
                spec["priority"],
                now_ref,
            )
    return missing


async def load_open_evidence_requirements(pool: asyncpg.Pool, symbol: str) -> list[dict]:
    rows = await pool.fetch(
        """SELECT requirement_key, source_type, reason, priority, blocking_state,
                  attempts, next_retry_at, last_error, source_ref, updated_at
             FROM evidence_requirement
            WHERE symbol = $1
              AND blocking_state <> 'satisfied'
         ORDER BY
              CASE priority
                   WHEN 'blocking' THEN 0
                   WHEN 'high' THEN 1
                   WHEN 'medium' THEN 2
                   ELSE 3
              END,
              updated_at DESC""",
        symbol,
    )
    return [_row_to_requirement(row) for row in rows]


def _row_to_requirement(row: asyncpg.Record) -> dict:
    source_ref = row["source_ref"]
    if isinstance(source_ref, str):
        source_ref = json.loads(source_ref)
    return {
        "requirement_key": row["requirement_key"],
        "source_type": row["source_type"],
        "reason": row["reason"],
        "priority": row["priority"],
        "blocking_state": row["blocking_state"],
        "attempts": row["attempts"],
        "next_retry_at": row["next_retry_at"].isoformat() if row["next_retry_at"] else None,
        "last_error": row["last_error"],
        "source_ref": source_ref,
        "updated_at": row["updated_at"].isoformat(),
    }
