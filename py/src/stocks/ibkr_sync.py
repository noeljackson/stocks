"""Read-only IBKR position/fill sync (#25).

Connects to Trader Workstation or IB Gateway via ib_insync, normalizes current
positions and broker executions, writes broker-owned rows into `position` /
`position_fill`, and publishes `position.updated`.

Usage:
    python -m stocks.ibkr_sync --once
    python -m stocks.ibkr_sync --loop
"""

from __future__ import annotations

import argparse
import asyncio
import datetime as dt
import json
import logging
import os
from dataclasses import dataclass, field
from typing import Any

import asyncpg
import nats
from nats.js.errors import NotFoundError

from . import config

log = logging.getLogger("ibkr_sync")

BROKER = "ibkr"
POSITION_UPDATED = "position.updated"
MARKET_STREAM = "MARKET"
MARKET_STREAM_SUBJECTS = ["regime.*", "discovery.*", "market.>", "position.*"]


class IbkrConnectionError(RuntimeError):
    pass


@dataclass(frozen=True)
class IbkrConfig:
    host: str
    port: int
    client_id: int
    account: str | None
    readonly: bool
    timeout_seconds: float
    sync_interval_seconds: int


@dataclass(frozen=True)
class BrokerPosition:
    account: str
    symbol: str
    con_id: int
    side: str
    instrument: str
    qty: float
    avg_price: float
    delta_notional: float
    premium_at_risk: float
    as_of: dt.datetime
    contract: dict[str, Any]
    raw: dict[str, Any]

    @property
    def key(self) -> tuple[str, int, str, str]:
        return (self.account, self.con_id, self.side, self.instrument)


@dataclass(frozen=True)
class BrokerFill:
    account: str
    symbol: str
    con_id: int
    execution_id: str
    side: str
    instrument: str
    qty: float
    price: float
    fees: float
    filled_at: dt.datetime
    contract: dict[str, Any]
    raw: dict[str, Any]


@dataclass(frozen=True)
class BrokerSnapshot:
    positions: list[BrokerPosition]
    fills: list[BrokerFill]
    net_liquidation: float | None = None
    as_of: dt.datetime = field(default_factory=lambda: dt.datetime.now(dt.UTC))


@dataclass(frozen=True)
class BrokerSyncResult:
    positions_inserted: int = 0
    positions_updated: int = 0
    positions_closed: int = 0
    fills_inserted: int = 0
    fills_skipped: int = 0
    portfolio_updated: bool = False
    updates: tuple[dict[str, Any], ...] = ()


def _env_bool(name: str, default: bool) -> bool:
    raw = os.getenv(name)
    if raw is None or raw == "":
        return default
    return raw.strip().lower() in {"1", "true", "yes", "on"}


def _env_int(name: str, default: int) -> int:
    raw = os.getenv(name)
    if raw is None or raw == "":
        return default
    try:
        return int(raw)
    except ValueError:
        log.warning("invalid %s=%r; using %d", name, raw, default)
        return default


def _env_float(name: str, default: float) -> float:
    raw = os.getenv(name)
    if raw is None or raw == "":
        return default
    try:
        return float(raw)
    except ValueError:
        log.warning("invalid %s=%r; using %.1f", name, raw, default)
        return default


def load_ibkr_config() -> IbkrConfig:
    account = os.getenv("IBKR_ACCOUNT", "").strip() or None
    return IbkrConfig(
        host=os.getenv("IBKR_HOST", "127.0.0.1"),
        port=_env_int("IBKR_PORT", 7497),
        client_id=_env_int("IBKR_CLIENT_ID", 81),
        account=account,
        readonly=_env_bool("IBKR_READONLY", True),
        timeout_seconds=_env_float("IBKR_TIMEOUT_SECONDS", 6.0),
        sync_interval_seconds=max(5, _env_int("IBKR_SYNC_INTERVAL_SECONDS", 30)),
    )


def _attr(obj: Any, name: str, default: Any = None) -> Any:
    return getattr(obj, name, default)


def _float(value: Any, default: float = 0.0) -> float:
    try:
        parsed = float(value)
    except (TypeError, ValueError):
        return default
    if parsed != parsed:
        return default
    return parsed


