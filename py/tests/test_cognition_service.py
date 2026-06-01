import asyncio

import pytest

from stocks.cognition_service import _await_with_ack_progress, _run_symbol_once


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
