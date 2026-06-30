"""Disabled-by-default IBKR paper order adapter (#297).

The module has three layers:

* pure order planning and safety gates;
* a fake broker used by tests;
* an optional ib_insync transport used only when explicitly enabled.

Live account placement is intentionally outside this code path. The adapter only
accepts `environment_scope='paper'` and IBKR paper accounts (`DU...`).
"""

from __future__ import annotations

import argparse
import asyncio
import dataclasses
import datetime as dt
import json
import logging
import os
from dataclasses import dataclass, field
from enum import Enum
from typing import Any, Protocol

import asyncpg

from . import config as app_config

log = logging.getLogger("ibkr_paper_orders")

BROKER = "ibkr"
PAPER_ACCOUNT_PREFIX = "DU"
LIVE_PORTS = {4001, 4003, 7496}
PAPER_ORDER_UPDATED = "automation.paper_order.updated"


class BrokerFault(Enum):
    NONE = "none"
    DISCONNECT = "disconnect"
    REJECT = "reject"
    ERROR = "error"


class BrokerSubmission(Enum):
    SUBMITTED = "submitted"
    PARTIAL_FILL = "partial_fill"
    FILLED = "filled"
    CANCELLED = "cancelled"


@dataclass(frozen=True)
class PaperOrderConfig:
    db_enabled: bool = False
    env_enabled: bool = False
    broker: str = BROKER
    account_mode: str = "paper"
    broker_account: str | None = None
    max_position_snapshot_age_seconds: int = 120

    def replace(self, **changes: Any) -> PaperOrderConfig:
        return dataclasses.replace(self, **changes)

    @property
    def enabled(self) -> bool:
        return self.db_enabled and self.env_enabled


@dataclass(frozen=True)
class PaperOrderContext:
    reconciliation_id: str
    desired_position_id: str
    proof_id: str | None
    sleeve_id: str
    symbol: str
    environment_scope: str
    reconciliation_status: str
    proof_result: str | None
    blocked_reasons: list[str]
    existing_order_count: int
    position_snapshot_at: dt.datetime | None
    now: dt.datetime
    order_plan: dict[str, Any]

    def replace(self, **changes: Any) -> PaperOrderContext:
        return dataclasses.replace(self, **changes)


@dataclass(frozen=True)
class PreparedPaperOrder:
    client_order_id: str
    parent_client_order_id: str | None
    order_role: str
    symbol: str
    action: str
    quantity: float
    order_type: str
    position_side: str
    limit_price: float | None = None
    stop_price: float | None = None
    transmit: bool = True
    broker_order_id: str | None = None
    status: str = "planned"
    raw: dict[str, Any] = field(default_factory=dict)


@dataclass(frozen=True)
class PaperOrderEvent:
    client_order_id: str
    event_kind: str
    status: str
    broker_order_id: str | None = None
    filled_quantity: float | None = None
    fill_price: float | None = None
    message: str | None = None
    raw: dict[str, Any] = field(default_factory=dict)


@dataclass(frozen=True)
class PaperOrderIncident:
    severity: str
    kind: str
    title: str
    detail: str


@dataclass(frozen=True)
class PaperOrderOutcome:
    status: str
    blocked_reasons: list[str]
    orders: list[PreparedPaperOrder]
    events: list[PaperOrderEvent]
    incident: PaperOrderIncident | None = None

    @property
    def submitted(self) -> bool:
        return self.status in {"submitted", "reconciled"}


@dataclass(frozen=True)
class BrokerSubmitResult:
    submission: BrokerSubmission
    orders: list[PreparedPaperOrder]
    events: list[PaperOrderEvent]


class PaperBroker(Protocol):
    def submit(self, orders: list[PreparedPaperOrder]) -> BrokerSubmitResult:
        """Submit an already gated paper order plan."""


