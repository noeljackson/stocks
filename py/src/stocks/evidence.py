"""Evidence acquisition state shared by cognition services."""

from __future__ import annotations

import datetime as dt
import json

import asyncpg

FRESHNESS_TARGETS_MINUTES = {
    "price_history": 30,
    "company_facts": 6 * 60,
    "recent_news": 30,
    "analyst_estimates": 30,
    "product_research": 30,
}

SOURCE_HEALTH_BY_REQUIREMENT = {
    "price_history": ["fmp_price"],
    "company_facts": ["edgar", "xbrl"],
    "recent_news": ["fmp_news", "massive_news"],
    "analyst_estimates": ["fmp_estimates"],
    "product_research": ["web_research"],
}

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
            "Need recent narrative evidence before deciding whether the market has new information."
        ),
        "fetch_actions": ["fmp_news", "massive_news", "llm_sentiment_scoring"],
    },
    "analyst_estimates": {
        "source_type": "estimates",
        "priority": "high",
        "reason": "Need analyst estimate snapshots before evaluating revision/consensus drift.",
        "fetch_actions": ["fmp_analyst_estimates"],
    },
    "product_research": {
        "source_type": "web_research",
        "priority": "high",
        "reason": (
            "Need product/theme web research before claiming public evidence "
            "does or does not exist."
        ),
        "fetch_actions": ["gdelt_doc_search", "bing_news_rss_search"],
    },
}


def _iso(value) -> str | None:
    return value.isoformat() if value is not None else None


def _task_json(task: dict) -> dict:
    due_at = task["due_at"]
    return {
        "action": task["action"],
        "provider": task["provider"],
        "state": task["state"],
        "due_at": _iso(due_at) if hasattr(due_at, "isoformat") else due_at,
        "next_retry_at": task["next_retry_at"],
    }


def _parse_dt(value) -> dt.datetime | None:
    if value is None or isinstance(value, dt.datetime):
        return value
    if isinstance(value, str):
        normalized = value.replace("Z", "+00:00")
        try:
            return dt.datetime.fromisoformat(normalized)
        except ValueError:
            return None
    return None


def _latest_dt(values) -> dt.datetime | None:
    parsed = [v for v in (_parse_dt(value) for value in values) if v is not None]
    return max(parsed) if parsed else None


def _source_health_last_check(
    requirement_key: str,
    source_health: dict[str, dict] | None,
) -> dt.datetime | None:
    sources = SOURCE_HEALTH_BY_REQUIREMENT.get(requirement_key, [])
    rows = [source_health[s] for s in sources if source_health and s in source_health]
    return _latest_dt(
        row.get("last_success_at") or row.get("last_started_at") or row.get("updated_at")
        for row in rows
    )


def source_task_due_at(
    requirement_key: str,
    *,
    last_check_at: dt.datetime | None,
    now: dt.datetime | None = None,
) -> dt.datetime:
    now = now or dt.datetime.now(dt.UTC)
    if last_check_at is None:
        return now
    minutes = FRESHNESS_TARGETS_MINUTES.get(requirement_key, 30)
    return last_check_at + dt.timedelta(minutes=minutes)


def satisfied_source_task_state(
    requirement_key: str,
    *,
    evidence_counts: dict,
    source_health: dict[str, dict] | None,
    now: dt.datetime | None = None,
) -> tuple[str, dt.datetime, str]:
    """Return recurring source-task state for a requirement with evidence.

    A satisfied evidence requirement means cognition can reason with the data it
    has. The source task remains the freshness contract: once the last relevant
    check ages past the SLA, it becomes queued without turning the symbol blank.
    """
    now = now or dt.datetime.now(dt.UTC)
    source_check_at = (
        None
        if requirement_key == "product_research"
        else _source_health_last_check(requirement_key, source_health)
    )
    symbol_check_at = {
        "price_history": _parse_dt(evidence_counts.get("price_last_bar_at")),
        "company_facts": _parse_dt(evidence_counts.get("company_fact_last_ingested_at")),
        "recent_news": _parse_dt(evidence_counts.get("news_last_ingested_at")),
        "analyst_estimates": _parse_dt(evidence_counts.get("estimate_snapshot_last_at")),
        "product_research": _latest_dt([
            evidence_counts.get("research_run_last_at"),
            evidence_counts.get("research_evidence_last_retrieved_at"),
        ]),
    }.get(requirement_key)
    last_check_at = _latest_dt([source_check_at, symbol_check_at])
    due_at = source_task_due_at(requirement_key, last_check_at=last_check_at, now=now)
    if last_check_at is None:
        return "queued", due_at, "freshness_not_checked"
    if due_at <= now:
        return "queued", now, "freshness_due"
    return "satisfied", due_at, "fresh"