def _contract_dict(contract: Any) -> dict[str, Any]:
    return {
        "con_id": _attr(contract, "conId"),
        "symbol": _attr(contract, "symbol"),
        "local_symbol": _attr(contract, "localSymbol"),
        "sec_type": _attr(contract, "secType"),
        "exchange": _attr(contract, "exchange"),
        "primary_exchange": _attr(contract, "primaryExchange"),
        "currency": _attr(contract, "currency"),
        "right": _attr(contract, "right"),
        "strike": _attr(contract, "strike"),
        "last_trade_date_or_contract_month": _attr(contract, "lastTradeDateOrContractMonth"),
        "multiplier": _attr(contract, "multiplier"),
    }


def _symbol(contract: Any) -> str | None:
    symbol = str(_attr(contract, "symbol", "") or "").strip().upper()
    if symbol:
        return symbol
    local = str(_attr(contract, "localSymbol", "") or "").strip().upper()
    return local.split()[0] if local else None


def _con_id(contract: Any) -> int | None:
    con_id = _attr(contract, "conId")
    try:
        parsed = int(con_id)
    except (TypeError, ValueError):
        return None
    return parsed if parsed > 0 else None


def _instrument(contract: Any, now: dt.datetime) -> str | None:
    sec_type = str(_attr(contract, "secType", "") or "").upper()
    if sec_type == "STK":
        return "equity"
    if sec_type == "OPT":
        expiry = _parse_contract_expiry(_attr(contract, "lastTradeDateOrContractMonth"))
        if expiry and expiry - now.date() >= dt.timedelta(days=365):
            return "leaps"
        return "options"
    return None


def _side(contract: Any, signed_qty: float) -> str:
    sec_type = str(_attr(contract, "secType", "") or "").upper()
    if sec_type == "OPT":
        right = str(_attr(contract, "right", "") or "").upper()
        if right.startswith("P"):
            return "put"
        if right.startswith("C"):
            return "call"
        return "hedge"
    return "short" if signed_qty < 0 else "long"


def _multiplier(contract: Any, instrument: str) -> float:
    default = 100.0 if instrument in {"options", "leaps"} else 1.0
    multiplier = _float(_attr(contract, "multiplier"), default)
    return multiplier if multiplier > 0 else default


def _parse_contract_expiry(value: Any) -> dt.date | None:
    raw = str(value or "").strip()
    if len(raw) >= 8 and raw[:8].isdigit():
        try:
            return dt.datetime.strptime(raw[:8], "%Y%m%d").date()
        except ValueError:
            return None
    if len(raw) >= 6 and raw[:6].isdigit():
        try:
            return dt.datetime.strptime(raw[:6], "%Y%m").date()
        except ValueError:
            return None
    return None


def _unit_price(raw_price: float, multiplier: float, instrument: str) -> float:
    if instrument in {"options", "leaps"} and multiplier > 1.0 and raw_price > multiplier:
        return raw_price / multiplier
    return raw_price


def _exposure(
    side: str,
    instrument: str,
    qty: float,
    price: float,
    multiplier: float,
) -> tuple[float, float]:
    gross = abs(qty) * abs(price) * multiplier
    if instrument in {"options", "leaps"} or side in {"call", "put", "hedge"}:
        return 0.0, gross
    return gross, 0.0


def normalize_position(position: Any, *, now: dt.datetime | None = None) -> BrokerPosition | None:
    now = now or dt.datetime.now(dt.UTC)
    contract = _attr(position, "contract")
    account = str(_attr(position, "account", "") or "").strip()
    symbol = _symbol(contract)
    con_id = _con_id(contract)
    signed_qty = _float(_attr(position, "position"))
    if not account or not symbol or con_id is None or abs(signed_qty) <= 0:
        return None
    instrument = _instrument(contract, now)
    if instrument is None:
        return None
    side = _side(contract, signed_qty)
    qty = abs(signed_qty)
    multiplier = _multiplier(contract, instrument)
    avg_price = _unit_price(abs(_float(_attr(position, "avgCost"))), multiplier, instrument)
    delta_notional, premium_at_risk = _exposure(side, instrument, qty, avg_price, multiplier)
    contract_ref = _contract_dict(contract)
    return BrokerPosition(
        account=account,
        symbol=symbol,
        con_id=con_id,
        side=side,
        instrument=instrument,
        qty=qty,
        avg_price=avg_price,
        delta_notional=delta_notional,
        premium_at_risk=premium_at_risk,
        as_of=now,
        contract=contract_ref,
        raw={
            "position": signed_qty,
            "avg_cost": _attr(position, "avgCost"),
            "contract": contract_ref,
        },
    )


