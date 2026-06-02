import asyncio

import pytest

import stocks.cognition_service as cognition_service
from stocks.cognition_service import (
    _await_with_ack_progress,
    _decline_attention_assignment,
    _effective_sweep_interval_seconds,
    _effective_sweep_limit,
    _next_retry_from_evidence,
    _reclaim_running_cognition_runs,
    _reclaim_stale_cognition_runs,
    _run_sweep_targets,
    _run_symbol_once,
    _select_sweep_targets,
    _status_for_thesis_result,
    _sweep_concurrency,
    _sweep_trigger,
)


class FakePool:
    def __init__(self, open_theses: int = 0) -> None:
        self.open_theses = open_theses
        self.finish_args = None

    async def fetchval(self, sql: str, *_args):
        if "INSERT INTO cognition_run" in sql:
            return 42
        if "count(*)" in sql:
            return self.open_theses
        raise AssertionError(f"unexpected fetchval: {sql}")

    async def execute(self, sql: str, *args) -> None:
        if "UPDATE cognition_run" in sql:
            self.finish_args = args


class FakeMsg:
    def __init__(self) -> None:
        self.progress_calls = 0

    async def in_progress(self) -> None:
        self.progress_calls += 1


class ReclaimPool:
    def __init__(self, count: int = 0) -> None:
        self.count = count
        self.calls: list[tuple[str, tuple]] = []

    async def fetchval(self, sql: str, *args):
        self.calls.append((sql, args))
        return self.count


@pytest.mark.asyncio
async def test_startup_reclaim_marks_all_running_cognition_runs() -> None:
    pool = ReclaimPool(count=2)

    reclaimed = await _reclaim_running_cognition_runs(pool)

    assert reclaimed == 2
    sql, args = pool.calls[0]
    assert "WHERE status = 'running'" in sql
    assert "started_at <" not in sql
    assert args[0] == "orphaned_by_cognition_startup"


@pytest.mark.asyncio
async def test_stale_reclaim_uses_age_threshold() -> None:
    pool = ReclaimPool(count=1)

    reclaimed = await _reclaim_stale_cognition_runs(pool, max_age_minutes=45)

    assert reclaimed == 1
    sql, args = pool.calls[0]
    assert "WHERE status = 'running'" in sql
    assert "started_at < now()" in sql
    assert args[0] == 45
    assert args[1] == "stale_running_reclaim"


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


@pytest.mark.asyncio
async def test_pipeline_reconciles_existing_open_thesis(monkeypatch: pytest.MonkeyPatch) -> None:
    pool = FakePool(open_theses=1)
    draft_calls: list[str] = []

    async def refresh_context(symbol: str) -> int:
        assert symbol == "MU"
        return 7

    async def load_open_evidence_requirements(pool_arg, symbol: str) -> list[dict]:
        assert pool_arg is pool
        assert symbol == "MU"
        return []

    async def draft_thesis(symbol: str) -> dict:
        draft_calls.append(symbol)
        return {
            "_thesis_id": "11111111-1111-4111-8111-111111111111",
            "_reconciled_existing_thesis": True,
            "_reconciliation_classification": "no_change",
        }

    async def noop(thesis_id: str) -> None:
        assert thesis_id == "11111111-1111-4111-8111-111111111111"

    monkeypatch.setattr(cognition_service, "refresh_context", refresh_context)
    monkeypatch.setattr(
        cognition_service,
        "load_open_evidence_requirements",
        load_open_evidence_requirements,
    )
    monkeypatch.setattr(cognition_service, "draft_thesis", draft_thesis)
    monkeypatch.setattr(cognition_service, "sharpen_thesis", noop)
    monkeypatch.setattr(cognition_service, "challenge_thesis", noop)

    await cognition_service._run_pipeline(
        pool,
        "mu",
        source_ref={"trigger": "open_thesis_update_loop"},
    )

    assert draft_calls == ["MU"]
    assert pool.finish_args is not None
    assert pool.finish_args[1] == "no_change"
    assert pool.finish_args[3] == 7
    assert pool.finish_args[5] == "no_change"


def test_sweep_trigger_marks_open_thesis_update_when_evidence_exists() -> None:
    assert _sweep_trigger(4, "thesis-id") == "open_thesis_update_loop"


def test_sweep_trigger_marks_source_task_delta_for_open_thesis() -> None:
    assert _sweep_trigger(4, "thesis-id", "source_task_changed") == "source_task_delta"


def test_sweep_trigger_marks_source_task_delta_for_no_thesis_retry() -> None:
    assert _sweep_trigger(4, None, "source_task_changed_retry") == "source_task_delta"


def test_sweep_trigger_marks_evidence_delta_for_open_thesis() -> None:
    assert _sweep_trigger(4, "thesis-id", "evidence_item_changed") == "evidence_delta"


def test_sweep_trigger_marks_evidence_delta_for_no_thesis_retry() -> None:
    assert _sweep_trigger(4, None, "evidence_item_changed_retry") == "evidence_delta"


def test_sweep_trigger_keeps_open_thesis_update_auditable_when_evidence_bootstraps() -> None:
    assert _sweep_trigger(0, "thesis-id") == "open_thesis_update_loop"


def test_sweep_trigger_bootstraps_missing_evidence_without_open_thesis() -> None:
    assert _sweep_trigger(0, None) == "evidence_state_bootstrap"


def test_sweep_trigger_falls_back_to_maintenance_without_open_thesis() -> None:
    assert _sweep_trigger(4, None) == "maintenance_sweep"