def provider_for_fetch_action(action: str, source_type: str) -> str:
    if action.startswith("fmp_"):
        return "fmp"
    if action.startswith("massive_"):
        return "massive"
    if action.startswith("sec_"):
        return "sec"
    if action.startswith("gdelt_"):
        return "gdelt"
    if action.startswith("bing_"):
        return "bing"
    if action.startswith("llm_"):
        return "llm"
    return source_type


def source_task_state(blocking_state: str, acquisition_state: str | None) -> str:
    if blocking_state == "satisfied":
        return "satisfied"
    if blocking_state == "fetching":
        return "fetching"
    if acquisition_state == "rate_limited":
        return "rate_limited"
    if blocking_state == "blocked":
        return "failed"
    if acquisition_state in {
        "source_checked_no_new_rows",
        "source_checked_no_relevant_rows",
        "no_relevant_symbol_evidence_after_success",
    }:
        return "no_rows"
    return "queued"


def _task_due_at(state: str, retry_after_at: str | None) -> dt.datetime | str:
    if retry_after_at:
        return retry_after_at
    now = dt.datetime.now(dt.UTC)
    if state in {"no_rows", "failed", "blocked"}:
        return now + dt.timedelta(minutes=30)
    return now


def build_source_tasks(symbol: str, requirement: dict) -> list[dict]:
    state = source_task_state(
        requirement["blocking_state"],
        requirement.get("state_reason"),
    )
    tasks = []
    for action in requirement.get("fetch_actions", []):
        provider = provider_for_fetch_action(action, requirement["source_type"])
        tasks.append(
            {
                "source_type": requirement["source_type"],
                "requirement_key": requirement["requirement_key"],
                "action": action,
                "scope": "symbol",
                "target_id": symbol,
                "provider": provider,
                "limiter_key": provider,
                "state": state,
                "priority": requirement["priority"],
                "due_at": _task_due_at(state, requirement.get("retry_after_at")),
                "attempts": requirement.get("attempts", 0),
                "next_retry_at": requirement.get("retry_after_at"),
                "last_error": requirement.get("last_error"),
                "source_ref": {
                    "acquisition_state": requirement.get("state_reason"),
                    "evidence_counts": requirement.get("source_ref", {}).get("counts", {}),
                    "source_health": requirement.get("source_ref", {}).get("source_health", []),
                },
            }
        )
    return tasks


def build_satisfied_source_tasks(
    symbol: str,
    requirement_key: str,
    spec: dict,
    evidence_counts: dict,
    source_health: dict[str, dict] | None,
) -> list[dict]:
    state, due_at, state_reason = satisfied_source_task_state(
        requirement_key,
        evidence_counts=evidence_counts,
        source_health=source_health,
    )
    tasks = []
    for action in spec.get("fetch_actions", []):
        provider = provider_for_fetch_action(action, spec["source_type"])
        tasks.append(
            {
                "source_type": spec["source_type"],
                "requirement_key": requirement_key,
                "action": action,
                "scope": "symbol",
                "target_id": symbol,
                "provider": provider,
                "limiter_key": provider,
                "state": state,
                "priority": spec["priority"],
                "due_at": due_at,
                "attempts": 0,
                "next_retry_at": None,
                "last_error": None,
                "source_ref": {
                    "acquisition_state": state_reason,
                    "evidence_counts": evidence_counts,
                    "source_health": [
                        source_health[s]
                        for s in SOURCE_HEALTH_BY_REQUIREMENT.get(requirement_key, [])
                        if source_health and s in source_health
                    ],
                },
            }
        )
    return tasks