def normalize_fill(fill: Any, *, now: dt.datetime | None = None) -> BrokerFill | None:
    now = now or dt.datetime.now(dt.UTC)
    contract = _attr(fill, "contract")
    execution = _attr(fill, "execution")
    commission = _attr(fill, "commissionReport")
    symbol = _symbol(contract)
    con_id = _con_id(contract)
    signed_qty = _float(_attr(execution, "shares"))
    account = str(_attr(execution, "acctNumber", "") or "").strip()
    execution_id = str(_attr(execution, "execId", "") or "").strip()
    if not account or not symbol or con_id is None or not execution_id or abs(signed_qty) <= 0:
        return None
    instrument = _instrument(contract, now)
    if instrument is None:
        return None
    side = _side(contract, signed_qty)
    multiplier = _multiplier(contract, instrument)
    price = _unit_price(abs(_float(_attr(execution, "price"))), multiplier, instrument)
    filled_at = _parse_execution_time(_attr(execution, "time"), now)
    contract_ref = _contract_dict(contract)
    return BrokerFill(
        account=account,
        symbol=symbol,
        con_id=con_id,
        execution_id=execution_id,
        side=side,
        instrument=instrument,
        qty=abs(signed_qty),
        price=price,
        fees=abs(_float(_attr(commission, "commission"))),
        filled_at=filled_at,
        contract=contract_ref,
        raw={
            "execution": {
                "exec_id": execution_id,
                "order_id": _attr(execution, "orderId"),
                "perm_id": _attr(execution, "permId"),
                "side": _attr(execution, "side"),
                "shares": _attr(execution, "shares"),
                "price": _attr(execution, "price"),
                "exchange": _attr(execution, "exchange"),
            },
            "commission": {
                "commission": _attr(commission, "commission"),
                "currency": _attr(commission, "currency"),
                "realized_pnl": _attr(commission, "realizedPNL"),
            },
            "contract": contract_ref,
        },
    )


def _parse_execution_time(value: Any, fallback: dt.datetime) -> dt.datetime:
    if isinstance(value, dt.datetime):
        if value.tzinfo is None:
            return value.replace(tzinfo=dt.UTC)
        return value.astimezone(dt.UTC)
    if isinstance(value, str) and value.strip():
        for candidate in (value, value.replace("Z", "+00:00")):
            try:
                parsed = dt.datetime.fromisoformat(candidate)
            except ValueError:
                continue
            if parsed.tzinfo is None:
                parsed = parsed.replace(tzinfo=dt.UTC)
            return parsed.astimezone(dt.UTC)
    return fallback


def _net_liquidation(account_values: list[Any], account: str | None) -> float | None:
    for item in account_values:
        if account and str(_attr(item, "account", "") or "") != account:
            continue
        if str(_attr(item, "tag", "") or "") != "NetLiquidation":
            continue
        value = _float(_attr(item, "value"), -1.0)
        if value > 0:
            return value
    return None


