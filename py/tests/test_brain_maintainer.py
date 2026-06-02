import datetime as dt

from stocks.brain_maintainer import (
    _brain_llm_due,
    _default_expression_role,
    build_dislocation_map,
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
            "indicators": {
                "market_breadth_internals": {
                    "symbol_count": 500,
                    "pct_above_200d": 0.42,
                },
            },
            "subsector_rs": {},
        },
        now=now,
    )

    assert update["direction"] == "risk_off"
    assert update["missing_evidence"] == ["earnings_breadth"]
    assert update["source_ref"]["maintainer"]["sources"]["fred"]["freshness"] == "fresh"


def test_macro_update_clears_derived_internal_requirements() -> None:
    now = dt.datetime(2026, 6, 1, 15, 0, tzinfo=dt.UTC)
    update = build_macro_update(
        {
            "state": "forming",
            "direction": "neutral",
            "missing_evidence": [
                "earnings_breadth",
                "market_breadth_internals",
                "sector_relative_strength",
                "credit_internals_trend",
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
            "regime": "risk_on",
            "capitulation": False,
            "indicators": {
                "market_breadth_internals": {"symbol_count": 500},
                "earnings_breadth": {"signals": 120, "net_revision_breadth": 0.2},
                "sector_relative_strength": {
                    "sectors": [{"sector": "Technology", "return_20d": 0.08}],
                },
                "credit_internals_trend": {"latest_hy_oas_pct": 2.72, "trend": "stable"},
            },
            "subsector_rs": {},
        },
        now=now,
    )

    assert update["state"] == "active"
    assert update["direction"] == "risk_on"
    assert update["missing_evidence"] == []


def test_dislocation_map_classifies_loved_ignored_and_hated_sectors() -> None:
    now = dt.datetime(2026, 6, 1, 15, 0, tzinfo=dt.UTC)
    got = build_dislocation_map({
        "as_of": now.isoformat(),
        "regime": "neutral",
        "capitulation": False,
        "indicators": {
            "sector_relative_strength": {
                "sectors": [
                    {
                        "sector": "Technology",
                        "symbol_count": 20,
                        "return_20d": 0.12,
                        "return_60d": 0.22,
                        "return_120d": 0.31,
                    },
                    {
                        "sector": "Industrials",
                        "symbol_count": 14,
                        "return_20d": 0.02,
                        "return_60d": 0.01,
                        "return_120d": 0.03,
                    },
                    {
                        "sector": "Financial Services",
                        "symbol_count": 18,
                        "return_20d": -0.02,
                        "return_60d": -0.16,
                        "return_120d": -0.22,
                    },
                ],
            },
            "earnings_breadth": {
                "sectors": {
                    "Technology": {"signals": 20, "up": 14, "down": 3},
                    "Industrials": {"signals": 12, "up": 8, "down": 2},
                    "Financial Services": {"signals": 10, "up": 6, "down": 2},
                },
            },
            "sector_news_sentiment": {
                "sectors": {
                    "Technology": {
                        "articles_14d": 42,
                        "avg_polarity": 0.35,
                        "attention_ratio": 2.1,
                    },
                    "Industrials": {
                        "articles_14d": 3,
                        "avg_polarity": 0.05,
                        "attention_ratio": 0.45,
                    },
                    "Financial Services": {
                        "articles_14d": 9,
                        "avg_polarity": -0.35,
                        "attention_ratio": 0.9,
                    },
                },
            },
        },
        "subsector_rs": {},
    })

    assert got is not None
    sectors = got["sector_classifications"]
    assert sectors["Technology"]["classification"] == "loved_mania"
    assert sectors["Industrials"]["classification"] == "ignored_indifference"
    assert sectors["Financial Services"]["classification"] == "hated_avoided"
    assert "news attention is elevated" in sectors["Technology"]["reasons"]
    assert "news attention is low" in sectors["Industrials"]["reasons"]
    assert "news tone is negative" in sectors["Financial Services"]["reasons"]


def test_macro_update_stores_dislocation_map_in_maintainer_payload() -> None:
    now = dt.datetime(2026, 6, 1, 15, 0, tzinfo=dt.UTC)
    update = build_macro_update(
        {
            "state": "forming",
            "direction": "neutral",
            "missing_evidence": [],
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
            "regime": "neutral",
            "capitulation": False,
            "indicators": {
                "market_breadth_internals": {"symbol_count": 500},
                "sector_relative_strength": {
                    "sectors": [{
                        "sector": "Technology",
                        "return_20d": 0.1,
                        "return_60d": 0.2,
                    }],
                },
                "earnings_breadth": {"signals": 100, "sectors": {}},
                "sector_news_sentiment": {
                    "sectors": {
                        "Technology": {
                            "articles_14d": 30,
                            "avg_polarity": 0.25,
                            "attention_ratio": 2.0,
                        },
                    },
                },
                "credit_internals_trend": {"latest_hy_oas_pct": 2.72},
            },
            "subsector_rs": {},
        },
        now=now,
    )

    dislocation = update["source_ref"]["maintainer"]["dislocation_map"]
    assert dislocation["sector_classifications"]["Technology"]["classification"] == "loved_mania"
    assert any(item["kind"] == "macro_dislocation_map" for item in update["evidence"])


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
