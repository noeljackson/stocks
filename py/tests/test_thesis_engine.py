import pytest

from stocks.thesis_engine import (
    _evidence_weight,
    _extract_json,
    _normalize_known_unknowns,
    _normalize_system_confidence,
    _system_confidence_components,
    _thesis_review_attention_payload,
    classify_reconciliation,
)


def test_classify_reconciliation_flags_dropped_invalidation_as_weakened() -> None:
    prior = {
        "edge_rationale": "edge",
        "forecast": {"direction": "up"},
        "invalidation_conditions": [{"name": "margin_floor"}],
        "conviction_tier": "medium",
    }
    draft = {
        "edge_rationale": "edge updated",
        "forecast": {"direction": "up"},
        "invalidation_conditions": [],
        "conviction_tier": "high",
    }

    assert classify_reconciliation(prior, draft) == ("weakened_view", True)


def test_classify_reconciliation_detects_material_direction_change() -> None:
    prior = {
        "edge_rationale": "edge",
        "forecast": {"direction": "up"},
        "invalidation_conditions": [],
        "conviction_tier": "medium",
    }
    draft = {
        "edge_rationale": "edge updated",
        "forecast": {"direction": "down"},
        "invalidation_conditions": [],
        "conviction_tier": "medium",
    }

    assert classify_reconciliation(prior, draft) == ("material_change", False)


def test_classify_reconciliation_detects_strengthening() -> None:
    prior = {
        "edge_rationale": "edge",
        "forecast": {"direction": "up"},
        "invalidation_conditions": [{"name": "margin_floor"}],
        "conviction_tier": "low",
    }
    draft = {
        "edge_rationale": "edge with more support",
        "forecast": {"direction": "up"},
        "invalidation_conditions": [{"name": "margin_floor"}],
        "conviction_tier": "high",
    }

    assert classify_reconciliation(prior, draft) == ("strengthened_view", False)


def test_classify_reconciliation_detects_no_change() -> None:
    prior = {
        "edge_rationale": "same edge",
        "forecast": {"direction": "neutral"},
        "invalidation_conditions": [{"name": "demand_break"}],
        "conviction_tier": "low",
    }
    draft = {
        "edge_rationale": "same edge",
        "forecast": {"direction": "neutral"},
        "invalidation_conditions": [{"name": "demand_break"}],
        "conviction_tier": "low",
    }

    assert classify_reconciliation(prior, draft) == ("no_change", False)


def test_classify_reconciliation_detects_confidence_only_strengthening() -> None:
    prior = {
        "edge_rationale": "same edge",
        "forecast": {"direction": "up"},
        "invalidation_conditions": [{"name": "demand_break"}],
        "known_unknowns": [],
        "conviction_tier": "medium",
        "system_confidence": "medium",
    }
    draft = {
        "edge_rationale": "same edge",
        "forecast": {"direction": "up"},
        "invalidation_conditions": [{"name": "demand_break"}],
        "known_unknowns": [],
        "conviction_tier": "high",
        "system_confidence": "very_high",
    }

    assert classify_reconciliation(prior, draft) == ("strengthened_view", False)


def test_classify_reconciliation_versions_known_unknown_changes() -> None:
    prior = {
        "edge_rationale": "same edge",
        "forecast": {"direction": "neutral"},
        "invalidation_conditions": [{"name": "demand_break"}],
        "known_unknowns": [],
        "conviction_tier": "low",
    }
    draft = {
        "edge_rationale": "same edge",
        "forecast": {"direction": "neutral"},
        "invalidation_conditions": [{"name": "demand_break"}],
        "known_unknowns": [{
            "question": "Will demand break?",
            "watch_for": "next earnings call",
            "status": "open",
        }],
        "conviction_tier": "low",
    }

    assert classify_reconciliation(prior, draft) == ("confirmed_existing_view", False)


def test_normalize_system_confidence_accepts_prompt_variants() -> None:
    assert _normalize_system_confidence({"system_confidence": "Very High"}) == "very_high"
    assert _normalize_system_confidence({"forecast": {"confidence": "medium"}}) == "medium"
    assert _normalize_system_confidence({"conviction_tier": "high"}) == "high"
    assert _normalize_system_confidence({"system_confidence": "monitoring"}) == "low"