class IbkrGatewayClient:
    def __init__(self, cfg: IbkrConfig) -> None:
        self.cfg = cfg

    def fetch_snapshot(self) -> BrokerSnapshot:
        _ensure_event_loop_for_ib_insync()
        try:
            from ib_insync import IB
        except ImportError as exc:  # pragma: no cover - dependency is pinned in pyproject.
            raise RuntimeError("ib-insync is not installed; run `make py-setup`") from exc

        ib = IB()
        now = dt.datetime.now(dt.UTC)
        try:
            ib.connect(
                self.cfg.host,
                self.cfg.port,
                clientId=self.cfg.client_id,
                timeout=self.cfg.timeout_seconds,
                readonly=self.cfg.readonly,
                account=self.cfg.account or "",
            )
        except (OSError, TimeoutError) as exc:
            raise IbkrConnectionError(
                f"IBKR API not reachable at {self.cfg.host}:{self.cfg.port}; "
                "start TWS/IB Gateway and enable API socket access"
            ) from exc
        try:
            raw_positions = [
                p for p in ib.positions()
                if self.cfg.account is None or str(_attr(p, "account", "")) == self.cfg.account
            ]
            raw_fills = list(ib.fills())
            try:
                raw_fills.extend(ib.reqExecutions())
            except Exception as exc:  # noqa: BLE001 - IBKR may deny execution history by config.
                log.warning("IBKR reqExecutions failed; using session fills only: %s", exc)
            account_values = list(ib.accountValues(self.cfg.account or ""))
        finally:
            ib.disconnect()

        positions = [
            normalized for p in raw_positions
            if (normalized := normalize_position(p, now=now)) is not None
        ]
        fills_by_id: dict[str, BrokerFill] = {}
        for raw_fill in raw_fills:
            normalized = normalize_fill(raw_fill, now=now)
            if normalized is not None:
                fills_by_id[normalized.execution_id] = normalized
        return BrokerSnapshot(
            positions=positions,
            fills=list(fills_by_id.values()),
            net_liquidation=_net_liquidation(account_values, self.cfg.account),
            as_of=now,
        )


def _ensure_event_loop_for_ib_insync() -> None:
    try:
        asyncio.get_event_loop_policy().get_event_loop()
    except RuntimeError:
        asyncio.set_event_loop(asyncio.new_event_loop())


async def apply_snapshot(pool: asyncpg.Pool, snapshot: BrokerSnapshot) -> BrokerSyncResult:
    updates: list[dict[str, Any]] = []
    inserted = updated = closed = fills_inserted = fills_skipped = 0
    portfolio_updated = False

    async with pool.acquire() as conn:
        async with conn.transaction():
            if snapshot.net_liquidation is not None:
                await conn.execute(
                    """INSERT INTO portfolio_settings
                         (id, account_size_usd, high_water_mark_usd, updated_at, updated_by)
                       VALUES (1, $1, $1, now(), 'ibkr-sync')
                       ON CONFLICT (id) DO UPDATE SET
                         account_size_usd = EXCLUDED.account_size_usd,
                         high_water_mark_usd = GREATEST(
                            COALESCE(
                                portfolio_settings.high_water_mark_usd,
                                EXCLUDED.high_water_mark_usd
                            ),
                            EXCLUDED.high_water_mark_usd
                         ),
                         updated_at = now(),
                         updated_by = EXCLUDED.updated_by""",
                    snapshot.net_liquidation,
                )
                portfolio_updated = True

            current_keys: set[tuple[str, int, str, str]] = set()
            for position in snapshot.positions:
                await _ensure_ticker(conn, position.symbol)
                current_keys.add(position.key)
                position_id, was_inserted = await _upsert_broker_position(conn, position)
                if was_inserted:
                    inserted += 1
                else:
                    updated += 1
                updates.append(_position_update_payload(position, str(position_id), "open"))

            closed_payloads = await _close_missing_positions(conn, current_keys, snapshot.as_of)
            closed += len(closed_payloads)
            updates.extend(closed_payloads)

            for fill in snapshot.fills:
                await _ensure_ticker(conn, fill.symbol)
                did_insert = await _insert_broker_fill(conn, fill)
                if did_insert:
                    fills_inserted += 1
                else:
                    fills_skipped += 1

    return BrokerSyncResult(
        positions_inserted=inserted,
        positions_updated=updated,
        positions_closed=closed,
        fills_inserted=fills_inserted,
        fills_skipped=fills_skipped,
        portfolio_updated=portfolio_updated,
        updates=tuple(updates),
    )


