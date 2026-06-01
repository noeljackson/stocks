from stocks.thesis_engine import classify_reconciliation


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
