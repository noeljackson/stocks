import asyncio

import pytest

from stocks.cognition_service import (
    _await_with_ack_progress,
    _decline_attention_assignment,
    _effective_sweep_interval_seconds,
    _effective_sweep_limit,
    _next_retry_from_evidence,
    _run_symbol_once,
    _status_for_thesis_result,
    _sweep_trigger,
)


class FakeMsg:
    def __init__(self) -> None:
        self.progress_calls = 0

    async def in_progress(self) -> None:
        self.progress_calls += 1


@pytest.mark.asyncio
async def test_ack_progress_heartbeat_runs_while_pipeline_is_busy() -> None:
    msg = FakeMsg()

    async def slow_pipeline() -> str:
        await asyncio.sleep(0.035)
        return "done"

    result = await _await_with_ack_progress(
        msg,
        slow_pipeline(),
        progress_interval_seconds=0.01,
    )

    assert result == "done"
    assert msg.progress_calls >= 2


@pytest.mark.asyncio
async def test_run_symbol_once_skips_duplicate_inflight_symbol() -> None:
    in_flight = {"LITE"}
    calls = 0

    async def runner() -> None:
        nonlocal calls
        calls += 1

    ran = await _run_symbol_once("lite", runner, in_flight)

    assert not ran
    assert calls == 0
    assert in_flight == {"LITE"}


@pytest.mark.asyncio
async def test_run_symbol_once_releases_symbol_after_pipeline() -> None:
    in_flight: set[str] = set()

    async def runner() -> None:
        assert "CRWV" in in_flight

    ran = await _run_symbol_once("crwv", runner, in_flight)

    assert ran
    assert in_flight == set()


def test_sweep_trigger_marks_open_thesis_update_when_evidence_exists() -> None:
    assert _sweep_trigger(4, "thesis-id") == "open_thesis_update_loop"


def test_sweep_trigger_keeps_open_thesis_update_auditable_when_evidence_bootstraps() -> None:
    assert _sweep_trigger(0, "thesis-id") == "open_thesis_update_loop"


def test_sweep_trigger_bootstraps_missing_evidence_without_open_thesis() -> None:
    assert _sweep_trigger(0, None) == "evidence_state_bootstrap"


def test_sweep_trigger_falls_back_to_maintenance_without_open_thesis() -> None:
    assert _sweep_trigger(4, None) == "maintenance_sweep"


def test_decline_assignment_waits_on_llm_missing_evidence() -> None:
    assert _decline_attention_assignment([], [{
        "requirement_key": "product_research",
        "reason": "Need product adoption research.",
    }]) == ("waiting_on_data", "source", "missing_evidence")


def test_decline_assignment_routes_true_no_edge_to_operator_review() -> None:
    assert _decline_attention_assignment([]) == (
        "ready_for_review",
        "operator",
        "thesis_declined",
    )


def test_effective_sweep_interval_caps_stale_config(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("COGNITION_SWEEP_SECONDS", "900")
    monkeypatch.setenv("COGNITION_OPEN_THESIS_MAX_AGE_MINUTES", "30")

    assert _effective_sweep_interval_seconds() == 300


def test_effective_sweep_interval_preserves_disable(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("COGNITION_SWEEP_SECONDS", "0")

    assert _effective_sweep_interval_seconds() == 0


def test_effective_sweep_limit_floors_stale_config(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("COGNITION_MAX_SYMBOLS_PER_SWEEP", "5")
    monkeypatch.setenv("COGNITION_MIN_SYMBOLS_PER_SWEEP", "20")

    assert _effective_sweep_limit() == 20


def test_status_for_thesis_result_classifies_new_draft() -> None:
    assert _status_for_thesis_result({"_thesis_id": "abc"}) == ("drafted", None)


def test_status_for_thesis_result_classifies_no_change_reconciliation() -> None:
    assert _status_for_thesis_result({
        "_thesis_id": "abc",
        "_reconciled_existing_thesis": True,
        "_reconciliation_classification": "no_change",
    }) == ("no_change", "no_change")


def test_status_for_thesis_result_classifies_material_reconciliation() -> None:
    assert _status_for_thesis_result({
        "_thesis_id": "abc",
        "_reconciled_existing_thesis": True,
        "_reconciliation_classification": "weakened_view",
    }) == ("reconciled", "weakened_view")


def test_status_for_thesis_result_classifies_decline() -> None:
    assert _status_for_thesis_result({"edge_present": False}) == ("declined", None)


def test_next_retry_from_evidence_uses_earliest_retry() -> None:
    assert _next_retry_from_evidence([
        {"next_retry_at": "2026-06-02T12:30:00+00:00"},
        {"next_retry_at": "2026-06-02T12:00:00+00:00"},
        {"next_retry_at": None},
    ]).isoformat() == "2026-06-02T12:00:00+00:00"
