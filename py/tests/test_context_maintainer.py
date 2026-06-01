import datetime as dt
import json

from stocks.context_maintainer import _build_price_snapshot, _build_user_message
from stocks.evidence import (
    assess_evidence_requirements,
    build_satisfied_source_tasks,
    build_source_tasks,
    canonical_requirement_key,
    source_task_due_at,
)


def _bar(day: int, close: float, volume: float = 100.0):
    return {
        "ts": dt.datetime(2026, 1, day, tzinfo=dt.UTC),
        "open": close - 1,
        "high": close + 2,
        "low": close - 2,
        "close": close,
        "volume": volume,
    }


def test_price_snapshot_uses_full_available_history_for_sma_200():
    rows = []
    base = dt.datetime(2025, 1, 1, tzinfo=dt.UTC)
    for i in range(260):
        close = float(i + 1)
        rows.append({
            "ts": base + dt.timedelta(days=i),
            "open": close,
            "high": close,
            "low": close,
            "close": close,
            "volume": 100.0,
        })

    snap = _build_price_snapshot(rows)

    assert snap is not None
    assert snap["close"] == 260.0
    assert snap["sma_20"] == 250.5
    assert snap["sma_50"] == 235.5
    assert snap["sma_100"] == 210.5
    assert snap["sma_200"] == 160.5
    assert snap["pct_vs_sma_200"] == 61.99


def test_price_snapshot_handles_short_history_without_fake_sma():
    snap = _build_price_snapshot([_bar(1, 10.0), _bar(2, 12.0)])

    assert snap is not None
    assert snap["close"] == 12.0
    assert snap["sma_20"] is None
    assert snap["sma_200"] is None
    assert snap["pct_vs_sma_200"] is None


def test_context_user_message_includes_normalized_evidence_items() -> None:
    msg = _build_user_message(
        "MU",
        prior=None,
        events=[],
        facts=[],
        price_snapshot=None,
        news=[],
        estimate_revisions=[],
        analyst_opinion={},
        research_evidence=[],
        evidence_items=[{
            "kind": "news",
            "observed_at": "2026-06-01T12:00:00Z",
            "source": "fmp",
            "summary": "MU HBM customer win",
            "strength": 0.8,
            "polarity": 0.5,
        }],
        evidence_counts={},
        missing_evidence=[],
        today="2026-06-01",
    )

    parsed = json.loads(msg)
    assert parsed["evidence_items"][0]["summary"] == "MU HBM customer win"


def test_assess_evidence_requirements_reports_missing_inputs() -> None:
    missing = assess_evidence_requirements({
        "price_bars": 12,
        "company_facts": 0,
        "recent_news": 0,
        "estimate_snapshots": 4,
        "analyst_price_target_snapshots": 1,
        "research_evidence": 1,
    })

    keys = {r["requirement_key"] for r in missing}
    assert keys == {"company_facts", "recent_news"}


def test_llm_missing_evidence_maps_research_like_items_to_product_research() -> None:
    assert canonical_requirement_key({
        "requirement_key": "customer_adoption_research",
        "source_type": "web_research",
        "reason": "Need public MI325X customer adoption evidence.",
    }) == "product_research"


def test_llm_missing_evidence_maps_fundamental_items_to_company_facts() -> None:
    assert canonical_requirement_key({
        "requirement_key": "q2_10q_snapshot",
        "source_type": "filings",
        "reason": "Need latest 10-Q margin and cash-flow facts.",
    }) == "company_facts"


def test_assess_evidence_requirements_attaches_source_health_state() -> None:
    missing = assess_evidence_requirements(
        {
            "price_bars": 12,
            "company_facts": 0,
            "recent_news": 1,
            "estimate_snapshots": 4,
            "analyst_price_target_snapshots": 1,
            "research_evidence": 1,
        },
        {
            "xbrl": {
                "source": "xbrl",
                "last_status": "failed",
                "last_failure_kind": "rate_limited",
                "last_error": "429",
                "retry_after_at": "2026-06-01T14:00:00Z",
                "rows_seen": 0,
                "rows_inserted": 0,
            },
        },
    )

    [facts] = missing
    assert facts["requirement_key"] == "company_facts"
    assert facts["blocking_state"] == "blocked"
    assert facts["state_reason"] == "rate_limited"
    assert facts["last_error"] == "429"
    assert facts["source_ref"]["source_health"][0]["source"] == "xbrl"


def test_assess_evidence_requirements_marks_running_sources_as_fetching() -> None:
    missing = assess_evidence_requirements(
        {
            "price_bars": 12,
            "company_facts": 0,
            "recent_news": 1,
            "estimate_snapshots": 4,
            "analyst_price_target_snapshots": 1,
            "research_evidence": 1,
        },
        {
            "xbrl": {
                "source": "xbrl",
                "last_status": "running",
                "last_failure_kind": None,
                "last_error": None,
                "retry_after_at": None,
                "rows_seen": 0,
                "rows_inserted": 0,
            },
        },
    )

    [facts] = missing
    assert facts["blocking_state"] == "fetching"
    assert facts["state_reason"] == "fetching_required_sources"


