import asyncio

import pytest

from stocks.cognition_service import (
    _await_with_ack_progress,
    _effective_sweep_interval_seconds,
    _effective_sweep_limit,
    _run_symbol_once,
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