@dataclass
class FakePaperBroker:
    fault: BrokerFault = BrokerFault.NONE
    submission: BrokerSubmission = BrokerSubmission.SUBMITTED
    calls: int = 0

    def submit(self, orders: list[PreparedPaperOrder]) -> BrokerSubmitResult:
        self.calls += 1
        if self.fault == BrokerFault.DISCONNECT:
            raise BrokerDisconnected("paper broker disconnected")
        if self.fault == BrokerFault.ERROR:
            raise BrokerOrderError("paper broker returned an error")
        if self.fault == BrokerFault.REJECT:
            return BrokerSubmitResult(
                submission=BrokerSubmission.CANCELLED,
                orders=[
                    dataclasses.replace(
                        order,
                        status="rejected",
                        broker_order_id=f"FAKE-{idx + 1}",
                    )
                    for idx, order in enumerate(orders)
                ],
                events=[
                    PaperOrderEvent(
                        client_order_id=order.client_order_id,
                        event_kind="rejected",
                        status="rejected",
                        broker_order_id=f"FAKE-{idx + 1}",
                        message="fake reject",
                    )
                    for idx, order in enumerate(orders)
                ],
            )
        status = {
            BrokerSubmission.SUBMITTED: "submitted",
            BrokerSubmission.PARTIAL_FILL: "partially_filled",
            BrokerSubmission.FILLED: "filled",
            BrokerSubmission.CANCELLED: "cancelled",
        }[self.submission]
        event_kind = {
            BrokerSubmission.SUBMITTED: "submitted",
            BrokerSubmission.PARTIAL_FILL: "partial_fill",
            BrokerSubmission.FILLED: "fill",
            BrokerSubmission.CANCELLED: "cancelled",
        }[self.submission]
        submitted = [
            dataclasses.replace(order, status=status, broker_order_id=f"FAKE-{idx + 1}")
            for idx, order in enumerate(orders)
        ]
        events = [
            PaperOrderEvent(
                client_order_id=order.client_order_id,
                event_kind=event_kind,
                status=status,
                broker_order_id=f"FAKE-{idx + 1}",
                filled_quantity=order.quantity / 2
                if self.submission == BrokerSubmission.PARTIAL_FILL
                else (order.quantity if self.submission == BrokerSubmission.FILLED else None),
                fill_price=order.limit_price or order.stop_price,
                message=f"fake {event_kind}",
            )
            for idx, order in enumerate(orders)
        ]
        return BrokerSubmitResult(submission=self.submission, orders=submitted, events=events)


class BrokerDisconnected(RuntimeError):
    pass


class BrokerOrderError(RuntimeError):
    pass


def _blocked(reason: str, *, incident: PaperOrderIncident | None = None) -> PaperOrderOutcome:
    return PaperOrderOutcome(
        status="blocked",
        blocked_reasons=[reason],
        orders=[],
        events=[],
        incident=incident,
    )


def _incident(kind: str, title: str, detail: str, severity: str = "critical") -> PaperOrderOutcome:
    return PaperOrderOutcome(
        status="incident",
        blocked_reasons=[kind],
        orders=[],
        events=[],
        incident=PaperOrderIncident(
            severity=severity,
            kind=kind,
            title=title,
            detail=detail,
        ),
    )


def plan_paper_order_submission(
    ctx: PaperOrderContext,
    cfg: PaperOrderConfig,
) -> PaperOrderOutcome:
    if not cfg.enabled:
        return _blocked("paper_order_adapter_disabled")
    if cfg.broker != BROKER:
        return _blocked("paper_broker_mismatch")
    if cfg.account_mode != "paper":
        return _blocked("paper_mode_required")
    if not cfg.broker_account or not cfg.broker_account.upper().startswith(PAPER_ACCOUNT_PREFIX):
        return _blocked("paper_account_required")
    if ctx.environment_scope != "paper":
        return _blocked("environment_not_paper")
    if ctx.proof_result not in {"passed", "warning"}:
        return _blocked("proof_not_passed")
    if ctx.reconciliation_status != "needs_order":
        return _blocked("reconciliation_not_needs_order")
    if ctx.blocked_reasons:
        return _blocked("reconciliation_blocked")
    if ctx.existing_order_count > 0:
        return _blocked("duplicate_order_attempt")
    if _snapshot_stale(ctx.position_snapshot_at, ctx.now, cfg.max_position_snapshot_age_seconds):
        return _blocked(
            "stale_broker_position_snapshot",
            incident=PaperOrderIncident(
                severity="warning",
                kind="stale_broker_position_snapshot",
                title="IBKR paper position snapshot is stale",
                detail=f"Latest IBKR paper snapshot for {ctx.symbol} is missing or stale.",
            ),
        )

    orders = _prepare_orders(ctx)
    if not orders:
        return _blocked("order_plan_empty")
    return PaperOrderOutcome(
        status="ready",
        blocked_reasons=[],
        orders=orders,
        events=[],
    )


