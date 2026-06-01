import pytest

from stocks.thesis_engine import _evidence_weight, _extract_json, classify_reconciliation


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
