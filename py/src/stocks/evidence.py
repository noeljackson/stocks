"""Evidence acquisition state shared by cognition services."""

from __future__ import annotations

import datetime as dt
import json

import asyncpg

FRESHNESS_TARGETS_MINUTES = {
    "price_history": 30,
    "company_profile": 30,
    "filing_metadata": 30,
    "company_facts": 6 * 60,
    "earnings_calendar": 30,
    "recent_news": 30,
    "analyst_estimates": 30,
    "analyst_opinion": 30,
    "product_research": 30,
}

SOURCE_HEALTH_BY_REQUIREMENT = {
    "price_history": ["fmp_price"],
    "company_profile": ["fmp_profile_calendar"],
    "filing_metadata": ["edgar"],
    "company_facts": ["xbrl"],
    "earnings_calendar": ["fmp_profile_calendar"],
    "recent_news": ["fmp_news", "massive_news"],
    "analyst_estimates": ["fmp_estimates"],
    "analyst_opinion": ["fmp_analyst_opinion"],
    "product_research": ["web_research"],
}

SOURCE_TYPE_REQUIREMENT_ALIASES = {
    "price": "price_history",
    "technical": "price_history",
    "market": "price_history",
    "profile": "company_profile",
    "company_profile": "company_profile",
    "company_metadata": "company_profile",
    "metadata": "company_profile",
    "sector": "company_profile",
    "industry": "company_profile",
    "edgar": "filing_metadata",
    "sec_filings": "filing_metadata",
    "filing_metadata": "filing_metadata",
    "fundamentals": "company_facts",
    "filings": "company_facts",
    "xbrl": "company_facts",
    "facts": "company_facts",
    "earnings": "earnings_calendar",
    "earnings_calendar": "earnings_calendar",
    "calendar": "earnings_calendar",
    "catalyst": "earnings_calendar",
    "catalysts": "earnings_calendar",
    "news": "recent_news",
    "narrative": "recent_news",
    "estimates": "analyst_estimates",
    "estimate_revisions": "analyst_estimates",
    "revisions": "analyst_estimates",
    "analyst_opinion": "analyst_opinion",
    "price_targets": "analyst_opinion",
    "ratings": "analyst_opinion",
    "web_research": "product_research",
    "product_research": "product_research",
    "theme_research": "product_research",
    "customer_research": "product_research",
}