def submit_with_broker(
    ctx: PaperOrderContext,
    cfg: PaperOrderConfig,
    broker: PaperBroker,
) -> PaperOrderOutcome:
    planned = plan_paper_order_submission(ctx, cfg)
    if planned.status != "ready":
        return planned

    try:
        submitted = broker.submit(planned.orders)
    except BrokerDisconnected:
        return _incident(
            "paper_broker_disconnect",
            "IBKR paper broker disconnected",
            f"IBKR paper order submission disconnected for {ctx.symbol}.",
        )
    except Exception as exc:  # noqa: BLE001 - broker clients surface heterogeneous errors.
        return _incident(
            "paper_order_error",
            "IBKR paper order submission failed",
            f"IBKR paper order submission failed for {ctx.symbol}: {exc}",
        )

    if any(event.event_kind == "rejected" for event in submitted.events):
        return PaperOrderOutcome(
            status="incident",
            blocked_reasons=["paper_order_rejected"],
            orders=submitted.orders,
            events=submitted.events,
            incident=PaperOrderIncident(
                severity="critical",
                kind="paper_order_rejected",
                title="IBKR paper order rejected",
                detail=f"IBKR paper rejected an order for {ctx.symbol}.",
            ),
        )
    if submitted.submission == BrokerSubmission.PARTIAL_FILL:
        return PaperOrderOutcome(
            status="submitted",
            blocked_reasons=["paper_order_partial_fill"],
            orders=submitted.orders,
            events=submitted.events,
            incident=PaperOrderIncident(
                severity="warning",
                kind="paper_order_partial_fill",
                title="IBKR paper order partially filled",
                detail=f"IBKR paper partially filled an order for {ctx.symbol}.",
            ),
        )
    if submitted.submission == BrokerSubmission.FILLED:
        return PaperOrderOutcome(
            status="reconciled",
            blocked_reasons=[],
            orders=submitted.orders,
            events=submitted.events,
        )
    if submitted.submission == BrokerSubmission.CANCELLED:
        return PaperOrderOutcome(
            status="incident",
            blocked_reasons=["paper_order_cancelled"],
            orders=submitted.orders,
            events=submitted.events,
            incident=PaperOrderIncident(
                severity="warning",
                kind="paper_order_cancelled",
                title="IBKR paper order cancelled",
                detail=f"IBKR paper cancelled an order for {ctx.symbol}.",
            ),
        )
    return PaperOrderOutcome(
        status="submitted",
        blocked_reasons=[],
        orders=submitted.orders,
        events=submitted.events,
    )


def _snapshot_stale(
    seen_at: dt.datetime | None,
    now: dt.datetime,
    max_age_seconds: int,
) -> bool:
    if seen_at is None:
        return True
    if seen_at.tzinfo is None:
        seen_at = seen_at.replace(tzinfo=dt.UTC)
    return (now - seen_at).total_seconds() > max(0, max_age_seconds)