def test_assess_evidence_requirements_marks_checked_sources_as_missing() -> None:
    missing = assess_evidence_requirements(
        {
            "price_bars": 12,
            "company_facts": 0,
            "recent_news": 1,
            "estimate_snapshots": 4,
            "analyst_price_target_snapshots": 1,
            "research_evidence": 1,
        },
        {
            "xbrl": {
                "source": "xbrl",
                "last_status": "no_new_rows",
                "last_failure_kind": None,
                "last_error": None,
                "retry_after_at": None,
                "rows_seen": 1000,
                "rows_inserted": 0,
            },
        },
    )

    [facts] = missing
    assert facts["blocking_state"] == "missing"
    assert facts["state_reason"] == "source_checked_no_new_rows"


def test_assess_evidence_requirements_tracks_product_research() -> None:
    checked_at = dt.datetime(2026, 6, 1, 14, 0, tzinfo=dt.UTC).isoformat()
    missing = assess_evidence_requirements(
        {
            "price_bars": 12,
            "company_facts": 2,
            "recent_news": 1,
            "estimate_snapshots": 4,
            "analyst_price_target_snapshots": 1,
            "research_evidence": 0,
            "research_run_last_at": checked_at,
        },
        {
            "web_research": {
                "source": "web_research",
                "last_status": "no_new_rows",
                "last_failure_kind": None,
                "last_error": None,
                "retry_after_at": None,
                "rows_seen": 0,
                "rows_inserted": 0,
            },
        },
    )

    [research] = missing
    assert research["requirement_key"] == "product_research"
    assert research["source_type"] == "web_research"
    assert research["state_reason"] == "source_checked_no_relevant_rows"


def test_product_research_ignores_global_health_until_symbol_checked() -> None:
    missing = assess_evidence_requirements(
        {
            "price_bars": 12,
            "company_facts": 2,
            "recent_news": 1,
            "estimate_snapshots": 4,
            "analyst_price_target_snapshots": 1,
            "research_evidence": 0,
            "research_run_last_at": None,
        },
        {
            "web_research": {
                "source": "web_research",
                "last_status": "no_new_rows",
                "last_failure_kind": None,
                "last_error": None,
                "retry_after_at": None,
                "rows_seen": 0,
                "rows_inserted": 0,
            },
        },
    )

    [research] = missing
    assert research["requirement_key"] == "product_research"
    assert research["state_reason"] == "source_not_seen_for_symbol"
    tasks = build_source_tasks("NVDA", research)
    assert {task["state"] for task in tasks} == {"queued"}


def test_assess_evidence_requirements_tracks_analyst_opinion() -> None:
    missing = assess_evidence_requirements(
        {
            "price_bars": 12,
            "company_facts": 2,
            "recent_news": 1,
            "estimate_snapshots": 4,
            "analyst_price_target_snapshots": 0,
            "analyst_recommendation_snapshots": 0,
            "research_evidence": 1,
        },
        {
            "fmp_analyst_opinion": {
                "source": "fmp_analyst_opinion",
                "last_status": "no_new_rows",
                "last_failure_kind": None,
                "last_error": None,
                "retry_after_at": None,
                "rows_seen": 0,
                "rows_inserted": 0,
            },
        },
    )

    [opinion] = missing
    assert opinion["requirement_key"] == "analyst_opinion"
    assert opinion["source_type"] == "analyst_opinion"
    assert opinion["fetch_actions"] == [
        "fmp_price_target_consensus",
        "fmp_grades_historical",
        "fmp_price_target_news",
    ]


def test_build_source_tasks_maps_missing_requirement_to_fetch_work() -> None:
    [news] = assess_evidence_requirements(
        {
            "price_bars": 12,
            "company_facts": 2,
            "recent_news": 0,
            "estimate_snapshots": 4,
            "analyst_price_target_snapshots": 1,
            "research_evidence": 1,
        },
        {
            "fmp_news": {
                "source": "fmp_news",
                "last_status": "no_new_rows",
                "last_failure_kind": None,
                "last_error": None,
                "retry_after_at": None,
                "rows_seen": 0,
                "rows_inserted": 0,
            },
        },
    )

    tasks = build_source_tasks("MU", news)

    assert [task["action"] for task in tasks] == [
        "fmp_news",
        "massive_news",
        "llm_sentiment_scoring",
    ]
    assert {task["state"] for task in tasks} == {"no_rows"}
    assert {task["provider"] for task in tasks} == {"fmp", "massive", "llm"}


def test_build_source_tasks_maps_rate_limit_to_provider_pause() -> None:
    [estimates] = assess_evidence_requirements(
        {
            "price_bars": 12,
            "company_facts": 2,
            "recent_news": 1,
            "estimate_snapshots": 0,
            "analyst_price_target_snapshots": 1,
            "research_evidence": 1,
        },
        {
            "fmp_estimates": {
                "source": "fmp_estimates",
                "last_status": "failed",
                "last_failure_kind": "rate_limited",
                "last_error": "429",
                "retry_after_at": "2026-06-01T14:00:00Z",
                "rows_seen": 0,
                "rows_inserted": 0,
            },
        },
    )

    [task] = build_source_tasks("MU", estimates)

    assert task["action"] == "fmp_analyst_estimates"
    assert task["provider"] == "fmp"
    assert task["state"] == "rate_limited"
    assert task["next_retry_at"] == dt.datetime(2026, 6, 1, 14, 0, tzinfo=dt.UTC)