async def load_evidence_counts(pool: asyncpg.Pool, symbol: str) -> dict[str, object]:
    row = await pool.fetchrow(
        """SELECT
              (SELECT count(*) FROM price_bar WHERE symbol = $1) AS price_bars,
              (SELECT max(ts) FROM price_bar WHERE symbol = $1) AS price_last_bar_at,
              (SELECT count(*) FROM company_fact WHERE symbol = $1) AS company_facts,
              (SELECT max(ingested_at)
                 FROM company_fact WHERE symbol = $1) AS company_fact_last_ingested_at,
              (SELECT count(*) FROM news_article
                WHERE symbol = $1
                  AND published_at > now() - interval '30 days') AS recent_news,
              (SELECT max(ingested_at)
                 FROM news_article WHERE symbol = $1) AS news_last_ingested_at,
              (SELECT count(*) FROM estimate_snapshot WHERE symbol = $1) AS estimate_snapshots,
              (SELECT max(snapshot_at)
                 FROM estimate_snapshot WHERE symbol = $1) AS estimate_snapshot_last_at,
              (SELECT count(*) FROM research_evidence
                WHERE symbol = $1
                  AND retrieved_at > now() - interval '30 days') AS research_evidence,
              (SELECT max(retrieved_at)
                 FROM research_evidence WHERE symbol = $1) AS research_evidence_last_retrieved_at,
              (SELECT max(finished_at)
                 FROM research_retrieval_run WHERE symbol = $1) AS research_run_last_at
        """,
        symbol,
    )
    if row is None:
        return {}
    out = {}
    for key in row.keys():
        value = row[key]
        if key.endswith("_at"):
            out[key] = _iso(value)
        else:
            out[key] = int(value or 0)
    return out


async def load_source_health(pool: asyncpg.Pool) -> dict[str, dict]:
    rows = await pool.fetch(
        """SELECT source, last_status, last_started_at, last_success_at,
                  last_failure_at, last_failure_kind, last_error, retry_after_at,
                  rows_seen, rows_inserted, symbols_attempted, symbols_failed
             FROM source_health""",
    )
    out = {}
    for row in rows:
        out[row["source"]] = {
            "source": row["source"],
            "last_status": row["last_status"],
            "last_started_at": _iso(row["last_started_at"]),
            "last_success_at": _iso(row["last_success_at"]),
            "last_failure_at": _iso(row["last_failure_at"]),
            "last_failure_kind": row["last_failure_kind"],
            "last_error": row["last_error"],
            "retry_after_at": _iso(row["retry_after_at"]),
            "rows_seen": int(row["rows_seen"] or 0),
            "rows_inserted": int(row["rows_inserted"] or 0),
            "symbols_attempted": int(row["symbols_attempted"] or 0),
            "symbols_failed": int(row["symbols_failed"] or 0),
        }
    return out


def _acquisition_state(requirement_key: str, source_health: dict[str, dict] | None) -> dict:
    sources = SOURCE_HEALTH_BY_REQUIREMENT.get(requirement_key, [])
    rows = [source_health[s] for s in sources if source_health and s in source_health]
    if not rows:
        return {
            "blocking_state": "missing",
            "state_reason": "source_not_seen",
            "last_error": None,
            "retry_after_at": None,
            "source_health": [],
        }
    if any(r["last_status"] == "running" for r in rows):
        return {
            "blocking_state": "fetching",
            "state_reason": "fetching_required_sources",
            "last_error": None,
            "retry_after_at": None,
            "source_health": rows,
        }
    failures = [r for r in rows if r["last_status"] == "failed" or r.get("last_failure_kind")]
    if failures:
        retry_after = next(
            (r.get("retry_after_at") for r in failures if r.get("retry_after_at")),
            None,
        )
        last_error = next(
            (r.get("last_error") for r in failures if r.get("last_error")),
            None,
        )
        reason = next(
            (r.get("last_failure_kind") for r in failures if r.get("last_failure_kind")),
            None,
        )
        return {
            "blocking_state": "blocked",
            "state_reason": reason or "source_failed",
            "last_error": last_error,
            "retry_after_at": retry_after,
            "source_health": rows,
        }
    if any(r["last_status"] == "ok" and r["rows_inserted"] > 0 for r in rows):
        reason = "no_relevant_symbol_evidence_after_success"
    elif any(r["last_status"] == "no_new_rows" for r in rows):
        reason = "source_checked_no_new_rows"
    else:
        reason = "source_checked_no_relevant_rows"
    return {
        "blocking_state": "missing",
        "state_reason": reason,
        "last_error": None,
        "retry_after_at": None,
        "source_health": rows,
    }