def _prepare_orders(ctx: PaperOrderContext) -> list[PreparedPaperOrder]:
    raw_orders = ctx.order_plan.get("orders")
    if not isinstance(raw_orders, list):
        return []
    orders: list[PreparedPaperOrder] = []
    for idx, raw in enumerate(raw_orders):
        if not isinstance(raw, dict):
            continue
        quantity = _positive_float(raw.get("quantity"))
        action = _order_action(raw.get("action"))
        if quantity is None or action is None:
            continue
        order_type = str(raw.get("type") or "market").lower()
        parent_id = str(raw.get("client_order_id") or f"{ctx.desired_position_id}:{idx}")
        parent = PreparedPaperOrder(
            client_order_id=parent_id,
            parent_client_order_id=None,
            order_role="parent",
            symbol=ctx.symbol,
            action=action,
            quantity=quantity,
            order_type="market" if order_type == "market" else "limit",
            position_side=str(raw.get("position_side") or "long"),
            limit_price=_positive_float(raw.get("price")) if order_type == "limit" else None,
            transmit=not _has_bracket(raw),
            raw=raw,
        )
        orders.append(parent)
        bracket = raw.get("bracket")
        if isinstance(bracket, dict):
            take_profit = _positive_float(bracket.get("take_profit_price"))
            stop_price = _positive_float(bracket.get("stop_price"))
            close_action = _closing_action(action)
            if take_profit is not None:
                orders.append(
                    PreparedPaperOrder(
                        client_order_id=f"{parent_id}:tp",
                        parent_client_order_id=parent_id,
                        order_role="take_profit",
                        symbol=ctx.symbol,
                        action=close_action,
                        quantity=quantity,
                        order_type="limit",
                        position_side=parent.position_side,
                        limit_price=take_profit,
                        transmit=False,
                        raw={"bracket": bracket},
                    )
                )
            if stop_price is not None:
                orders.append(
                    PreparedPaperOrder(
                        client_order_id=f"{parent_id}:sl",
                        parent_client_order_id=parent_id,
                        order_role="stop_loss",
                        symbol=ctx.symbol,
                        action=close_action,
                        quantity=quantity,
                        order_type="stop",
                        position_side=parent.position_side,
                        stop_price=stop_price,
                        transmit=True,
                        raw={"bracket": bracket},
                    )
                )
    return orders


def _positive_float(value: Any) -> float | None:
    try:
        parsed = float(value)
    except (TypeError, ValueError):
        return None
    if parsed <= 0 or parsed != parsed:
        return None
    return parsed


def _order_action(value: Any) -> str | None:
    action = str(value or "").lower()
    return action if action in {"buy", "sell", "sell_short", "buy_to_cover"} else None


def _closing_action(action: str) -> str:
    if action == "buy":
        return "sell"
    if action == "sell_short":
        return "buy_to_cover"
    if action == "sell":
        return "buy"
    return "sell_short"


def _has_bracket(raw: dict[str, Any]) -> bool:
    bracket = raw.get("bracket")
    return isinstance(bracket, dict) and (
        _positive_float(bracket.get("take_profit_price")) is not None
        or _positive_float(bracket.get("stop_price")) is not None
    )


class IbkrPaperBroker:
    def __init__(
        self,
        host: str,
        port: int,
        client_id: int,
        timeout_seconds: float,
        account: str,
    ) -> None:
        if port in LIVE_PORTS:
            raise ValueError(f"refusing live-looking IBKR API port {port}")
        self.host = host
        self.port = port
        self.client_id = client_id
        self.timeout_seconds = timeout_seconds
        self.account = account
        self._ib: Any | None = None
        self._stock_cls: Any | None = None
        self._order_cls: Any | None = None

    async def connect(self) -> None:
        from ib_insync import IB, Order, Stock

        self._ib = IB()
        self._stock_cls = Stock
        self._order_cls = Order
        await asyncio.wait_for(
            self._ib.connectAsync(self.host, self.port, clientId=self.client_id),
            timeout=self.timeout_seconds,
        )

    def submit(self, orders: list[PreparedPaperOrder]) -> BrokerSubmitResult:
        if self._ib is None or self._stock_cls is None or self._order_cls is None:
            raise BrokerDisconnected("IBKR paper broker is not connected")
        contract = self._stock_cls(orders[0].symbol, "SMART", "USD")
        trades = []
        parent_order_ids: dict[str, int] = {}
        for order in orders:
            ib_order = self._ib_order(order)
            if order.parent_client_order_id is not None:
                parent_id = parent_order_ids.get(order.parent_client_order_id)
                if parent_id is None:
                    raise BrokerOrderError("bracket child missing parent order id")
                ib_order.parentId = parent_id
            trade = self._ib.placeOrder(contract, ib_order)
            broker_order_id = str(getattr(trade.order, "orderId", "") or "")
            if order.parent_client_order_id is None and broker_order_id:
                parent_order_ids[order.client_order_id] = int(broker_order_id)
            trades.append((order, trade, broker_order_id))

        submitted_orders = [
            dataclasses.replace(order, status=_trade_status(trade), broker_order_id=broker_order_id)
            for order, trade, broker_order_id in trades
        ]
        events = [
            _trade_event(order, trade, broker_order_id)
            for order, trade, broker_order_id in trades
        ]
        if any(event.event_kind == "rejected" for event in events):
            submission = BrokerSubmission.CANCELLED
        elif any(event.event_kind == "partial_fill" for event in events):
            submission = BrokerSubmission.PARTIAL_FILL
        elif events and all(event.event_kind == "fill" for event in events):
            submission = BrokerSubmission.FILLED
        else:
            submission = BrokerSubmission.SUBMITTED
        return BrokerSubmitResult(submission=submission, orders=submitted_orders, events=events)

    def _ib_order(self, order: PreparedPaperOrder) -> Any:
        ib_order = self._order_cls()
        ib_order.action = {
            "buy": "BUY",
            "sell": "SELL",
            "sell_short": "SELL",
            "buy_to_cover": "BUY",
        }[order.action]
        ib_order.totalQuantity = order.quantity
        ib_order.orderType = {
            "market": "MKT",
            "limit": "LMT",
            "stop": "STP",
        }[order.order_type]
        if order.limit_price is not None:
            ib_order.lmtPrice = order.limit_price
        if order.stop_price is not None:
            ib_order.auxPrice = order.stop_price
        ib_order.transmit = order.transmit
        ib_order.orderRef = order.client_order_id
        ib_order.account = self.account
        return ib_order