def test_select_sweep_targets_reserves_bootstrap_capacity() -> None:
    rows = [
        {"symbol": "MU", "sweep_reason": "open_thesis_due"},
        {"symbol": "NVDA", "sweep_reason": "open_thesis_due"},
        {"symbol": "CRDO", "sweep_reason": "open_thesis_due"},
        {"symbol": "2454.TW", "sweep_reason": "evidence_state_missing"},
        {"symbol": "LITE", "sweep_reason": "context_missing"},
    ]

    selected = _select_sweep_targets(rows, limit=3, bootstrap_floor=2)

    assert [row["symbol"] for row in selected] == ["2454.TW", "LITE", "MU"]


def test_select_sweep_targets_preserves_order_when_no_bootstrap_work() -> None:
    rows = [
        {"symbol": "MU", "sweep_reason": "open_thesis_due"},
        {"symbol": "NVDA", "sweep_reason": "open_thesis_due"},
        {"symbol": "CRDO", "sweep_reason": "maintenance_sweep"},
    ]

    selected = _select_sweep_targets(rows, limit=2, bootstrap_floor=1)

    assert [row["symbol"] for row in selected] == ["MU", "NVDA"]


def _sweep_row(symbol: str) -> dict:
    return {
        "symbol": symbol,
        "open_theses": 1,
        "evidence_rows": 4,
        "thesis_at": None,
        "thesis_evaluated_at": None,
        "source_task_at": None,
        "evidence_item_at": None,
        "thesis_id": f"thesis-{symbol}",
        "sweep_reason": "open_thesis_due",
        "context_version": 7,
    }


@pytest.mark.asyncio
async def test_run_sweep_targets_uses_bounded_concurrency(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    pool = FakePool(open_theses=1)
    active = 0
    max_active = 0
    calls: list[tuple[str, dict]] = []

    async def run_pipeline(pool_arg, symbol: str, *, source_ref: dict, candidate_id=None) -> None:
        nonlocal active, max_active
        assert pool_arg is pool
        assert candidate_id is None
        active += 1
        max_active = max(max_active, active)
        try:
            await asyncio.sleep(0.02)
            calls.append((symbol, source_ref))
        finally:
            active -= 1

    monkeypatch.setenv("COGNITION_SWEEP_CONCURRENCY", "2")
    monkeypatch.setattr(cognition_service, "_run_pipeline", run_pipeline)

    await _run_sweep_targets(
        pool,
        [_sweep_row("MU"), _sweep_row("NVDA"), _sweep_row("AMD")],
    )

    assert max_active == 2
    assert [symbol for symbol, _ in calls] == ["MU", "NVDA", "AMD"]
    assert {ref["trigger"] for _, ref in calls} == {"open_thesis_update_loop"}
    assert {ref["sweep_concurrency"] for _, ref in calls} == {2}


@pytest.mark.asyncio
async def test_run_sweep_target_marks_evidence_delta_source_ref(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    pool = FakePool(open_theses=1)
    calls: list[tuple[str, dict]] = []
    row = _sweep_row("MU")
    row["sweep_reason"] = "evidence_item_changed"
    row["evidence_item_at"] = "2026-06-02T12:00:00+00:00"

    async def run_pipeline(pool_arg, symbol: str, *, source_ref: dict, candidate_id=None) -> None:
        assert pool_arg is pool
        assert candidate_id is None
        calls.append((symbol, source_ref))

    monkeypatch.setattr(cognition_service, "_run_pipeline", run_pipeline)

    await cognition_service._run_sweep_target(pool, row)

    assert calls == [("MU", calls[0][1])]
    ref = calls[0][1]
    assert ref["trigger"] == "evidence_delta"
    assert ref["sweep_reason"] == "evidence_item_changed"
    assert ref["evidence_item_at"] == "2026-06-02T12:00:00+00:00"


def test_sweep_concurrency_defaults_to_two(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.delenv("COGNITION_SWEEP_CONCURRENCY", raising=False)

    assert _sweep_concurrency() == 2


@pytest.mark.asyncio
async def test_sweep_once_runs_ticker_targets_before_parent_brain_maintenance(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    pool = FakePool(open_theses=1)
    order: list[str] = []

    async def reclaim(_pool, *, max_age_minutes: int) -> int:
        assert max_age_minutes > 0
        order.append("reclaim")
        return 0

    async def refresh_evidence(_pool, *, limit: int) -> int:
        assert limit > 0
        order.append("evidence")
        return 0

    async def sweep_targets(_pool, **kwargs) -> list[dict]:
        assert kwargs["limit"] > 0
        order.append("select")
        return [_sweep_row("MU")]

    async def run_targets(_pool, targets: list) -> None:
        assert [row["symbol"] for row in targets] == ["MU"]
        order.append("tickers")

    async def refresh_brain(_pool, *, limit: int) -> int:
        assert limit > 0
        order.append("brain")
        return 1

    monkeypatch.setattr(cognition_service, "_reclaim_stale_cognition_runs", reclaim)
    monkeypatch.setattr(cognition_service, "refresh_open_evidence_requirements", refresh_evidence)
    monkeypatch.setattr(cognition_service, "_sweep_targets", sweep_targets)
    monkeypatch.setattr(cognition_service, "_run_sweep_targets", run_targets)
    monkeypatch.setattr(cognition_service, "refresh_brain_theses", refresh_brain)

    await cognition_service._sweep_once(pool)

    assert order == ["reclaim", "evidence", "select", "tickers", "brain"]


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


def test_effective_sweep_limit_uses_configured_value_above_floor(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("COGNITION_MAX_SYMBOLS_PER_SWEEP", "25")
    monkeypatch.setenv("COGNITION_MIN_SYMBOLS_PER_SWEEP", "20")

    assert _effective_sweep_limit() == 25


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
