import datetime as dt

from stocks.brain_maintainer import (
    _brain_llm_due,
    _default_expression_role,
    build_macro_update,
    build_theme_update,
    merge_llm_update,
    normalize_symbol,
    symbols_from_json,
)


def test_symbols_from_json_normalizes_and_filters_symbols() -> None:
    symbols = [" nvda ", "$amd", "$2454.tw", "bad symbol", 123, "NVDA", "BRK.B"]
    assert symbols_from_json(symbols) == [
        "NVDA",
        "AMD",
        "2454.TW",
        "BRK.B",
    ]
    assert normalize_symbol("too-long-symbol") is None
    assert normalize_symbol(".TW") is None


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
    assert update["source_ref"]["maintainer"]["deterministic_fingerprint"]
    assert update["evidence"][0]["kind"] == "linked_ticker_coverage"


def test_theme_update_uses_proxy_price_for_commodity_price_history() -> None:
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
            "proxy_count": 1,
            "proxy_price_symbols": 1,
            "news_symbols": 2,
            "estimate_symbols": 2,
            "opinion_symbols": 1,
            "research_symbols": 1,
        },
        now=dt.datetime(2026, 6, 1, 15, 0, tzinfo=dt.UTC),
    )

    assert update["state"] == "forming"
    assert "commodity_price_history" not in update["missing_evidence"]
    assert "inventory_data" in update["missing_evidence"]
    assert "producer_estimate_revisions" not in update["missing_evidence"]
    assert update["source_ref"]["maintainer"]["coverage"]["commodity_proxies"] == 1
    assert update["source_ref"]["maintainer"]["coverage"]["commodity_proxy_price"] == 1


def test_theme_update_requires_proxy_price_when_proxy_exists() -> None:
    update = build_theme_update(
        {
            "state": "forming",
            "direction": "mixed",
            "missing_evidence": ["commodity_price_history"],
            "evidence": [],
            "source_ref": {},
        },
        {
            "linked_count": 2,
            "context_symbols": 2,
            "price_symbols": 2,
            "proxy_count": 1,
            "proxy_price_symbols": 0,
            "news_symbols": 2,
            "estimate_symbols": 2,
            "opinion_symbols": 1,
            "research_symbols": 1,
        },
        now=dt.datetime(2026, 6, 1, 15, 0, tzinfo=dt.UTC),
    )

    assert "commodity_price_history" in update["missing_evidence"]


def test_default_expression_role_marks_commodity_proxies() -> None:
    assert _default_expression_role("copper_industrial_metals", "CPER", "beneficiary") == "proxy"
    assert (
        _default_expression_role("copper_industrial_metals", "FCX", "beneficiary")
        == "beneficiary"
    )
    assert _default_expression_role("wheat_agriculture_food", "WEAT", "beneficiary") == "proxy"


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


def test_brain_llm_due_on_changed_deterministic_fingerprint() -> None:
    now = dt.datetime(2026, 6, 1, 15, 0, tzinfo=dt.UTC)
    assert _brain_llm_due(
        {
            "source_ref": {
                "maintainer": {"deterministic_fingerprint": "old"},
                "llm": {"evaluated_at": now.isoformat()},
            }
        },
        {"fingerprint": "new"},
        now=now,
        max_age_minutes=720,
    )


def test_brain_llm_not_due_when_fingerprint_and_llm_are_fresh() -> None:
    now = dt.datetime(2026, 6, 1, 15, 0, tzinfo=dt.UTC)
    assert not _brain_llm_due(
        {
            "source_ref": {
                "maintainer": {"deterministic_fingerprint": "same"},
                "llm": {"evaluated_at": now.isoformat()},
            }
        },
        {"fingerprint": "same"},
        now=now,
        max_age_minutes=720,
    )