def test_system_confidence_components_are_stable() -> None:
    components = _system_confidence_components({
        "system_confidence": "high",
        "conviction_tier": "high",
        "edge_present": True,
        "missing_evidence": [{"requirement_key": "news"}],
        "known_unknowns": [{"question": "x", "watch_for": "y"}],
        "system_confidence_components": {"evidence_strength": "multi-source"},
    })

    assert components == {
        "evidence_strength": "multi-source",
        "system_confidence": "high",
        "promotion_tier": "high",
        "edge_present": True,
        "missing_evidence_count": 1,
        "known_unknowns_count": 1,
    }


def test_thesis_review_attention_payload_explains_direction_change() -> None:
    payload = _thesis_review_attention_payload(
        symbol="MU",
        thesis_id="11111111-1111-4111-8111-111111111111",
        version=4,
        classification="material_change",
        draft={"forecast": {"direction": "down"}},
        context={"version": 9, "as_of": "2026-06-02T12:00:00+00:00"},
    )

    assert payload["title"] == "MU thesis needs review: material change"
    assert "direction to down" in payload["reason"]
    assert payload["state_reason"] == "thesis_material_change"
    assert payload["source_ref"] == {
        "event": "thesis_reconciliation",
        "classification": "material_change",
        "operator_action_required": True,
        "thesis_id": "11111111-1111-4111-8111-111111111111",
        "version": 4,
        "context": {
            "version": 9,
            "as_of": "2026-06-02T12:00:00+00:00",
        },
        "draft_direction": "down",
    }


def test_thesis_review_attention_payload_uses_decline_reason() -> None:
    payload = _thesis_review_attention_payload(
        symbol="DELL",
        thesis_id="22222222-2222-4222-8222-222222222222",
        version=2,
        classification="invalidates_existing_view",
        draft={"no_edge_reason": "Fresh evidence no longer supports the edge."},
        context=None,
    )

    assert payload["title"] == "DELL thesis needs review: invalidates existing view"
    assert payload["reason"] == "Fresh evidence no longer supports the edge."
    assert payload["source_ref"]["no_edge_reason"] == "Fresh evidence no longer supports the edge."


def test_normalize_known_unknowns_keeps_explicit_questions() -> None:
    out = _normalize_known_unknowns(
        "MU",
        {
            "known_unknowns": [{
                "question": "Is HBM pricing still tightening?",
                "watch_for": "contract pricing commentary",
                "deadline_at": "2026-08-01T00:00:00Z",
                "evidence_source": "news:contract_pricing",
            }],
        },
    )

    assert out == [{
        "question": "Is HBM pricing still tightening?",
        "watch_for": "contract pricing commentary",
        "status": "open",
        "deadline_at": "2026-08-01T00:00:00Z",
        "evidence_source": "news:contract_pricing",
    }]


def test_normalize_known_unknowns_derives_from_missing_evidence() -> None:
    out = _normalize_known_unknowns(
        "DELL",
        {
            "missing_evidence": [{
                "requirement_key": "product_research",
                "source_type": "web_research",
                "priority": "high",
                "reason": "Need customer win evidence before making AI server demand claims.",
            }],
        },
    )

    assert out[0]["question"] == "What does product research show for DELL?"
    assert out[0]["watch_for"] == (
        "Need customer win evidence before making AI server demand claims."
    )
    assert out[0]["requirement_key"] == "product_research"


def test_normalize_known_unknowns_has_default_for_empty_draft() -> None:
    out = _normalize_known_unknowns("NVDA", {})

    assert out[0]["question"] == "What fresh evidence would materially change the NVDA thesis?"
    assert out[0]["evidence_source"] == "normalized evidence_item stream"


def test_evidence_weight_prefers_strength_and_clamps() -> None:
    assert _evidence_weight({"strength": 0.74, "polarity": -0.2}) == 0.74
    assert _evidence_weight({"strength": 2.0}) == 1.0
    assert _evidence_weight({"strength": -1.0}) == 0.0


def test_evidence_weight_falls_back_to_absolute_polarity() -> None:
    assert _evidence_weight({"polarity": -0.45}) == 0.45
    assert _evidence_weight({"polarity": -2.0}) == 1.0
    assert _evidence_weight({"polarity": 2.0}) == 1.0


def test_evidence_weight_returns_none_without_numeric_signal() -> None:
    assert _evidence_weight({"strength": None, "polarity": None}) is None
    assert _evidence_weight({"strength": "high", "polarity": "positive"}) is None


def test_extract_json_raises_value_error_for_malformed_object() -> None:
    with pytest.raises(ValueError, match="could not parse JSON object"):
        _extract_json('Here is the draft: {"edge_present" true}')
