import datetime as dt
from types import SimpleNamespace

from stocks.ibkr_sync import normalize_fill, normalize_position


def stock_contract(symbol: str = "AVGO") -> SimpleNamespace:
    return SimpleNamespace(
        conId=12345,
        symbol=symbol,
        localSymbol=symbol,
        secType="STK",
        exchange="SMART",
        primaryExchange="NASDAQ",
        currency="USD",
        right="",
        strike=0,
        lastTradeDateOrContractMonth="",
        multiplier="1",
    )


def option_contract() -> SimpleNamespace:
    return SimpleNamespace(
        conId=98765,
        symbol="AVGO",
        localSymbol="AVGO  280115C00300000",
        secType="OPT",
        exchange="SMART",
        primaryExchange="",
        currency="USD",
        right="C",
        strike=300,
        lastTradeDateOrContractMonth="20280115",
        multiplier="100",
    )


def test_normalize_stock_position_maps_long_equity_exposure() -> None:
    now = dt.datetime(2026, 6, 26, tzinfo=dt.UTC)
    raw = SimpleNamespace(account="DU123", contract=stock_contract(), position=10, avgCost=250.0)

    position = normalize_position(raw, now=now)

    assert position is not None
    assert position.symbol == "AVGO"
    assert position.side == "long"
    assert position.instrument == "equity"
    assert position.qty == 10
    assert position.avg_price == 250.0
    assert position.delta_notional == 2_500.0
    assert position.premium_at_risk == 0.0


def test_normalize_short_stock_position_preserves_short_side() -> None:
    now = dt.datetime(2026, 6, 26, tzinfo=dt.UTC)
    raw = SimpleNamespace(
        account="DU123",
        contract=stock_contract("COP"),
        position=-8,
        avgCost=92.5,
    )

    position = normalize_position(raw, now=now)

    assert position is not None
    assert position.side == "short"
    assert position.qty == 8
    assert position.delta_notional == 740.0


def test_normalize_leaps_position_uses_contract_multiplier_for_premium() -> None:
    now = dt.datetime(2026, 6, 26, tzinfo=dt.UTC)
    raw = SimpleNamespace(account="DU123", contract=option_contract(), position=2, avgCost=1_250.0)

    position = normalize_position(raw, now=now)

    assert position is not None
    assert position.side == "call"
    assert position.instrument == "leaps"
    assert position.avg_price == 12.5
    assert position.delta_notional == 0.0
    assert position.premium_at_risk == 2_500.0


def test_normalize_broker_fill_dedup_key_and_fees() -> None:
    now = dt.datetime(2026, 6, 26, tzinfo=dt.UTC)
    fill = SimpleNamespace(
        contract=stock_contract(),
        execution=SimpleNamespace(
            acctNumber="DU123",
            execId="0000e1",
            shares=10,
            price=251.0,
            time="2026-06-26T14:30:00+00:00",
            orderId=100,
            permId=200,
            side="BOT",
            exchange="SMART",
        ),
        commissionReport=SimpleNamespace(commission=1.25, currency="USD", realizedPNL=0.0),
    )

    normalized = normalize_fill(fill, now=now)

    assert normalized is not None
    assert normalized.execution_id == "0000e1"
    assert normalized.qty == 10
    assert normalized.price == 251.0
    assert normalized.fees == 1.25
    assert normalized.filled_at.isoformat() == "2026-06-26T14:30:00+00:00"


def test_normalize_unsupported_contract_returns_none() -> None:
    now = dt.datetime(2026, 6, 26, tzinfo=dt.UTC)
    contract = SimpleNamespace(conId=77, symbol="ES", secType="FUT", multiplier="50")
    raw = SimpleNamespace(account="DU123", contract=contract, position=1, avgCost=6000)

    assert normalize_position(raw, now=now) is None
