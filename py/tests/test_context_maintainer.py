import datetime as dt

from stocks.context_maintainer import _build_price_snapshot
from stocks.evidence import assess_evidence_requirements


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


def test_assess_evidence_requirements_reports_missing_inputs() -> None:
    missing = assess_evidence_requirements({
        "price_bars": 12,
        "company_facts": 0,
        "recent_news": 0,
        "estimate_snapshots": 4,
    })

    keys = {r["requirement_key"] for r in missing}
    assert keys == {"company_facts", "recent_news"}


def test_assess_evidence_requirements_attaches_source_health_state() -> None:
    missing = assess_evidence_requirements(
        {
            "price_bars": 12,
            "company_facts": 0,
            "recent_news": 1,
            "estimate_snapshots": 4,
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


def test_assess_evidence_requirements_empty_when_core_inputs_present() -> None:
    assert assess_evidence_requirements({
        "price_bars": 260,
        "company_facts": 20,
        "recent_news": 5,
        "estimate_snapshots": 10,
    }) == []
