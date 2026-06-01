import datetime as dt

from stocks.context_maintainer import _build_price_snapshot


def _bar(day: int, close: float, volume: float = 100.0):
    return {
        "ts": dt.datetime(2026, 1, day, tzinfo=dt.UTC),
        "open": close - 1,
        "high": close + 2,
        "low": close - 2,
        "close": close,
        "volume": volume,
    }


def test_price_snapshot_uses_full_available_history_for_sma_200():
    rows = []
    base = dt.datetime(2025, 1, 1, tzinfo=dt.UTC)
    for i in range(260):
        close = float(i + 1)
        rows.append({
            "ts": base + dt.timedelta(days=i),
            "open": close,
            "high": close,
            "low": close,
            "close": close,
            "volume": 100.0,
        })

    snap = _build_price_snapshot(rows)

    assert snap is not None
    assert snap["close"] == 260.0
    assert snap["sma_20"] == 250.5
    assert snap["sma_50"] == 235.5
    assert snap["sma_100"] == 210.5
    assert snap["sma_200"] == 160.5
    assert snap["pct_vs_sma_200"] == 61.99


def test_price_snapshot_handles_short_history_without_fake_sma():
    snap = _build_price_snapshot([_bar(1, 10.0), _bar(2, 12.0)])

    assert snap is not None
    assert snap["close"] == 12.0
    assert snap["sma_20"] is None
    assert snap["sma_200"] is None
    assert snap["pct_vs_sma_200"] is None