EVIDENCE_REQUIREMENTS = {
    "price_history": {
        "source_type": "price",
        "priority": "blocking",
        "reason": "Need daily OHLCV bars before evaluating technical setup or context freshness.",
        "fetch_actions": ["fmp_price_backfill"],
    },
    "company_profile": {
        "source_type": "profile",
        "priority": "medium",
        "reason": (
            "Need company profile metadata for sector, industry, market cap, exchange, "
            "and issuer classification."
        ),
        "fetch_actions": ["fmp_company_profile"],
    },
    "filing_metadata": {
        "source_type": "filings",
        "priority": "medium",
        "reason": (
            "Need recent SEC submission metadata to catch 8-K, 10-Q, and 10-K events "
            "between slower fundamental fact refreshes."
        ),
        "fetch_actions": ["sec_edgar_submissions"],
    },
    "company_facts": {
        "source_type": "fundamentals",
        "priority": "high",
        "reason": "Need SEC/XBRL company facts before making fundamental claims.",
        "fetch_actions": ["sec_company_tickers_cik_lookup", "sec_companyfacts_xbrl"],
    },
    "earnings_calendar": {
        "source_type": "catalysts",
        "priority": "medium",
        "reason": (
            "Need upcoming/recent earnings dates before setting catalyst timing "
            "or deciding whether a claim just reported."
        ),
        "fetch_actions": ["fmp_earnings_calendar"],
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
    "analyst_opinion": {
        "source_type": "analyst_opinion",
        "priority": "medium",
        "reason": (
            "Need analyst price targets and recommendation mix before judging whether "
            "a thesis is outside consensus or already consensus."
        ),
        "fetch_actions": [
            "fmp_price_target_consensus",
            "fmp_grades_historical",
            "fmp_price_target_news",
            "fmp_grades_latest_news",
        ],
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
    next_retry_at = task["next_retry_at"]
    return {
        "action": task["action"],
        "provider": task["provider"],
        "state": task["state"],
        "due_at": _iso(due_at) if hasattr(due_at, "isoformat") else due_at,
        "next_retry_at": (
            _iso(next_retry_at) if hasattr(next_retry_at, "isoformat") else next_retry_at
        ),
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


SOURCE_RUNNING_STALE_AFTER = dt.timedelta(minutes=15)


def _source_running_is_fresh(row: dict, *, now: dt.datetime | None = None) -> bool:
    if row.get("last_status") != "running":
        return False
    started_at = _parse_dt(row.get("last_started_at") or row.get("updated_at"))
    if started_at is None:
        return True
    now = now or dt.datetime.now(dt.UTC)
    return started_at >= now - SOURCE_RUNNING_STALE_AFTER


def canonical_requirement_key(item: dict) -> str | None:
    """Map LLM-declared missing evidence onto the acquisition FSM.

    Prompts should prefer canonical requirement keys, but LLMs may emit
    product/theme-specific names like `customer_adoption_research`. Those still
    need to create retrieval pressure instead of becoming inert prose.
    """
    raw_key = str(item.get("requirement_key") or "").strip().lower()
    if raw_key in EVIDENCE_REQUIREMENTS:
        return raw_key

    source_type = str(item.get("source_type") or "").strip().lower()
    if source_type in SOURCE_TYPE_REQUIREMENT_ALIASES:
        return SOURCE_TYPE_REQUIREMENT_ALIASES[source_type]

    reason = str(item.get("reason") or "").lower()
    haystack = f"{raw_key} {source_type} {reason}"
    if any(token in haystack for token in ("price", "ohlcv", "sma", "rsi", "technical")):
        return "price_history"
    if any(token in haystack for token in ("profile", "market cap", "sector", "industry")):
        return "company_profile"
    if any(
        token in haystack
        for token in ("8-k", "submission", "filing metadata", "recent filing")
    ):
        return "filing_metadata"
    if any(token in haystack for token in ("filing", "xbrl", "fundamental", "10-q", "10-k")):
        return "company_facts"
    if any(token in haystack for token in ("earnings", "calendar", "catalyst date")):
        return "earnings_calendar"
    if any(token in haystack for token in ("news", "article", "headline", "narrative")):
        return "recent_news"
    if any(token in haystack for token in ("estimate", "revision", "consensus")):
        return "analyst_estimates"
    if any(token in haystack for token in ("price target", "rating", "analyst opinion")):
        return "analyst_opinion"
    if any(
        token in haystack
        for token in (
            "research",
            "product",
            "customer",
            "adoption",
            "design win",
            "benchmark",
            "theme",
            "roadmap",
            "commodity",
            "supply",
            "demand",
        )
    ):
        return "product_research"
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
        "company_profile": _parse_dt(evidence_counts.get("company_profile_last_profile_at")),
        "filing_metadata": _parse_dt(evidence_counts.get("filing_event_last_ingested_at")),
        "company_facts": _parse_dt(evidence_counts.get("company_fact_last_ingested_at")),
        "earnings_calendar": _parse_dt(evidence_counts.get("earnings_calendar_last_updated_at")),
        "recent_news": _parse_dt(evidence_counts.get("news_last_ingested_at")),
        "analyst_estimates": _parse_dt(evidence_counts.get("estimate_snapshot_last_at")),
        "analyst_opinion": _latest_dt([
            evidence_counts.get("analyst_price_target_snapshot_last_at"),
            evidence_counts.get("analyst_recommendation_snapshot_last_at"),
            evidence_counts.get("analyst_price_target_event_last_at"),
            evidence_counts.get("analyst_rating_event_last_at"),
        ]),
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


def provider_for_source(source: str) -> str:
    if source.startswith("fmp_"):
        return "fmp"
    if source.startswith("massive_"):
        return "massive"
    if source in {"edgar", "xbrl"}:
        return "sec"
    if source in {"fred", "cboe", "web_research"}:
        return source
    return source


def provider_pause_until(
    provider: str,
    source_health: dict[str, dict] | None,
    *,
    now: dt.datetime | None = None,
) -> tuple[dt.datetime, dict] | None:
    """Return the active provider-wide retry gate, if any.

    Source-specific health rows are produced by many adapters, but a vendor
    429 is provider-wide in practice. If `fmp_estimates` is paused, the planner
    should also hold `fmp_price_backfill`, `fmp_news`, and analyst-opinion tasks
    until the shared retry time.
    """
    if not source_health:
        return None
    now = now or dt.datetime.now(dt.UTC)
    best: tuple[dt.datetime, dict] | None = None
    for row in source_health.values():
        if provider_for_source(row.get("source") or "") != provider:
            continue
        if row.get("last_failure_kind") != "rate_limited":
            continue
        retry_at = _parse_dt(row.get("retry_after_at"))
        if retry_at is None or retry_at <= now:
            continue
        if best is None or retry_at > best[0]:
            best = (retry_at, row)
    if best is None:
        return None
    return best[0], best[1]


def apply_provider_pause(
    task: dict,
    source_health: dict[str, dict] | None,
    *,
    now: dt.datetime | None = None,
) -> dict:
    pause = provider_pause_until(task["provider"], source_health, now=now)
    if pause is None:
        return task
    retry_after_at, source_row = pause
    out = dict(task)
    out["state"] = "rate_limited"
    out["due_at"] = retry_after_at
    out["next_retry_at"] = retry_after_at
    out["last_error"] = out.get("last_error") or source_row.get("last_error")
    source_ref = dict(out.get("source_ref") or {})
    source_ref["provider_pause"] = {
        "provider": task["provider"],
        "source": source_row.get("source"),
        "retry_after_at": _iso(retry_after_at),
        "last_error": source_row.get("last_error"),
    }
    out["source_ref"] = source_ref
    return out


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


def _task_due_at(state: str, retry_after_at: object | None) -> dt.datetime:
    if retry_after_at:
        parsed = _parse_dt(retry_after_at)
        if parsed is not None:
            return parsed
    now = dt.datetime.now(dt.UTC)
    if state in {"no_rows", "failed", "blocked"}:
        return now + dt.timedelta(minutes=30)
    return now


def build_source_tasks(
    symbol: str,
    requirement: dict,
    source_health: dict[str, dict] | None = None,
) -> list[dict]:
    state = source_task_state(
        requirement["blocking_state"],
        requirement.get("state_reason"),
    )
    tasks = []
    retry_after_at = _parse_dt(requirement.get("retry_after_at"))
    for action in requirement.get("fetch_actions", []):
        provider = provider_for_fetch_action(action, requirement["source_type"])
        task = {
            "source_type": requirement["source_type"],
            "requirement_key": requirement["requirement_key"],
            "action": action,
            "scope": "symbol",
            "target_id": symbol,
            "provider": provider,
            "limiter_key": provider,
            "state": state,
            "priority": requirement["priority"],
            "due_at": _task_due_at(state, retry_after_at),
            "attempts": requirement.get("attempts", 0),
            "next_retry_at": retry_after_at,
            "last_error": requirement.get("last_error"),
            "source_ref": {
                "acquisition_state": requirement.get("state_reason"),
                "evidence_counts": requirement.get("source_ref", {}).get("counts", {}),
                "source_health": requirement.get("source_ref", {}).get("source_health", []),
            },
        }
        tasks.append(apply_provider_pause(task, source_health))
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
        task = {
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
        tasks.append(apply_provider_pause(task, source_health))
    return tasks


async def load_evidence_counts(pool: asyncpg.Pool, symbol: str) -> dict[str, object]:
    row = await pool.fetchrow(
        """SELECT
              (SELECT count(*) FROM price_bar WHERE symbol = $1) AS price_bars,
              (SELECT max(ts) FROM price_bar WHERE symbol = $1) AS price_last_bar_at,
              (SELECT count(*) FROM company_profile
                WHERE symbol = $1) AS company_profiles,
              (SELECT max(profile_at)
                 FROM company_profile WHERE symbol = $1) AS company_profile_last_profile_at,
              (SELECT count(*) FROM ingest_event
                WHERE symbol = $1 AND source = 'edgar') AS filing_events,
              (SELECT max(ingested_at)
                 FROM ingest_event
                WHERE symbol = $1
                  AND source = 'edgar') AS filing_event_last_ingested_at,
              (SELECT count(*) FROM company_fact WHERE symbol = $1) AS company_facts,
              (SELECT max(ingested_at)
                 FROM company_fact WHERE symbol = $1) AS company_fact_last_ingested_at,
              (SELECT count(*) FROM earnings_calendar_event
                WHERE symbol = $1
                  AND report_date >= current_date - 30
                  AND report_date <= current_date + 180) AS earnings_calendar_events,
              (SELECT max(updated_at)
                 FROM earnings_calendar_event
                WHERE symbol = $1) AS earnings_calendar_last_updated_at,
              (SELECT count(*) FROM news_article
                WHERE symbol = $1
                  AND published_at > now() - interval '30 days') AS recent_news,
              (SELECT max(ingested_at)
                 FROM news_article WHERE symbol = $1) AS news_last_ingested_at,
              (SELECT count(*) FROM estimate_snapshot WHERE symbol = $1) AS estimate_snapshots,
              (SELECT max(snapshot_at)
                 FROM estimate_snapshot WHERE symbol = $1) AS estimate_snapshot_last_at,
              (SELECT count(*)
                 FROM analyst_price_target_snapshot
                WHERE symbol = $1) AS analyst_price_target_snapshots,
              (SELECT max(snapshot_at)
                 FROM analyst_price_target_snapshot
                WHERE symbol = $1) AS analyst_price_target_snapshot_last_at,
              (SELECT count(*)
                 FROM analyst_recommendation_snapshot
                WHERE symbol = $1) AS analyst_recommendation_snapshots,
              (SELECT max(snapshot_at)
                 FROM analyst_recommendation_snapshot
                WHERE symbol = $1) AS analyst_recommendation_snapshot_last_at,
              (SELECT count(*)
                 FROM analyst_price_target_event
                WHERE symbol = $1
                  AND published_at > now() - interval '90 days') AS analyst_price_target_events,
              (SELECT max(ingested_at)
                 FROM analyst_price_target_event
                WHERE symbol = $1) AS analyst_price_target_event_last_at,
              (SELECT count(*)
                 FROM analyst_rating_event
                WHERE symbol = $1
                  AND published_at > now() - interval '90 days') AS analyst_rating_events,
              (SELECT max(ingested_at)
                 FROM analyst_rating_event
                WHERE symbol = $1) AS analyst_rating_event_last_at,
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


def _acquisition_state(
    requirement_key: str,
    source_health: dict[str, dict] | None,
    evidence_counts: dict[str, object] | None = None,
) -> dict:
    if requirement_key == "product_research":
        last_run_at = _parse_dt((evidence_counts or {}).get("research_run_last_at"))
        if last_run_at is None:
            return {
                "blocking_state": "missing",
                "state_reason": "source_not_seen_for_symbol",
                "last_error": None,
                "retry_after_at": None,
                "source_health": [],
            }
        return {
            "blocking_state": "missing",
            "state_reason": "source_checked_no_relevant_rows",
            "last_error": None,
            "retry_after_at": None,
            "source_health": [],
        }

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
    running_rows = [r for r in rows if r["last_status"] == "running"]
    if any(_source_running_is_fresh(r) for r in running_rows):
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
    if running_rows:
        return {
            "blocking_state": "missing",
            "state_reason": "source_running_stale",
            "last_error": "source still marked running after reclaim window",
            "retry_after_at": None,
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
        "company_profile": evidence_counts.get("company_profiles", 0) > 0,
        "filing_metadata": evidence_counts.get("filing_events", 0) > 0,
        "company_facts": evidence_counts.get("company_facts", 0) > 0,
        "earnings_calendar": evidence_counts.get("earnings_calendar_events", 0) > 0,
        "recent_news": evidence_counts.get("recent_news", 0) > 0,
        "analyst_estimates": evidence_counts.get("estimate_snapshots", 0) > 0,
        "analyst_opinion": (
            evidence_counts.get("analyst_price_target_snapshots", 0) > 0
            or evidence_counts.get("analyst_recommendation_snapshots", 0) > 0
            or evidence_counts.get("analyst_rating_events", 0) > 0
        ),
        "product_research": evidence_counts.get("research_evidence", 0) > 0,
    }
    for key, satisfied in checks.items():
        if satisfied:
            continue
        spec = EVIDENCE_REQUIREMENTS[key]
        acquisition = _acquisition_state(key, source_health, evidence_counts)
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
            await upsert_open_evidence_requirement(pool, symbol, key, req, source_health)
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


async def upsert_open_evidence_requirement(
    pool: asyncpg.Pool,
    symbol: str,
    key: str,
    req: dict,
    source_health: dict[str, dict] | None = None,
) -> None:
    source_tasks = build_source_tasks(symbol, req, source_health)
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
        _parse_dt(req["retry_after_at"]),
        req["last_error"],
        json.dumps(req["source_ref"]),
    )
    await sync_source_tasks(pool, source_tasks)


async def sync_llm_missing_evidence(
    pool: asyncpg.Pool,
    symbol: str,
    missing_evidence: object,
) -> list[dict]:
    """Convert thesis-engine missing_evidence into active acquisition work."""
    if not isinstance(missing_evidence, list):
        return []

    evidence_counts = await load_evidence_counts(pool, symbol)
    source_health = await load_source_health(pool)
    missing = assess_evidence_requirements(evidence_counts, source_health)
    missing_by_key = {r["requirement_key"]: r for r in missing}
    synced: list[dict] = []
    seen: set[str] = set()
    for item in missing_evidence:
        if not isinstance(item, dict):
            continue
        key = canonical_requirement_key(item)
        if key is None or key in seen:
            continue
        seen.add(key)
        req = missing_by_key.get(key)
        if req is None:
            continue
        req = {
            **req,
            "reason": item.get("reason") or req["reason"],
            "source_ref": {
                **req["source_ref"],
                "triggered_by": "thesis_engine.missing_evidence",
                "llm_missing_evidence": item,
            },
        }
        await upsert_open_evidence_requirement(pool, symbol, key, req, source_health)
        synced.append(req)
    return synced


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
                  state = CASE
                      WHEN source_task.state = 'fetching'
                       AND EXCLUDED.state = 'fetching'
                      THEN source_task.state
                      ELSE EXCLUDED.state
                  END,
                  priority = EXCLUDED.priority,
                  due_at = CASE
                      WHEN source_task.state = 'fetching'
                       AND EXCLUDED.state = 'fetching'
                      THEN source_task.due_at
                      ELSE EXCLUDED.due_at
                  END,
                  attempts = GREATEST(source_task.attempts, EXCLUDED.attempts),
                  next_retry_at = CASE
                      WHEN source_task.state = 'fetching'
                       AND EXCLUDED.state = 'fetching'
                      THEN source_task.next_retry_at
                      ELSE EXCLUDED.next_retry_at
                  END,
                  last_error = CASE
                      WHEN source_task.state = 'fetching'
                       AND EXCLUDED.state = 'fetching'
                      THEN source_task.last_error
                      ELSE EXCLUDED.last_error
                  END,
                  source_ref = source_task.source_ref || EXCLUDED.source_ref,
                  updated_at = CASE
                      WHEN source_task.state = 'fetching'
                       AND EXCLUDED.state = 'fetching'
                      THEN source_task.updated_at
                      ELSE now()
                  END""",
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
                          (
                              st.due_at <= now()
                              AND st.state IN (
                                  'queued', 'no_rows', 'failed',
                                  'rate_limited', 'satisfied'
                              )
                          )
                          OR (
                              st.state = 'fetching'
                              AND st.updated_at < now() - interval '15 minutes'
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