def test_build_source_tasks_applies_provider_wide_pause() -> None:
    retry_after = "2099-06-01T14:00:00Z"
    expected_retry = dt.datetime(2099, 6, 1, 14, 0, tzinfo=dt.UTC)
    source_health = {
        "fmp_estimates": {
            "source": "fmp_estimates",
            "last_status": "failed",
            "last_failure_kind": "rate_limited",
            "last_error": "429",
            "retry_after_at": retry_after,
            "rows_seen": 0,
            "rows_inserted": 0,
        },
    }
    [price] = assess_evidence_requirements(
        {
            "price_bars": 0,
            "company_facts": 2,
            "recent_news": 1,
            "estimate_snapshots": 4,
            "analyst_price_target_snapshots": 1,
            "research_evidence": 1,
        },
        source_health,
    )

    [task] = build_source_tasks("MU", price, source_health)

    assert task["action"] == "fmp_price_backfill"
    assert task["provider"] == "fmp"
    assert task["state"] == "rate_limited"
    assert task["due_at"] == expected_retry
    assert task["next_retry_at"] == expected_retry
    assert task["source_ref"]["provider_pause"]["source"] == "fmp_estimates"


def test_assess_evidence_requirements_empty_when_core_inputs_present() -> None:
    assert assess_evidence_requirements({
        "price_bars": 260,
        "company_facts": 20,
        "recent_news": 5,
        "estimate_snapshots": 10,
        "analyst_price_target_snapshots": 1,
        "research_evidence": 3,
    }) == []


def test_satisfied_source_task_due_at_uses_requirement_freshness_window() -> None:
    last_check = dt.datetime(2026, 6, 1, 14, 0, tzinfo=dt.UTC)

    due_at = source_task_due_at("product_research", last_check_at=last_check)

    assert due_at == dt.datetime(2026, 6, 1, 14, 30, tzinfo=dt.UTC)


def test_build_satisfied_source_tasks_marks_fresh_requirement_satisfied() -> None:
    now = dt.datetime.now(dt.UTC)
    tasks = build_satisfied_source_tasks(
        "MU",
        "product_research",
        {
            "source_type": "web_research",
            "priority": "high",
            "fetch_actions": ["gdelt_doc_search", "bing_news_rss_search"],
        },
        {
            "research_evidence": 2,
            "research_run_last_at": (now - dt.timedelta(minutes=5)).isoformat(),
            "research_evidence_last_retrieved_at": (now - dt.timedelta(minutes=5)).isoformat(),
        },
        {},
    )

    assert {task["state"] for task in tasks} == {"satisfied"}
    assert {task["provider"] for task in tasks} == {"gdelt", "bing"}
    assert all(task["due_at"] > now for task in tasks)


def test_build_satisfied_source_tasks_requeues_stale_requirement() -> None:
    now = dt.datetime.now(dt.UTC)
    tasks = build_satisfied_source_tasks(
        "MU",
        "product_research",
        {
            "source_type": "web_research",
            "priority": "high",
            "fetch_actions": ["gdelt_doc_search"],
        },
        {
            "research_evidence": 2,
            "research_run_last_at": (now - dt.timedelta(hours=2)).isoformat(),
            "research_evidence_last_retrieved_at": (now - dt.timedelta(hours=2)).isoformat(),
        },
        {},
    )

    [task] = tasks
    assert task["state"] == "queued"
    assert task["source_ref"]["acquisition_state"] == "freshness_due"


def test_satisfied_source_tasks_pause_only_limited_provider() -> None:
    now = dt.datetime.now(dt.UTC)
    retry_after = (now + dt.timedelta(hours=1)).isoformat()
    tasks = build_satisfied_source_tasks(
        "MU",
        "recent_news",
        {
            "source_type": "news",
            "priority": "high",
            "fetch_actions": ["fmp_news", "massive_news", "llm_sentiment_scoring"],
        },
        {
            "recent_news": 2,
            "news_last_ingested_at": (now - dt.timedelta(hours=2)).isoformat(),
        },
        {
            "fmp_analyst_opinion": {
                "source": "fmp_analyst_opinion",
                "last_status": "failed",
                "last_failure_kind": "rate_limited",
                "last_error": "429",
                "retry_after_at": retry_after,
                "rows_seen": 0,
                "rows_inserted": 0,
            },
        },
    )

    by_action = {task["action"]: task for task in tasks}
    assert by_action["fmp_news"]["state"] == "rate_limited"
    assert by_action["fmp_news"]["next_retry_at"] == dt.datetime.fromisoformat(retry_after)
    assert by_action["massive_news"]["state"] == "queued"
    assert by_action["llm_sentiment_scoring"]["state"] == "queued"
