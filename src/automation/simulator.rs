use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{Value, json};
use uuid::Uuid;

use super::TargetSide;

const EPS: f64 = 0.000_001;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SimulatedFault {
    #[default]
    None,
    Reject,
    Disconnect,
    StaleSnapshot,
    UnknownOrderStatus,
}

#[derive(Debug, Clone)]
pub struct SimulationConfig {
    pub fill_fraction: f64,
    pub fault: SimulatedFault,
    pub bracket_stop_loss_pct: f64,
    pub bracket_take_profit_pct: f64,
    pub max_snapshot_age_secs: i64,
    pub broker_last_seen_at: Option<DateTime<Utc>>,
    pub prior_reject_count: u32,
}

impl Default for SimulationConfig {
    fn default() -> Self {
        Self {
            fill_fraction: 1.0,
            fault: SimulatedFault::None,
            bracket_stop_loss_pct: 0.08,
            bracket_take_profit_pct: 0.16,
            max_snapshot_age_secs: 120,
            broker_last_seen_at: None,
            prior_reject_count: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct SimulatedPosition {
    pub side: TargetSide,
    pub quantity: f64,
    pub avg_price: f64,
}

impl Default for SimulatedPosition {
    fn default() -> Self {
        Self {
            side: TargetSide::Flat,
            quantity: 0.0,
            avg_price: 0.0,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SimulatedFill {
    pub side: TargetSide,
    pub quantity: f64,
    pub price: f64,
    pub notional_usd: f64,
    pub realized_pnl_delta: f64,
    pub leg_kind: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SimulatedIncident {
    pub severity: String,
    pub kind: String,
    pub title: String,
    pub detail: String,
}

#[derive(Debug, Clone)]
pub struct ReconciliationInput {
    pub desired_position_id: Uuid,
    pub proof_id: Uuid,
    pub sleeve_id: Uuid,
    pub symbol: String,
    pub environment_scope: String,
    pub target_side: TargetSide,
    pub target_notional_usd: f64,
    pub market_price: f64,
    pub current_position: SimulatedPosition,
    pub now: DateTime<Utc>,
    pub config: SimulationConfig,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReconciliationOutcome {
    pub status: String,
    pub idempotency_key: String,
    pub target_snapshot: Value,
    pub broker_snapshot: Value,
    pub delta_snapshot: Value,
    pub order_plan: Value,
    pub blocked_reasons: Vec<String>,
    pub fills: Vec<SimulatedFill>,
    pub final_position: SimulatedPosition,
    pub realized_pnl_delta: f64,
    pub unrealized_pnl: f64,
    pub incident: Option<SimulatedIncident>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SimulatedReconciliationReceipt {
    pub reconciliation_id: Uuid,
    pub status: String,
    pub duplicate: bool,
    pub fills: usize,
    pub incident: bool,
}

pub fn reconcile_simulated(input: &ReconciliationInput) -> ReconciliationOutcome {
    let idempotency_key = format!("sim:{}:{}", input.desired_position_id, input.proof_id);
    let target_notional_usd = input.target_notional_usd.max(0.0);
    let market_price = input.market_price.max(0.0);
    let current = normalize_position(input.current_position);
    let target_quantity = if input.target_side == TargetSide::Flat || market_price <= EPS {
        0.0
    } else {
        target_notional_usd / market_price
    };

    let target_snapshot = json!({
        "desired_position_id": input.desired_position_id,
        "proof_id": input.proof_id,
        "sleeve_id": input.sleeve_id,
        "symbol": input.symbol,
        "environment_scope": input.environment_scope,
        "target_side": input.target_side,
        "target_notional_usd": target_notional_usd,
        "target_quantity": target_quantity,
        "market_price": market_price,
    });
    let broker_snapshot = json!({
        "broker": "simulator",
        "account": "digital",
        "side": current.side,
        "quantity": current.quantity,
        "avg_price": current.avg_price,
        "notional_usd": current.quantity * market_price,
        "last_seen_at": input.config.broker_last_seen_at,
        "max_snapshot_age_secs": input.config.max_snapshot_age_secs,
    });

    if market_price <= EPS {
        return blocked_outcome(
            input,
            idempotency_key,
            target_snapshot,
            broker_snapshot,
            "missing_latest_price",
        );
    }
    if let Some(incident) = simulated_incident(input) {
        return incident_outcome(
            input,
            idempotency_key,
            target_snapshot,
            broker_snapshot,
            incident,
        );
    }

    let legs = build_legs(current, input.target_side, target_quantity);
    if legs.is_empty() {
        return ReconciliationOutcome {
            status: "noop".to_string(),
            idempotency_key,
            target_snapshot,
            broker_snapshot,
            delta_snapshot: json!({
                "action": "noop",
                "quantity_delta": 0.0,
                "notional_delta_usd": 0.0,
            }),
            order_plan: json!({
                "broker": "simulator",
                "account": "digital",
                "orders": [],
            }),
            blocked_reasons: Vec::new(),
            fills: Vec::new(),
            final_position: current,
            realized_pnl_delta: 0.0,
            unrealized_pnl: unrealized_pnl(current, market_price),
            incident: None,
        };
    }

    let fill_fraction = input.config.fill_fraction.clamp(0.0, 1.0);
    let mut position = current;
    let mut fills = Vec::new();
    let mut orders = Vec::new();
    let mut realized_pnl_delta = 0.0;
    let mut any_partial = false;

    for (idx, leg) in legs.into_iter().enumerate() {
        let fill_quantity = leg.quantity * fill_fraction;
        any_partial |= fill_quantity + EPS < leg.quantity;
        let fill = if fill_quantity > EPS {
            let fill = apply_leg(&mut position, &leg, fill_quantity, market_price);
            realized_pnl_delta += fill.realized_pnl_delta;
            Some(fill)
        } else {
            None
        };
        if let Some(fill) = fill {
            fills.push(fill);
        }
        orders.push(order_json(input, &leg, fill_quantity, idx));
    }

    let mut blocked_reasons = Vec::new();
    let status = if any_partial {
        blocked_reasons.push("partial_fill_open".to_string());
        "submitted"
    } else {
        "reconciled"
    };

    ReconciliationOutcome {
        status: status.to_string(),
        idempotency_key,
        target_snapshot,
        broker_snapshot,
        delta_snapshot: json!({
            "target_side": input.target_side,
            "target_quantity": target_quantity,
            "current_side": current.side,
            "current_quantity": current.quantity,
            "final_side": position.side,
            "final_quantity": position.quantity,
            "quantity_delta": signed_quantity(position.side, position.quantity)
                - signed_quantity(current.side, current.quantity),
            "notional_delta_usd": (signed_quantity(position.side, position.quantity)
                - signed_quantity(current.side, current.quantity)) * market_price,
            "realized_pnl_delta": realized_pnl_delta,
        }),
        order_plan: json!({
            "broker": "simulator",
            "account": "digital",
            "orders": orders,
        }),
        blocked_reasons,
        fills,
        final_position: position,
        realized_pnl_delta,
        unrealized_pnl: unrealized_pnl(position, market_price),
        incident: None,
    }
}

#[derive(Debug, Clone, Copy)]
struct OrderLeg {
    side: TargetSide,
    quantity: f64,
    kind: &'static str,
}

fn normalize_position(position: SimulatedPosition) -> SimulatedPosition {
    if position.side == TargetSide::Flat || position.quantity <= EPS || position.avg_price <= EPS {
        SimulatedPosition::default()
    } else {
        SimulatedPosition {
            side: position.side,
            quantity: position.quantity.max(0.0),
            avg_price: position.avg_price.max(0.0),
        }
    }
}

fn build_legs(
    current: SimulatedPosition,
    target_side: TargetSide,
    target_quantity: f64,
) -> Vec<OrderLeg> {
    let target_quantity = target_quantity.max(0.0);
    if target_side == TargetSide::Flat {
        return if current.side == TargetSide::Flat {
            Vec::new()
        } else {
            vec![OrderLeg {
                side: current.side,
                quantity: current.quantity,
                kind: "exit",
            }]
        };
    }

    if current.side == TargetSide::Flat {
        return vec![OrderLeg {
            side: target_side,
            quantity: target_quantity,
            kind: "enter",
        }];
    }

    if current.side == target_side {
        if (current.quantity - target_quantity).abs() <= EPS {
            Vec::new()
        } else if target_quantity > current.quantity {
            vec![OrderLeg {
                side: target_side,
                quantity: target_quantity - current.quantity,
                kind: "increase",
            }]
        } else {
            vec![OrderLeg {
                side: target_side,
                quantity: current.quantity - target_quantity,
                kind: "reduce",
            }]
        }
    } else {
        vec![
            OrderLeg {
                side: current.side,
                quantity: current.quantity,
                kind: "exit",
            },
            OrderLeg {
                side: target_side,
                quantity: target_quantity,
                kind: "enter",
            },
        ]
    }
}

fn apply_leg(
    position: &mut SimulatedPosition,
    leg: &OrderLeg,
    fill_quantity: f64,
    market_price: f64,
) -> SimulatedFill {
    let fill_quantity = fill_quantity.max(0.0);
    let mut realized_pnl_delta = 0.0;
    match leg.kind {
        "enter" => {
            *position = SimulatedPosition {
                side: leg.side,
                quantity: fill_quantity,
                avg_price: market_price,
            };
        }
        "increase" if position.side == leg.side => {
            let old_qty = position.quantity;
            let new_qty = old_qty + fill_quantity;
            let avg_price = if new_qty <= EPS {
                0.0
            } else {
                ((old_qty * position.avg_price) + (fill_quantity * market_price)) / new_qty
            };
            position.quantity = new_qty;
            position.avg_price = avg_price;
        }
        "reduce" | "exit" if position.side == leg.side => {
            let closed_qty = fill_quantity.min(position.quantity);
            realized_pnl_delta =
                realized_pnl(position.side, closed_qty, position.avg_price, market_price);
            let remaining = (position.quantity - closed_qty).max(0.0);
            if remaining <= EPS {
                *position = SimulatedPosition::default();
            } else {
                position.quantity = remaining;
            }
        }
        _ => {}
    }

    SimulatedFill {
        side: leg.side,
        quantity: fill_quantity,
        price: market_price,
        notional_usd: fill_quantity * market_price,
        realized_pnl_delta,
        leg_kind: leg.kind.to_string(),
    }
}

fn order_json(
    input: &ReconciliationInput,
    leg: &OrderLeg,
    filled_quantity: f64,
    idx: usize,
) -> Value {
    json!({
        "client_order_id": format!("{}:{}", input.desired_position_id, idx),
        "type": "market",
        "action": order_action(leg),
        "position_side": leg.side,
        "leg_kind": leg.kind,
        "quantity": leg.quantity,
        "filled_quantity": filled_quantity,
        "price": input.market_price,
        "status": if filled_quantity + EPS >= leg.quantity { "filled" } else { "partially_filled" },
        "bracket": bracket_json(input, leg),
    })
}

fn order_action(leg: &OrderLeg) -> &'static str {
    match (leg.side, leg.kind) {
        (TargetSide::Long, "enter" | "increase") => "buy",
        (TargetSide::Long, "reduce" | "exit") => "sell",
        (TargetSide::Short, "enter" | "increase") => "sell_short",
        (TargetSide::Short, "reduce" | "exit") => "buy_to_cover",
        _ => "none",
    }
}

fn bracket_json(input: &ReconciliationInput, leg: &OrderLeg) -> Value {
    if !matches!(leg.kind, "enter" | "increase") {
        return Value::Null;
    }
    let stop_pct = input.config.bracket_stop_loss_pct.max(0.0);
    let target_pct = input.config.bracket_take_profit_pct.max(0.0);
    let price = input.market_price;
    let (stop_price, take_profit_price) = match leg.side {
        TargetSide::Long => (price * (1.0 - stop_pct), price * (1.0 + target_pct)),
        TargetSide::Short => (price * (1.0 + stop_pct), price * (1.0 - target_pct)),
        TargetSide::Flat => (price, price),
    };
    json!({
        "stop_price": round_cents(stop_price),
        "take_profit_price": round_cents(take_profit_price),
    })
}

fn simulated_incident(input: &ReconciliationInput) -> Option<SimulatedIncident> {
    if let Some(seen_at) = input.config.broker_last_seen_at {
        if input.now.signed_duration_since(seen_at).num_seconds()
            > input.config.max_snapshot_age_secs.max(0)
        {
            return Some(SimulatedIncident {
                severity: "warning".to_string(),
                kind: "stale_broker_data".to_string(),
                title: "Simulated broker snapshot is stale".to_string(),
                detail: format!(
                    "Latest simulated broker snapshot for {} is older than {} seconds.",
                    input.symbol, input.config.max_snapshot_age_secs
                ),
            });
        }
    }
    match input.config.fault {
        SimulatedFault::None => None,
        SimulatedFault::StaleSnapshot => Some(SimulatedIncident {
            severity: "warning".to_string(),
            kind: "stale_broker_data".to_string(),
            title: "Simulated broker snapshot is stale".to_string(),
            detail: format!(
                "Digital broker was configured with stale state for {}.",
                input.symbol
            ),
        }),
        SimulatedFault::Reject => {
            let repeated = input.config.prior_reject_count > 0;
            Some(SimulatedIncident {
                severity: if repeated { "critical" } else { "warning" }.to_string(),
                kind: if repeated {
                    "repeated_order_reject"
                } else {
                    "order_rejected"
                }
                .to_string(),
                title: if repeated {
                    "Repeated simulated order reject"
                } else {
                    "Simulated order rejected"
                }
                .to_string(),
                detail: format!(
                    "Digital broker rejected the simulated order for {}.",
                    input.symbol
                ),
            })
        }
        SimulatedFault::Disconnect => Some(SimulatedIncident {
            severity: "critical".to_string(),
            kind: "broker_disconnect".to_string(),
            title: "Simulated broker disconnected".to_string(),
            detail: format!("Digital broker is disconnected for {}.", input.symbol),
        }),
        SimulatedFault::UnknownOrderStatus => Some(SimulatedIncident {
            severity: "warning".to_string(),
            kind: "unknown_order_status".to_string(),
            title: "Simulated order status unknown".to_string(),
            detail: format!(
                "Digital broker returned an unknown order status for {}.",
                input.symbol
            ),
        }),
    }
}

fn incident_outcome(
    input: &ReconciliationInput,
    idempotency_key: String,
    target_snapshot: Value,
    broker_snapshot: Value,
    incident: SimulatedIncident,
) -> ReconciliationOutcome {
    ReconciliationOutcome {
        status: "incident".to_string(),
        idempotency_key,
        target_snapshot,
        broker_snapshot,
        delta_snapshot: json!({ "incident_kind": incident.kind }),
        order_plan: json!({
            "broker": "simulator",
            "account": "digital",
            "orders": [],
        }),
        blocked_reasons: vec![incident.kind.clone()],
        fills: Vec::new(),
        final_position: normalize_position(input.current_position),
        realized_pnl_delta: 0.0,
        unrealized_pnl: unrealized_pnl(
            normalize_position(input.current_position),
            input.market_price,
        ),
        incident: Some(incident),
    }
}

fn blocked_outcome(
    input: &ReconciliationInput,
    idempotency_key: String,
    target_snapshot: Value,
    broker_snapshot: Value,
    reason: &str,
) -> ReconciliationOutcome {
    ReconciliationOutcome {
        status: "blocked".to_string(),
        idempotency_key,
        target_snapshot,
        broker_snapshot,
        delta_snapshot: json!({ "blocked_reason": reason }),
        order_plan: json!({
            "broker": "simulator",
            "account": "digital",
            "orders": [],
        }),
        blocked_reasons: vec![reason.to_string()],
        fills: Vec::new(),
        final_position: normalize_position(input.current_position),
        realized_pnl_delta: 0.0,
        unrealized_pnl: 0.0,
        incident: None,
    }
}

fn realized_pnl(side: TargetSide, quantity: f64, avg_price: f64, fill_price: f64) -> f64 {
    match side {
        TargetSide::Long => (fill_price - avg_price) * quantity,
        TargetSide::Short => (avg_price - fill_price) * quantity,
        TargetSide::Flat => 0.0,
    }
}

fn unrealized_pnl(position: SimulatedPosition, market_price: f64) -> f64 {
    match position.side {
        TargetSide::Long => (market_price - position.avg_price) * position.quantity,
        TargetSide::Short => (position.avg_price - market_price) * position.quantity,
        TargetSide::Flat => 0.0,
    }
}

fn signed_quantity(side: TargetSide, quantity: f64) -> f64 {
    match side {
        TargetSide::Long => quantity,
        TargetSide::Short => -quantity,
        TargetSide::Flat => 0.0,
    }
}

fn round_cents(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use pretty_assertions::assert_eq;

    fn input(
        target_side: TargetSide,
        target_notional_usd: f64,
        market_price: f64,
        current_position: SimulatedPosition,
    ) -> ReconciliationInput {
        ReconciliationInput {
            desired_position_id: Uuid::new_v4(),
            proof_id: Uuid::new_v4(),
            sleeve_id: Uuid::new_v4(),
            symbol: "NVDA".to_string(),
            environment_scope: "shadow".to_string(),
            target_side,
            target_notional_usd,
            market_price,
            current_position,
            now: Utc::now(),
            config: SimulationConfig::default(),
        }
    }

    fn long(quantity: f64, avg_price: f64) -> SimulatedPosition {
        SimulatedPosition {
            side: TargetSide::Long,
            quantity,
            avg_price,
        }
    }

    fn short(quantity: f64, avg_price: f64) -> SimulatedPosition {
        SimulatedPosition {
            side: TargetSide::Short,
            quantity,
            avg_price,
        }
    }

    #[test]
    fn no_op_when_target_matches_current_position() {
        let outcome =
            reconcile_simulated(&input(TargetSide::Long, 1_000.0, 100.0, long(10.0, 100.0)));

        assert_eq!(outcome.status, "noop");
        assert!(outcome.fills.is_empty());
        assert_eq!(outcome.final_position.side, TargetSide::Long);
        assert_eq!(outcome.final_position.quantity, 10.0);
    }

    #[test]
    fn enter_creates_market_fill_and_bracket_plan() {
        let outcome = reconcile_simulated(&input(
            TargetSide::Long,
            1_000.0,
            100.0,
            SimulatedPosition::default(),
        ));

        assert_eq!(outcome.status, "reconciled");
        assert_eq!(outcome.fills.len(), 1);
        assert_eq!(outcome.fills[0].quantity, 10.0);
        assert_eq!(outcome.final_position, long(10.0, 100.0));
        assert_eq!(
            outcome.order_plan["orders"][0]["bracket"]["stop_price"],
            92.0
        );
        assert_eq!(
            outcome.order_plan["orders"][0]["bracket"]["take_profit_price"],
            116.0
        );
    }

    #[test]
    fn reduce_realizes_pnl_and_keeps_remaining_position() {
        let outcome =
            reconcile_simulated(&input(TargetSide::Long, 600.0, 120.0, long(10.0, 100.0)));

        assert_eq!(outcome.status, "reconciled");
        assert_eq!(outcome.fills[0].leg_kind, "reduce");
        assert_eq!(outcome.fills[0].quantity, 5.0);
        assert_eq!(outcome.realized_pnl_delta, 100.0);
        assert_eq!(outcome.final_position, long(5.0, 100.0));
    }

    #[test]
    fn exit_closes_position_and_realizes_pnl() {
        let outcome = reconcile_simulated(&input(TargetSide::Flat, 0.0, 90.0, long(10.0, 100.0)));

        assert_eq!(outcome.status, "reconciled");
        assert_eq!(outcome.fills[0].leg_kind, "exit");
        assert_eq!(outcome.realized_pnl_delta, -100.0);
        assert_eq!(outcome.final_position.side, TargetSide::Flat);
        assert_eq!(outcome.final_position.quantity, 0.0);
    }

    #[test]
    fn flip_exits_then_enters_opposite_side() {
        let outcome =
            reconcile_simulated(&input(TargetSide::Short, 800.0, 80.0, long(10.0, 100.0)));

        assert_eq!(outcome.status, "reconciled");
        assert_eq!(outcome.fills.len(), 2);
        assert_eq!(outcome.fills[0].leg_kind, "exit");
        assert_eq!(outcome.fills[1].leg_kind, "enter");
        assert_eq!(outcome.realized_pnl_delta, -200.0);
        assert_eq!(outcome.final_position, short(10.0, 80.0));
    }

    #[test]
    fn partial_fill_updates_sleeve_but_leaves_order_submitted() {
        let mut input = input(
            TargetSide::Long,
            1_000.0,
            100.0,
            SimulatedPosition::default(),
        );
        input.config.fill_fraction = 0.4;
        let outcome = reconcile_simulated(&input);

        assert_eq!(outcome.status, "submitted");
        assert_eq!(outcome.fills[0].quantity, 4.0);
        assert_eq!(outcome.final_position, long(4.0, 100.0));
        assert!(
            outcome
                .blocked_reasons
                .iter()
                .any(|reason| reason == "partial_fill_open")
        );
    }

    #[test]
    fn reject_and_disconnect_raise_incidents() {
        let mut rejected = input(
            TargetSide::Long,
            1_000.0,
            100.0,
            SimulatedPosition::default(),
        );
        rejected.config.fault = SimulatedFault::Reject;
        rejected.config.prior_reject_count = 1;
        let rejected = reconcile_simulated(&rejected);
        assert_eq!(rejected.status, "incident");
        assert_eq!(
            rejected.incident.as_ref().map(|i| i.kind.as_str()),
            Some("repeated_order_reject")
        );

        let mut disconnected = input(
            TargetSide::Long,
            1_000.0,
            100.0,
            SimulatedPosition::default(),
        );
        disconnected.config.fault = SimulatedFault::Disconnect;
        let disconnected = reconcile_simulated(&disconnected);
        assert_eq!(disconnected.status, "incident");
        assert_eq!(
            disconnected.incident.as_ref().map(|i| i.kind.as_str()),
            Some("broker_disconnect")
        );
    }

    #[test]
    fn stale_and_unknown_broker_state_raise_incidents() {
        let mut stale = input(
            TargetSide::Long,
            1_000.0,
            100.0,
            SimulatedPosition::default(),
        );
        stale.config.broker_last_seen_at = Some(stale.now - Duration::seconds(600));
        let stale = reconcile_simulated(&stale);
        assert_eq!(stale.status, "incident");
        assert_eq!(
            stale.incident.as_ref().map(|i| i.kind.as_str()),
            Some("stale_broker_data")
        );

        let mut unknown = input(
            TargetSide::Long,
            1_000.0,
            100.0,
            SimulatedPosition::default(),
        );
        unknown.config.fault = SimulatedFault::UnknownOrderStatus;
        let unknown = reconcile_simulated(&unknown);
        assert_eq!(unknown.status, "incident");
        assert_eq!(
            unknown.incident.as_ref().map(|i| i.kind.as_str()),
            Some("unknown_order_status")
        );
    }

    #[test]
    fn short_bracket_prices_invert_stop_and_target() {
        let outcome = reconcile_simulated(&input(
            TargetSide::Short,
            1_000.0,
            100.0,
            SimulatedPosition::default(),
        ));

        assert_eq!(outcome.status, "reconciled");
        assert_eq!(
            outcome.order_plan["orders"][0]["bracket"]["stop_price"],
            108.0
        );
        assert_eq!(
            outcome.order_plan["orders"][0]["bracket"]["take_profit_price"],
            84.0
        );
    }
}