def _trade_status(trade: Any) -> str:
    status = str(getattr(getattr(trade, "orderStatus", None), "status", "") or "").lower()
    return {
        "submitted": "submitted",
        "presubmitted": "submitted",
        "filled": "filled",
        "partiallyfilled": "partially_filled",
        "cancelled": "cancelled",
        "apicancelled": "cancelled",
        "inactive": "rejected",
    }.get(status, "submitted")


def _trade_event(
    order: PreparedPaperOrder,
    trade: Any,
    broker_order_id: str | None,
) -> PaperOrderEvent:
    status = _trade_status(trade)
    event_kind = {
        "filled": "fill",
        "partially_filled": "partial_fill",
        "cancelled": "cancelled",
        "rejected": "rejected",
    }.get(status, "submitted")
    order_status = getattr(trade, "orderStatus", None)
    filled = getattr(order_status, "filled", None)
    avg_fill_price = getattr(order_status, "avgFillPrice", None)
    return PaperOrderEvent(
        client_order_id=order.client_order_id,
        event_kind=event_kind,
        status=status,
        broker_order_id=broker_order_id,
        filled_quantity=_positive_float(filled),
        fill_price=_positive_float(avg_fill_price),
        raw=_jsonable_trade(trade),
    )


def _jsonable_trade(trade: Any) -> dict[str, Any]:
    order_status = getattr(trade, "orderStatus", None)
    return {
        "status": getattr(order_status, "status", None),
        "filled": getattr(order_status, "filled", None),
        "remaining": getattr(order_status, "remaining", None),
        "avg_fill_price": getattr(order_status, "avgFillPrice", None),
    }


def _env_bool(name: str, default: bool = False) -> bool:
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
        return default


def _env_float(name: str, default: float) -> float:
    raw = os.getenv(name)
    if raw is None or raw == "":
        return default
    try:
        return float(raw)
    except ValueError:
        return default


async def load_paper_order_config(pool: asyncpg.Pool) -> PaperOrderConfig:
    row = await pool.fetchrow(
        """SELECT enabled, broker, account_mode, broker_account,
                  max_position_snapshot_age_seconds
             FROM automation_paper_order_config
            WHERE config_id = 1"""
    )
    if row is None:
        return PaperOrderConfig(env_enabled=_env_bool("IBKR_PAPER_ORDERS_ENABLED", False))
    return PaperOrderConfig(
        db_enabled=bool(row["enabled"]),
        env_enabled=_env_bool("IBKR_PAPER_ORDERS_ENABLED", False),
        broker=str(row["broker"]),
        account_mode=str(row["account_mode"]),
        broker_account=row["broker_account"],
        max_position_snapshot_age_seconds=int(row["max_position_snapshot_age_seconds"]),
    )


