use serde::Serialize;
use serde_json::{Value, json};
use uuid::Uuid;

#[derive(Debug, Clone, Default)]
pub struct AllocationLimits {
    pub max_strategy_allocation_pct: Option<f64>,
    pub max_symbol_allocation_pct: Option<f64>,
    pub max_portfolio_allocation_pct: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct AllocationRequest {
    pub sleeve_id: Option<Uuid>,
    pub symbol: String,
    pub target_notional_usd: f64,
    pub portfolio_value_usd: f64,
}

#[derive(Debug, Clone)]
pub struct SleeveAllocation {
    pub sleeve_id: Option<Uuid>,
    pub symbol: String,
    pub sleeve_kind: String,
    pub status: String,
    pub allocated_notional_usd: f64,
    pub current_notional_usd: f64,
    pub realized_pnl: f64,
    pub unrealized_pnl: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct AllocationDecision {
    pub allowed: bool,
    pub blocked_reasons: Vec<String>,
    pub snapshot: Value,
}

pub fn evaluate_allocation(
    request: &AllocationRequest,
    sleeves: &[SleeveAllocation],
    limits: &AllocationLimits,
) -> AllocationDecision {
    let target = request.target_notional_usd.max(0.0);
    let portfolio_value = request.portfolio_value_usd.max(0.0);
    let current_self = sleeves
        .iter()
        .find(|sleeve| same_sleeve(request.sleeve_id, sleeve.sleeve_id))
        .map(sleeve_notional)
        .unwrap_or(0.0);
    let current_symbol_total: f64 = sleeves
        .iter()
        .filter(|sleeve| sleeve.symbol.eq_ignore_ascii_case(&request.symbol))
        .map(sleeve_notional)
        .sum();
    let current_portfolio_total: f64 = sleeves.iter().map(sleeve_notional).sum();
    let proposed_symbol_total = (current_symbol_total - current_self).max(0.0) + target;
    let proposed_portfolio_total = (current_portfolio_total - current_self).max(0.0) + target;
    let is_reduction = target <= current_self + 0.000_001;

    let mut blocked_reasons = Vec::new();
    if let Some(existing) = sleeves
        .iter()
        .find(|sleeve| same_sleeve(request.sleeve_id, sleeve.sleeve_id))
    {
        match existing.status.as_str() {
            "frozen" if !is_reduction => blocked_reasons.push("sleeve_frozen".to_string()),
            "closed" if target > 0.000_001 => blocked_reasons.push("sleeve_closed".to_string()),
            _ => {}
        }
    }

    let strategy_cap_usd = cap_usd(limits.max_strategy_allocation_pct, portfolio_value);
    if cap_exceeded(target, current_self, strategy_cap_usd) {
        blocked_reasons.push("strategy_allocation_cap_exceeded".to_string());
    }

    let symbol_cap_usd = cap_usd(limits.max_symbol_allocation_pct, portfolio_value);
    if cap_exceeded(proposed_symbol_total, current_symbol_total, symbol_cap_usd) {
        blocked_reasons.push("symbol_allocation_cap_exceeded".to_string());
    }

    let portfolio_cap_usd = cap_usd(limits.max_portfolio_allocation_pct, portfolio_value);
    if cap_exceeded(
        proposed_portfolio_total,
        current_portfolio_total,
        portfolio_cap_usd,
    ) {
        blocked_reasons.push("portfolio_allocation_cap_exceeded".to_string());
    }

    blocked_reasons.sort();
    blocked_reasons.dedup();

    AllocationDecision {
        allowed: blocked_reasons.is_empty(),
        blocked_reasons,
        snapshot: json!({
            "request": {
                "sleeve_id": request.sleeve_id,
                "symbol": request.symbol,
                "target_notional_usd": target,
                "portfolio_value_usd": portfolio_value,
                "is_reduction": is_reduction,
            },
            "current": {
                "sleeve_notional_usd": current_self,
                "symbol_notional_usd": current_symbol_total,
                "portfolio_notional_usd": current_portfolio_total,
            },
            "proposed": {
                "symbol_notional_usd": proposed_symbol_total,
                "portfolio_notional_usd": proposed_portfolio_total,
            },
            "limits": {
                "max_strategy_allocation_pct": limits.max_strategy_allocation_pct,
                "max_strategy_notional_usd": strategy_cap_usd,
                "max_symbol_allocation_pct": limits.max_symbol_allocation_pct,
                "max_symbol_notional_usd": symbol_cap_usd,
                "max_portfolio_allocation_pct": limits.max_portfolio_allocation_pct,
                "max_portfolio_notional_usd": portfolio_cap_usd,
            },
            "sleeves": sleeves.iter().map(|sleeve| {
                json!({
                    "sleeve_id": sleeve.sleeve_id,
                    "symbol": sleeve.symbol,
                    "sleeve_kind": sleeve.sleeve_kind,
                    "status": sleeve.status,
                    "allocated_notional_usd": sleeve.allocated_notional_usd,
                    "current_notional_usd": sleeve.current_notional_usd,
                    "realized_pnl": sleeve.realized_pnl,
                    "unrealized_pnl": sleeve.unrealized_pnl,
                })
            }).collect::<Vec<_>>(),
        }),
    }
}

fn same_sleeve(left: Option<Uuid>, right: Option<Uuid>) -> bool {
    left.is_some() && left == right
}

fn sleeve_notional(sleeve: &SleeveAllocation) -> f64 {
    sleeve
        .allocated_notional_usd
        .max(sleeve.current_notional_usd)
        .max(0.0)
}

fn cap_usd(pct: Option<f64>, portfolio_value: f64) -> Option<f64> {
    pct.map(|pct| (pct.max(0.0) * portfolio_value).max(0.0))
}

fn cap_exceeded(proposed: f64, current: f64, cap: Option<f64>) -> bool {
    let Some(cap) = cap else {
        return false;
    };
    proposed > cap + 0.000_001 && proposed > current + 0.000_001
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> AllocationLimits {
        AllocationLimits {
            max_strategy_allocation_pct: Some(0.10),
            max_symbol_allocation_pct: Some(0.10),
            max_portfolio_allocation_pct: Some(0.90),
        }
    }

    fn request(sleeve_id: Option<Uuid>, symbol: &str, target: f64) -> AllocationRequest {
        AllocationRequest {
            sleeve_id,
            symbol: symbol.to_string(),
            target_notional_usd: target,
            portfolio_value_usd: 100_000.0,
        }
    }

    fn sleeve(
        sleeve_id: Option<Uuid>,
        symbol: &str,
        status: &str,
        allocated: f64,
    ) -> SleeveAllocation {
        SleeveAllocation {
            sleeve_id,
            symbol: symbol.to_string(),
            sleeve_kind: "strategy".to_string(),
            status: status.to_string(),
            allocated_notional_usd: allocated,
            current_notional_usd: allocated,
            realized_pnl: 0.0,
            unrealized_pnl: 0.0,
        }
    }

    #[test]
    fn shared_symbol_allocations_count_against_symbol_cap() {
        let decision = evaluate_allocation(
            &request(None, "NVDA", 3_000.0),
            &[
                sleeve(Some(Uuid::new_v4()), "NVDA", "active", 4_000.0),
                sleeve(Some(Uuid::new_v4()), "NVDA", "active", 4_000.0),
            ],
            &limits(),
        );

        assert!(!decision.allowed);
        assert!(
            decision
                .blocked_reasons
                .iter()
                .any(|reason| reason == "symbol_allocation_cap_exceeded")
        );
    }

    #[test]
    fn own_current_allocation_is_replaced_for_resize() {
        let sleeve_id = Uuid::new_v4();
        let decision = evaluate_allocation(
            &request(Some(sleeve_id), "NVDA", 7_000.0),
            &[
                sleeve(Some(sleeve_id), "NVDA", "active", 5_000.0),
                sleeve(Some(Uuid::new_v4()), "NVDA", "active", 2_000.0),
            ],
            &limits(),
        );

        assert!(decision.allowed, "{decision:?}");
    }

    #[test]
    fn partial_exit_is_allowed_even_when_current_allocation_is_over_cap() {
        let sleeve_id = Uuid::new_v4();
        let decision = evaluate_allocation(
            &request(Some(sleeve_id), "NVDA", 5_000.0),
            &[sleeve(Some(sleeve_id), "NVDA", "active", 15_000.0)],
            &limits(),
        );

        assert!(decision.allowed, "{decision:?}");
    }

    #[test]
    fn frozen_sleeve_blocks_increasing_allocation() {
        let sleeve_id = Uuid::new_v4();
        let decision = evaluate_allocation(
            &request(Some(sleeve_id), "NVDA", 6_000.0),
            &[sleeve(Some(sleeve_id), "NVDA", "frozen", 5_000.0)],
            &limits(),
        );

        assert!(!decision.allowed);
        assert!(
            decision
                .blocked_reasons
                .iter()
                .any(|reason| reason == "sleeve_frozen")
        );
    }

    #[test]
    fn portfolio_cap_blocks_total_allocation() {
        let decision = evaluate_allocation(
            &request(None, "AVGO", 30_000.0),
            &[sleeve(Some(Uuid::new_v4()), "NVDA", "active", 80_000.0)],
            &limits(),
        );

        assert!(!decision.allowed);
        assert!(
            decision
                .blocked_reasons
                .iter()
                .any(|reason| reason == "portfolio_allocation_cap_exceeded")
        );
    }
}