async def _ensure_ticker(conn: asyncpg.Connection, symbol: str) -> None:
    await conn.execute(
        """INSERT INTO ticker (symbol, tier, status)
           VALUES ($1, 3, 'active')
           ON CONFLICT (symbol) DO NOTHING""",
        symbol,
    )


async def _upsert_broker_position(
    conn: asyncpg.Connection,
    position: BrokerPosition,
) -> tuple[Any, bool]:
    row = await conn.fetchrow(
        """SELECT position_id
             FROM position
            WHERE source = 'broker'
              AND broker = $1
              AND broker_account = $2
              AND broker_con_id = $3
              AND side = $4
              AND instrument = $5
              AND closed_at IS NULL
            LIMIT 1
            FOR UPDATE""",
        BROKER,
        position.account,
        position.con_id,
        position.side,
        position.instrument,
    )
    if row is None:
        position_id = await conn.fetchval(
            """INSERT INTO position
                 (thesis_id, symbol, side, instrument, qty, avg_price,
                  delta_notional, premium_at_risk, opened_at, source, broker,
                  broker_account, broker_con_id, broker_contract, broker_last_sync_at)
               VALUES (NULL, $1, $2, $3, $4, $5, $6, $7, $8, 'broker', $9, $10, $11, $12::jsonb, $8)
            RETURNING position_id""",
            position.symbol,
            position.side,
            position.instrument,
            position.qty,
            position.avg_price,
            position.delta_notional,
            position.premium_at_risk,
            position.as_of,
            BROKER,
            position.account,
            position.con_id,
            json.dumps(position.contract, default=_json_default),
        )
        return position_id, True

    position_id = row["position_id"]
    await conn.execute(
        """UPDATE position
              SET symbol = $2,
                  qty = $3,
                  avg_price = $4,
                  delta_notional = $5,
                  premium_at_risk = $6,
                  broker_contract = $7::jsonb,
                  broker_last_sync_at = $8
            WHERE position_id = $1""",
        position_id,
        position.symbol,
        position.qty,
        position.avg_price,
        position.delta_notional,
        position.premium_at_risk,
        json.dumps(position.contract, default=_json_default),
        position.as_of,
    )
    return position_id, False


async def _close_missing_positions(
    conn: asyncpg.Connection,
    current_keys: set[tuple[str, int, str, str]],
    closed_at: dt.datetime,
) -> list[dict[str, Any]]:
    rows = await conn.fetch(
        """SELECT position_id, broker_account, broker_con_id, side, instrument, symbol
             FROM position
            WHERE source = 'broker'
              AND broker = $1
              AND closed_at IS NULL
            FOR UPDATE""",
        BROKER,
    )
    updates: list[dict[str, Any]] = []
    for row in rows:
        key = (row["broker_account"], int(row["broker_con_id"]), row["side"], row["instrument"])
        if key in current_keys:
            continue
        await conn.execute(
            """UPDATE position
                  SET closed_at = $2,
                      broker_last_sync_at = $2
                WHERE position_id = $1""",
            row["position_id"],
            closed_at,
        )
        updates.append({
            "source": BROKER,
            "status": "closed",
            "position_id": str(row["position_id"]),
            "account": row["broker_account"],
            "symbol": row["symbol"],
            "con_id": row["broker_con_id"],
            "side": row["side"],
            "instrument": row["instrument"],
            "as_of": closed_at.isoformat(),
        })
    return updates