async def load_next_context(pool: asyncpg.Pool) -> PaperOrderContext | None:
    row = await pool.fetchrow(
        """SELECT ar.reconciliation_id::text,
                  ar.desired_position_id::text,
                  ar.proof_id::text,
                  ar.sleeve_id::text,
                  ar.symbol,
                  ar.environment_scope,
                  ar.status AS reconciliation_status,
                  COALESCE(ap.result, 'blocked') AS proof_result,
                  ar.blocked_reasons,
                  ar.order_plan,
                  COALESCE(existing.order_count, 0)::int AS existing_order_count,
                  GREATEST(pos.latest_position_sync_at, ps.updated_at) AS position_snapshot_at
             FROM automation_execution_reconciliation ar
        LEFT JOIN automation_proof ap ON ap.proof_id = ar.proof_id
        LEFT JOIN LATERAL (
                  SELECT COUNT(*) AS order_count
                    FROM automation_broker_order bo
                   WHERE bo.reconciliation_id = ar.reconciliation_id
             ) existing ON TRUE
        LEFT JOIN LATERAL (
                  SELECT MAX(broker_last_sync_at) AS latest_position_sync_at
                    FROM position
                   WHERE source = 'broker'
                     AND broker = 'ibkr'
             ) pos ON TRUE
        LEFT JOIN portfolio_settings ps
               ON ps.id = 1
              AND ps.updated_by = 'ibkr-sync'
            WHERE ar.environment_scope = 'paper'
              AND ar.status = 'needs_order'
         ORDER BY ar.created_at ASC
            LIMIT 1"""
    )
    if row is None:
        return None
    return PaperOrderContext(
        reconciliation_id=row["reconciliation_id"],
        desired_position_id=row["desired_position_id"],
        proof_id=row["proof_id"],
        sleeve_id=row["sleeve_id"],
        symbol=row["symbol"],
        environment_scope=row["environment_scope"],
        reconciliation_status=row["reconciliation_status"],
        proof_result=row["proof_result"],
        blocked_reasons=list(row["blocked_reasons"] or []),
        existing_order_count=int(row["existing_order_count"]),
        position_snapshot_at=row["position_snapshot_at"],
        now=dt.datetime.now(dt.UTC),
        order_plan=dict(row["order_plan"] or {}),
    )


async def persist_outcome(
    pool: asyncpg.Pool,
    ctx: PaperOrderContext,
    cfg: PaperOrderConfig,
    outcome: PaperOrderOutcome,
) -> None:
    async with pool.acquire() as conn:
        async with conn.transaction():
            for order in outcome.orders:
                await conn.execute(
                    """INSERT INTO automation_broker_order
                         (reconciliation_id, desired_position_id, proof_id, sleeve_id, symbol,
                          environment_scope, broker, broker_account, client_order_id,
                          broker_order_id, parent_client_order_id, order_role, action,
                          position_side, order_type, quantity, limit_price, stop_price,
                          transmit, status, raw)
                       VALUES ($1::uuid, $2::uuid, $3::uuid, $4::uuid, $5, 'paper', 'ibkr',
                               $6, $7, $8, $9,
                               $10, $11, $12, $13, $14, $15, $16, $17, $18, $19::jsonb)
                       ON CONFLICT (broker, broker_account, client_order_id) DO UPDATE SET
                         broker_order_id = COALESCE(EXCLUDED.broker_order_id,
                                                    automation_broker_order.broker_order_id),
                         status = EXCLUDED.status,
                         raw = EXCLUDED.raw,
                         updated_at = now()""",
                    ctx.reconciliation_id,
                    ctx.desired_position_id,
                    ctx.proof_id,
                    ctx.sleeve_id,
                    ctx.symbol,
                    cfg.broker_account,
                    order.client_order_id,
                    order.broker_order_id,
                    order.parent_client_order_id,
                    order.order_role,
                    order.action,
                    order.position_side,
                    order.order_type,
                    order.quantity,
                    order.limit_price,
                    order.stop_price,
                    order.transmit,
                    order.status,
                    json.dumps(dataclasses.asdict(order), default=_json_default),
                )
            for event in outcome.events:
                await conn.execute(
                    """INSERT INTO automation_broker_order_event
                         (reconciliation_id, symbol, broker, broker_account, client_order_id,
                          broker_order_id, event_kind, status, filled_quantity, fill_price,
                          message, raw)
                       VALUES ($1::uuid, $2, 'ibkr', $3, $4, $5, $6, $7, $8, $9, $10,
                               $11::jsonb)""",
                    ctx.reconciliation_id,
                    ctx.symbol,
                    cfg.broker_account,
                    event.client_order_id,
                    event.broker_order_id,
                    event.event_kind,
                    event.status,
                    event.filled_quantity,
                    event.fill_price,
                    event.message,
                    json.dumps(dataclasses.asdict(event), default=_json_default),
                )
            await conn.execute(
                """UPDATE automation_execution_reconciliation
                      SET status = $2,
                          blocked_reasons = $3::jsonb,
                          updated_at = now()
                    WHERE reconciliation_id = $1::uuid""",
                ctx.reconciliation_id,
                outcome.status if outcome.status != "ready" else "pending",
                json.dumps(outcome.blocked_reasons),
            )
            if outcome.incident is not None:
                await conn.execute(
                    """INSERT INTO automation_incident
                         (severity, kind, symbol, sleeve_id, desired_position_id,
                          proof_id, reconciliation_id, title, detail, source_ref)
                       VALUES ($1, $2, $3, $4::uuid, $5::uuid, $6::uuid, $7::uuid, $8, $9,
                               $10::jsonb)""",
                    outcome.incident.severity,
                    outcome.incident.kind,
                    ctx.symbol,
                    ctx.sleeve_id,
                    ctx.desired_position_id,
                    ctx.proof_id,
                    ctx.reconciliation_id,
                    outcome.incident.title,
                    outcome.incident.detail,
                    json.dumps({"source": "ibkr_paper_orders"}),
                )