def test_merge_llm_update_rewrites_parent_claim_and_keeps_coverage_evidence() -> None:
    now = dt.datetime(2026, 6, 1, 15, 0, tzinfo=dt.UTC)
    thesis = {
        "state": "forming",
        "direction": "mixed",
        "summary": "Seed summary",
        "core_claim": "Seed claim",
        "why_now": None,
        "evidence": [{"source": "manual", "claim": "keep me"}],
        "open_questions": ["old question"],
        "missing_evidence": ["old_gap"],
        "beneficiaries": ["MU", "NVDA"],
        "losers": [],
        "invalidation_conditions": [],
        "source_ref": {"llm": {"fingerprint": "old"}},
    }
    update = {
        "state": "active",
        "direction": "bullish",
        "missing_evidence": [],
        "evidence": [{
            "generated_by": "brain_maintainer",
            "kind": "linked_ticker_coverage",
            "coverage": {"linked": 2},
        }],
        "source_ref": {"maintainer": {"fingerprint": "det", "deterministic_fingerprint": "det"}},
        "fingerprint": "det",
        "diff": {"coverage": {"linked": 2}},
    }
    merged = merge_llm_update(
        thesis,
        update,
        {
            "state": "active",
            "direction": "bullish",
            "summary": "HBM evidence is improving.",
            "core_claim": "Memory suppliers benefit if revisions keep rising.",
            "why_now": "Recent evidence changed.",
            "evidence": [{
                "claim": "MU revision up",
                "source": "evidence_item",
                "evidence_ids": [7],
            }],
            "missing_evidence": [],
            "open_questions": ["Is pricing still tightening?"],
            "beneficiaries": ["MU"],
            "losers": [],
            "invalidation_conditions": [{
                "name": "pricing_rollover",
                "assertion": "HBM pricing falls",
            }],
            "material_change_reason": "New revision evidence",
        },
        prompt_hash="abc123",
        now=now,
    )

    assert merged["summary"] == "HBM evidence is improving."
    assert merged["core_claim"] == "Memory suppliers benefit if revisions keep rising."
    assert merged["llm_material"] is True
    assert merged["source_ref"]["llm"]["prompt_hash"] == "abc123"
    assert merged["evidence"][0]["claim"] == "keep me"
    assert merged["evidence"][1]["generated_by"] == "brain_llm"
    assert merged["evidence"][-1]["generated_by"] == "brain_maintainer"


def test_merge_llm_update_preserves_deterministic_gaps_and_filters_satisfied_proxy_price() -> None:
    now = dt.datetime(2026, 6, 1, 15, 0, tzinfo=dt.UTC)
    thesis = {
        "state": "forming",
        "direction": "mixed",
        "summary": "Seed summary",
        "core_claim": "Seed claim",
        "why_now": None,
        "evidence": [],
        "open_questions": [],
        "missing_evidence": ["commodity_price_history", "inventory_data"],
        "beneficiaries": ["CPER", "FCX"],
        "losers": [],
        "invalidation_conditions": [],
        "source_ref": {},
    }
    update = {
        "state": "forming",
        "direction": "mixed",
        "missing_evidence": ["inventory_data"],
        "evidence": [{
            "generated_by": "brain_maintainer",
            "kind": "linked_ticker_coverage",
            "coverage": {"commodity_proxies": 1, "commodity_proxy_price": 1},
        }],
        "source_ref": {"maintainer": {"fingerprint": "det", "deterministic_fingerprint": "det"}},
        "fingerprint": "det",
        "diff": {"coverage": {"commodity_proxy_price": 1}},
    }
    merged = merge_llm_update(
        thesis,
        update,
        {
            "state": "forming",
            "direction": "mixed",
            "summary": "Copper proxy price is present.",
            "core_claim": "Copper remains under review.",
            "missing_evidence": [
                "commodity_price_history",
                "inventory_data",
                "china_demand_indicators",
            ],
        },
        prompt_hash="abc123",
        now=now,
    )

    assert "commodity_price_history" not in merged["missing_evidence"]
    assert merged["missing_evidence"] == ["inventory_data", "china_demand_indicators"]