def assess_evidence_requirements(
    evidence_counts: dict[str, object],
    source_health: dict[str, dict] | None = None,
) -> list[dict]:
    missing = []
    checks = {
        "price_history": evidence_counts.get("price_bars", 0) > 0,
        "company_facts": evidence_counts.get("company_facts", 0) > 0,
        "recent_news": evidence_counts.get("recent_news", 0) > 0,
        "analyst_estimates": evidence_counts.get("estimate_snapshots", 0) > 0,
        "product_research": evidence_counts.get("research_evidence", 0) > 0,
    }
    for key, satisfied in checks.items():
        if satisfied:
            continue
        spec = EVIDENCE_REQUIREMENTS[key]
        acquisition = _acquisition_state(key, source_health)
        missing.append(
            {
                "requirement_key": key,
                "source_type": spec["source_type"],
                "priority": spec["priority"],
                "reason": spec["reason"],
                "fetch_actions": spec["fetch_actions"],
                "blocking_state": acquisition["blocking_state"],
                "state_reason": acquisition["state_reason"],
                "last_error": acquisition["last_error"],
                "retry_after_at": acquisition["retry_after_at"],
                "source_ref": {
                    "counts": evidence_counts,
                    "fetch_actions": spec["fetch_actions"],
                    "acquisition_state": acquisition["state_reason"],
                    "source_health": acquisition["source_health"],
                },
            }
        )
    return missing


async def sync_evidence_requirements(
    pool: asyncpg.Pool,
    symbol: str,
    evidence_counts: dict[str, object],
    source_health: dict[str, dict] | None = None,
) -> list[dict]:
    missing = assess_evidence_requirements(evidence_counts, source_health)
    missing_by_key = {r["requirement_key"]: r for r in missing}

    for key, spec in EVIDENCE_REQUIREMENTS.items():
        if key in missing_by_key:
            req = missing_by_key[key]
            source_tasks = build_source_tasks(symbol, req)
            req["source_ref"]["source_tasks"] = [_task_json(task) for task in source_tasks]
            await pool.execute(
                """INSERT INTO evidence_requirement
                     (symbol, requirement_key, source_type, reason, priority,
                      blocking_state, next_retry_at, last_error, source_ref)
                   VALUES (
                     $1, $2, $3, $4, $5, $6,
                     COALESCE($7::timestamptz, now() + interval '30 minutes'),
                     $8,
                     $9::jsonb
                   )
                   ON CONFLICT (symbol, requirement_key) DO UPDATE SET
                     source_type = EXCLUDED.source_type,
                     reason = EXCLUDED.reason,
                     priority = EXCLUDED.priority,
                     blocking_state = EXCLUDED.blocking_state,
                     attempts = CASE
                         WHEN evidence_requirement.next_retry_at IS NOT NULL
                          AND evidence_requirement.next_retry_at <= now()
                         THEN evidence_requirement.attempts + 1
                         ELSE evidence_requirement.attempts
                     END,
                     next_retry_at = CASE
                         WHEN EXCLUDED.blocking_state = 'blocked'
                          AND EXCLUDED.next_retry_at IS NOT NULL
                         THEN EXCLUDED.next_retry_at
                         WHEN evidence_requirement.next_retry_at IS NULL
                           OR evidence_requirement.next_retry_at <= now()
                         THEN EXCLUDED.next_retry_at
                         ELSE evidence_requirement.next_retry_at
                     END,
                     source_ref = EXCLUDED.source_ref,
                     last_error = EXCLUDED.last_error,
                     satisfied_at = NULL,
                     updated_at = now()""",
                symbol,
                key,
                req["source_type"],
                req["reason"],
                req["priority"],
                req["blocking_state"],
                req["retry_after_at"],
                req["last_error"],
                json.dumps(req["source_ref"]),
            )
            await sync_source_tasks(pool, source_tasks)
        else:
            source_tasks = build_satisfied_source_tasks(
                symbol, key, spec, evidence_counts, source_health,
            )
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
                json.dumps({
                    "counts": evidence_counts,
                    "source_tasks": [_task_json(task) for task in source_tasks],
                }),
            )
            await sync_source_tasks(pool, source_tasks)
    return missing