async def _insert_broker_fill(conn: asyncpg.Connection, fill: BrokerFill) -> bool:
    position = await conn.fetchrow(
        """SELECT position_id, thesis_id, side, instrument
             FROM position
            WHERE source = 'broker'
              AND broker = $1
              AND broker_account = $2
              AND broker_con_id = $3
              AND closed_at IS NULL
            ORDER BY broker_last_sync_at DESC NULLS LAST
            LIMIT 1""",
        BROKER,
        fill.account,
        fill.con_id,
    )
    if position is None:
        log.info(
            "skipping broker fill with no open broker position: %s %s",
            fill.symbol,
            fill.execution_id,
        )
        return False

    inserted = await conn.fetchval(
        """INSERT INTO position_fill
             (position_id, ticket_id, decision_id, thesis_id, symbol, side, instrument,
              qty, price, fees, filled_at, source, notes, raw, broker, broker_account,
              broker_execution_id)
           VALUES ($1, NULL, NULL, $2, $3, $4, $5, $6, $7, $8, $9, 'broker',
                   'IBKR broker import', $10::jsonb, $11, $12, $13)
           ON CONFLICT DO NOTHING
        RETURNING fill_id""",
        position["position_id"],
        position["thesis_id"],
        fill.symbol,
        position["side"] or fill.side,
        position["instrument"] or fill.instrument,
        fill.qty,
        fill.price,
        fill.fees,
        fill.filled_at,
        json.dumps(fill.raw, default=_json_default),
        BROKER,
        fill.account,
        fill.execution_id,
    )
    return inserted is not None


def _position_update_payload(
    position: BrokerPosition,
    position_id: str,
    status: str,
) -> dict[str, Any]:
    return {
        "source": BROKER,
        "status": status,
        "position_id": position_id,
        "account": position.account,
        "symbol": position.symbol,
        "con_id": position.con_id,
        "side": position.side,
        "instrument": position.instrument,
        "qty": position.qty,
        "avg_price": position.avg_price,
        "delta_notional": position.delta_notional,
        "premium_at_risk": position.premium_at_risk,
        "as_of": position.as_of.isoformat(),
    }


def _json_default(value: Any) -> str:
    if hasattr(value, "isoformat"):
        return value.isoformat()
    return str(value)


async def publish_updates(nats_url: str, updates: tuple[dict[str, Any], ...]) -> None:
    if not updates:
        return
    nc = await nats.connect(nats_url)
    js = nc.jetstream()
    try:
        try:
            await js.stream_info(MARKET_STREAM)
        except NotFoundError:
            await js.add_stream(name=MARKET_STREAM, subjects=MARKET_STREAM_SUBJECTS)
        for update in updates:
            await js.publish(POSITION_UPDATED, json.dumps(update, default=_json_default).encode())
    finally:
        await nc.drain()


async def sync_once() -> BrokerSyncResult:
    cfg = config.load()
    ibkr_cfg = load_ibkr_config()
    client = IbkrGatewayClient(ibkr_cfg)
    snapshot = await asyncio.to_thread(client.fetch_snapshot)
    pool = await asyncpg.create_pool(cfg.database_url, min_size=1, max_size=2)
    assert pool is not None
    try:
        result = await apply_snapshot(pool, snapshot)
    finally:
        await pool.close()
    await publish_updates(cfg.nats_url, result.updates)
    return result


async def sync_loop() -> None:
    ibkr_cfg = load_ibkr_config()
    while True:
        try:
            result = await sync_once()
            log.info(
                "IBKR sync ok: positions inserted=%d updated=%d closed=%d fills=%d skipped=%d",
                result.positions_inserted,
                result.positions_updated,
                result.positions_closed,
                result.fills_inserted,
                result.fills_skipped,
            )
        except Exception:
            log.exception("IBKR sync failed")
        await asyncio.sleep(ibkr_cfg.sync_interval_seconds)


def _parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Sync IBKR positions/fills into stocks")
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--once", action="store_true", help="run one sync pass and exit")
    mode.add_argument("--loop", action="store_true", help="run continuously")
    return parser.parse_args()


def _cli() -> None:
    logging.basicConfig(level=logging.INFO, format="%(asctime)s %(name)s %(levelname)s %(message)s")
    args = _parse_args()
    try:
        if args.loop:
            asyncio.run(sync_loop())
        else:
            result = asyncio.run(sync_once())
            print(json.dumps({
                "positions_inserted": result.positions_inserted,
                "positions_updated": result.positions_updated,
                "positions_closed": result.positions_closed,
                "fills_inserted": result.fills_inserted,
                "fills_skipped": result.fills_skipped,
                "portfolio_updated": result.portfolio_updated,
            }, indent=2))
    except IbkrConnectionError as exc:
        raise SystemExit(str(exc)) from None


if __name__ == "__main__":
    _cli()
