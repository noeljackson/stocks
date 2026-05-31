from stocks.thesis_engine import (
    _context_has_substance,
    _draft_kind,
    _normalize_monitoring_draft,
)


def test_blank_context_declines_no_edge_result():
    parsed = {"edge_present": False, "no_edge_reason": "context is a blank slate"}
    assert not _context_has_substance({"structural": {}, "narrative": {}, "market": {}})
    assert _draft_kind(parsed, {"structural": {}, "narrative": {}, "market": {}}) == "decline"


def test_substantial_no_edge_result_becomes_monitoring():
    context = {
        "structural": {"summary": "large profitable AI accelerator supplier"},
        "narrative": {},
        "market": {},
    }
    parsed = {
        "edge_present": False,
        "no_edge_reason": "facts are already consensus",
    }
    assert _context_has_substance(context)
    assert _draft_kind(parsed, context) == "monitoring"
    normalized = _normalize_monitoring_draft("NVDA", parsed)
    assert normalized["thesis_kind"] == "monitoring"
    assert normalized["forecast"]["direction"] == "neutral"
    assert "Monitoring thesis for NVDA" in normalized["edge_rationale"]


def test_explicit_actionable_kind_wins():
    parsed = {"thesis_kind": "actionable_edge", "edge_present": True}
    assert _draft_kind(parsed, None) == "actionable_edge"
