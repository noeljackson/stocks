//! Deterministic risk overlay (SPEC §7).
//!
//! [`evaluate`] is pure — no I/O, no clock. [`run`] wires it into a JetStream
//! durable consumer on THESIS/thesis.actionable that loads positions from
//! Postgres + the active risk config and publishes risk.veto / risk.warning.
//!
//! Limit semantics (config name='risk' v1):
//! - `single_name_delta_notional_pct`   HARD veto
//! - `options_premium_aggregate_pct`    HARD veto
//! - `cash_floor_pct`                   HARD veto (current + post-entry)
//! - `drawdown_brake`                   tiered: warn+scale → halt
//! - `subsector_cluster_pct`            SOFT (concentrated-specialist edge)
//! - `concurrent_positions`             HARD veto at cap

mod service;
#[cfg(test)]
mod tests;

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

pub use service::run;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub single_name_delta_notional_pct: f64,
    #[serde(default)]
    pub options_premium_aggregate_pct: f64,
    #[serde(default)]
    pub cash_floor_pct: f64,
    #[serde(default)]
    pub drawdown_brake: Vec<DrawdownBrake>,
    #[serde(default)]
    pub subsector_cluster_pct: f64,
    #[serde(default)]
    pub concurrent_positions: u32,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub struct DrawdownBrake {
    pub at_pct: f64,
    #[serde(default)]
    pub size_mult: f64,
    #[serde(default)]
    pub halt_new: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Position {
    pub symbol: String,
    pub cluster: String,
    pub instrument: String,
    pub delta_notional: f64,
    pub premium_at_risk: f64,
}

#[derive(Debug, Clone, Default)]
pub struct Intent {
    pub symbol: String,
    pub cluster: String,
    pub instrument: String,
    pub delta_notional: f64,
    pub premium_at_risk: f64,
}

#[derive(Debug, Clone, Copy)]
pub struct Portfolio {
    pub total_value: f64,
    pub cash_pct: f64,
    pub drawdown_pct: f64,
}

/// Operator-set frame for the portfolio (#26). When `account_size_usd` is
/// `None`, the system is unconfigured and the risk overlay falls back to a
/// demo portfolio with a loud warning rather than crashing.
#[derive(Debug, Clone, Copy, Default)]
pub struct PortfolioSettings {
    pub account_size_usd: Option<f64>,
    pub high_water_mark_usd: Option<f64>,
}

/// Pure derivation: given operator settings, open positions, and the realized
/// PnL summed over closed positions, return the [`Portfolio`] the risk
/// overlay should evaluate against. Returns `None` if the system is
/// unconfigured (account_size_usd unset) — caller decides the fallback.
///
/// - `cash_pct = max(0, account_size - sum(open delta + premium)) / account_size`
/// - `drawdown_pct` is negative (or zero). With realized PnL only it captures
///   the closed-book draw; once IBKR is wired (#25) we'll fold unrealized in.
///   Anchored at `high_water_mark_usd` if set; otherwise the account size.
#[must_use]
pub fn derive_portfolio(
    settings: PortfolioSettings,
    open: &[Position],
    realized_pnl: f64,
) -> Option<Portfolio> {
    let total = settings.account_size_usd?;
    if total <= 0.0 {
        return None;
    }
    let consumed: f64 = open
        .iter()
        .map(|p| p.delta_notional + p.premium_at_risk)
        .sum();
    let cash = (total - consumed).max(0.0);
    let cash_pct = 100.0 * cash / total;

    let anchor = settings.high_water_mark_usd.unwrap_or(total).max(total);
    let current_equity = total + realized_pnl;
    let drawdown_pct = if current_equity >= anchor {
        0.0
    } else {
        100.0 * (current_equity - anchor) / anchor
    };

    Some(Portfolio { total_value: total, cash_pct, drawdown_pct })
}

#[derive(Debug, Clone, Serialize)]
pub struct Decision {
    pub veto: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub reasons: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    pub size_mult: f64,
}

impl Default for Decision {
    fn default() -> Self {
        Self {
            veto: false,
            reasons: Vec::new(),
            warnings: Vec::new(),
            size_mult: 1.0,
        }
    }
}

/// Returns whether the proposed intent passes the risk overlay.
#[must_use]
pub fn evaluate(intent: &Intent, positions: &[Position], port: Portfolio, cfg: &Config) -> Decision {
    let mut d = Decision::default();

    let pct = |v: f64| -> f64 {
        if port.total_value == 0.0 {
            0.0
        } else {
            100.0 * v / port.total_value
        }
    };

    // --- single-name delta-notional (HARD) ---
    if cfg.single_name_delta_notional_pct > 0.0 {
        let existing: f64 = positions
            .iter()
            .filter(|p| p.symbol == intent.symbol)
            .map(|p| p.delta_notional)
            .sum();
        if pct(existing + intent.delta_notional) > cfg.single_name_delta_notional_pct {
            d.veto = true;
            d.reasons.push("single_name_delta_notional_pct".into());
        }
    }

    // --- options premium aggregate (HARD) ---
    if cfg.options_premium_aggregate_pct > 0.0 {
        let agg: f64 = positions.iter().map(|p| p.premium_at_risk).sum();
        if pct(agg + intent.premium_at_risk) > cfg.options_premium_aggregate_pct {
            d.veto = true;
            d.reasons.push("options_premium_aggregate_pct".into());
        }
    }

    // --- cash floor (HARD, current + post-entry) ---
    if cfg.cash_floor_pct > 0.0 {
        let consumed = intent.delta_notional + intent.premium_at_risk;
        let post_cash = port.cash_pct - pct(consumed);
        if port.cash_pct < cfg.cash_floor_pct || post_cash < cfg.cash_floor_pct {
            d.veto = true;
            d.reasons.push("cash_floor_pct".into());
        }
    }

    // --- drawdown brake (tiered) ---
    for b in &cfg.drawdown_brake {
        if port.drawdown_pct <= b.at_pct {
            if b.halt_new {
                d.veto = true;
                d.reasons.push("drawdown_brake_halt".into());
            }
            if b.size_mult > 0.0 && b.size_mult < d.size_mult {
                d.size_mult = b.size_mult;
                d.warnings
                    .push(format!("drawdown brake: size scaled to {}x", b.size_mult));
            }
        }
    }

    // --- sub-sector cluster (SOFT — warn only) ---
    if cfg.subsector_cluster_pct > 0.0 && !intent.cluster.is_empty() {
        let mut total = intent.delta_notional;
        for p in positions {
            if p.cluster == intent.cluster {
                total += p.delta_notional;
            }
        }
        if pct(total) > cfg.subsector_cluster_pct {
            d.warnings.push(format!(
                "sub-sector {} exposure exceeds {}%",
                intent.cluster, cfg.subsector_cluster_pct
            ));
        }
    }

    // --- concurrent positions (HARD; only new symbols count toward the cap) ---
    if cfg.concurrent_positions > 0 {
        let existing: HashSet<&str> = positions.iter().map(|p| p.symbol.as_str()).collect();
        if !existing.contains(intent.symbol.as_str())
            && u32::try_from(existing.len()).unwrap_or(u32::MAX) >= cfg.concurrent_positions
        {
            d.veto = true;
            d.reasons.push("concurrent_positions".into());
        }
    }

    d
}
