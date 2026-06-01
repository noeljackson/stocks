import datetime as dt

from stocks.brain_maintainer import (
    build_macro_update,
    build_theme_update,
    normalize_symbol,
    symbols_from_json,
)


def test_symbols_from_json_normalizes_and_filters_symbols() -> None:
    assert symbols_from_json([" nvda ", "$amd", "bad symbol", 123, "NVDA", "BRK.B"]) == [
        "NVDA",
        "AMD",
        "BRK.B",
    ]
    assert normalize_symbol("too-long-symbol") is None


def test_theme_update_marks_active_when_linked_coverage_is_complete() -> None:
    update = build_theme_update(
        {
            "state": "forming",
            "direction": "mixed",
            "missing_evidence": [
                "theme_estimate_revision_breadth",
                "customer_adoption_research",
                "relative_strength_by_role",
            ],
            "evidence": [],
            "source_ref": {},
        },
        {
            "linked_count": 3,
            "context_symbols": 3,
            "open_thesis_symbols": 3,
            "price_symbols": 3,
            "news_symbols": 2,
            "estimate_symbols": 2,
            "opinion_symbols": 2,
            "research_symbols": 1,
            "bullish_theses": 2,
            "bearish_theses": 0,
        },
        now=dt.datetime(2026, 6, 1, 15, 0, tzinfo=dt.UTC),
    )

    assert update["state"] == "active"
    assert update["direction"] == "bullish"
    assert update["missing_evidence"] == []
    coverage = update["source_ref"]["maintainer"]["coverage"]
    assert coverage["linked"] == 3
    assert coverage["open_theses"] == 3
    assert update["evidence"][0]["kind"] == "linked_ticker_coverage"


def test_theme_update_keeps_commodity_specific_gaps_visible() -> None:
    update = build_theme_update(
        {
            "state": "forming",
            "direction": "mixed",
            "missing_evidence": [
                "commodity_price_history",
                "inventory_data",
                "producer_estimate_revisions",
            ],
            "evidence": [],
            "source_ref": {},
        },
        {
            "linked_count": 2,
            "context_symbols": 2,
            "open_thesis_symbols": 1,
            "price_symbols": 2,
            "news_symbols": 2,
            "estimate_symbols": 2,
            "opinion_symbols": 1,
            "research_symbols": 1,
        },
        now=dt.datetime(2026, 6, 1, 15, 0, tzinfo=dt.UTC),
    )

    assert update["state"] == "forming"
    assert "commodity_price_history" in update["missing_evidence"]
    assert "inventory_data" in update["missing_evidence"]
    assert "producer_estimate_revisions" not in update["missing_evidence"]


def test_macro_update_derives_direction_from_market_state_and_source_freshness() -> None:
    now = dt.datetime(2026, 6, 1, 15, 0, tzinfo=dt.UTC)
    update = build_macro_update(
        {
            "state": "forming",
            "direction": "neutral",
            "missing_evidence": [
                "fred_macro",
                "market_breadth",
                "credit_spreads",
                "earnings_breadth",
            ],
            "evidence": [],
            "source_ref": {},
        },
        {
            "fred": {
                "source": "fred",
                "last_status": "no_new_rows",
                "last_success_at": now,
                "rows_seen": 4,
                "rows_inserted": 0,
            },
            "cboe": {
                "source": "cboe",
                "last_status": "no_new_rows",
                "last_success_at": now,
                "rows_seen": 2,
                "rows_inserted": 0,
            },
        },
        {
            "as_of": now.isoformat(),
            "regime": "risk_off",
            "capitulation": False,
            "indicators": {},
            "subsector_rs": {},
        },
        now=now,
    )

    assert update["direction"] == "risk_off"
    assert update["missing_evidence"] == ["earnings_breadth"]
    assert update["source_ref"]["maintainer"]["sources"]["fred"]["freshness"] == "fresh"