async def sync_source_tasks(pool: asyncpg.Pool, tasks: list[dict]) -> None:
    for task in tasks:
        await pool.execute(
            """INSERT INTO source_task
                 (source_type, requirement_key, action, scope, target_id,
                  provider, limiter_key, state, priority, due_at, attempts,
                  next_retry_at, last_error, source_ref)
               VALUES (
                  $1, $2, $3, $4, $5,
                  $6, $7, $8, $9, $10::timestamptz, $11,
                  $12::timestamptz, $13, $14::jsonb
               )
               ON CONFLICT (scope, target_id, requirement_key, action) DO UPDATE SET
                  source_type = EXCLUDED.source_type,
                  provider = EXCLUDED.provider,
                  limiter_key = EXCLUDED.limiter_key,
                  state = EXCLUDED.state,
                  priority = EXCLUDED.priority,
                  due_at = EXCLUDED.due_at,
                  attempts = GREATEST(source_task.attempts, EXCLUDED.attempts),
                  next_retry_at = EXCLUDED.next_retry_at,
                  last_error = EXCLUDED.last_error,
                  source_ref = EXCLUDED.source_ref,
                  updated_at = now()""",
            task["source_type"],
            task["requirement_key"],
            task["action"],
            task["scope"],
            task["target_id"],
            task["provider"],
            task["limiter_key"],
            task["state"],
            task["priority"],
            task["due_at"],
            task["attempts"],
            task["next_retry_at"],
            task["last_error"],
            json.dumps(task["source_ref"]),
        )


async def refresh_open_evidence_requirements(
    pool: asyncpg.Pool,
    *,
    limit: int = 200,
) -> int:
    """Refresh active ticker evidence rows without invoking LLMs.

    This bootstraps newly introduced requirement keys, keeps open requirements
    current, and requeues satisfied source tasks when their freshness window is
    due.
    """
    rows = await pool.fetch(
        """WITH active_symbols AS (
               SELECT symbol
                 FROM ticker
                WHERE status = 'active'
           ),
           evidence_state AS (
               SELECT a.symbol,
                      count(DISTINCT er.requirement_key) AS requirement_count,
                      COALESCE(
                          bool_or(er.blocking_state <> 'satisfied'),
                          false
                      ) AS has_open_requirement,
                      COALESCE(bool_or(
                          st.due_at <= now()
                          AND st.state IN (
                              'queued', 'no_rows', 'failed',
                              'rate_limited', 'satisfied'
                          )
                      ), false) AS has_due_task
                 FROM active_symbols a
            LEFT JOIN evidence_requirement er ON er.symbol = a.symbol
            LEFT JOIN source_task st
                   ON st.scope = 'symbol'
                  AND st.target_id = a.symbol
             GROUP BY a.symbol
           )
           SELECT symbol
             FROM evidence_state
            WHERE requirement_count < $2
               OR has_open_requirement
               OR has_due_task
         ORDER BY
              (requirement_count < $2) DESC,
              has_open_requirement DESC,
              has_due_task DESC,
              symbol
            LIMIT $1""",
        limit,
        len(EVIDENCE_REQUIREMENTS),
    )
    if not rows:
        return 0

    source_health = await load_source_health(pool)
    for row in rows:
        symbol = row["symbol"]
        evidence_counts = await load_evidence_counts(pool, symbol)
        await sync_evidence_requirements(pool, symbol, evidence_counts, source_health)
    return len(rows)


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