def _json_default(value: Any) -> str:
    if isinstance(value, (dt.datetime, dt.date)):
        return value.isoformat()
    return str(value)


async def run_once(pool: asyncpg.Pool) -> PaperOrderOutcome | None:
    cfg = await load_paper_order_config(pool)
    ctx = await load_next_context(pool)
    if ctx is None:
        return None
    planned = plan_paper_order_submission(ctx, cfg)
    if planned.status == "ready":
        assert cfg.broker_account is not None
        broker = IbkrPaperBroker(
            host=os.getenv("IBKR_PAPER_ORDER_HOST", os.getenv("IBKR_HOST", "127.0.0.1")),
            port=_env_int("IBKR_PAPER_ORDER_PORT", _env_int("IBKR_PORT", 7497)),
            client_id=_env_int("IBKR_PAPER_ORDER_CLIENT_ID", 91),
            timeout_seconds=_env_float("IBKR_PAPER_ORDER_TIMEOUT_SECONDS", 6.0),
            account=cfg.broker_account,
        )
        try:
            await broker.connect()
        except Exception as exc:  # noqa: BLE001 - ib_insync connection errors vary.
            outcome = _incident(
                "paper_broker_disconnect",
                "IBKR paper broker disconnected",
                f"IBKR paper connection failed for {ctx.symbol}: {exc}",
            )
        else:
            outcome = submit_with_broker(ctx, cfg, broker)
    else:
        outcome = planned
    await persist_outcome(pool, ctx, cfg, outcome)
    return outcome


async def run_loop(pool: asyncpg.Pool) -> None:
    interval = max(5, _env_int("IBKR_PAPER_ORDER_INTERVAL_SECONDS", 30))
    while True:
        try:
            outcome = await run_once(pool)
            if outcome is not None:
                log.info("paper order outcome: %s", outcome.status)
        except Exception:
            log.exception("paper order adapter failed")
        await asyncio.sleep(interval)


async def _amain(loop: bool) -> None:
    cfg = app_config.load()
    pool = await asyncpg.create_pool(cfg.database_url, min_size=1, max_size=2)
    assert pool is not None
    try:
        if loop:
            await run_loop(pool)
        else:
            outcome = await run_once(pool)
            print(
                json.dumps(
                    dataclasses.asdict(outcome) if outcome else None,
                    default=_json_default,
                )
            )
    finally:
        await pool.close()


def main() -> None:
    logging.basicConfig(level=os.getenv("LOG_LEVEL", "INFO"))
    parser = argparse.ArgumentParser(description="Submit explicitly enabled IBKR paper orders")
    parser.add_argument("--once", action="store_true")
    parser.add_argument("--loop", action="store_true")
    args = parser.parse_args()
    asyncio.run(_amain(loop=args.loop and not args.once))


if __name__ == "__main__":
    main()
