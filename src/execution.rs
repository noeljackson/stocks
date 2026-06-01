//! Execution helpers for manual fills and position accounting.
//!
//! This module is deliberately pure: HTTP handlers and broker bridges can use
//! the same exposure/PnL rules without mixing database I/O into the math.

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FillExposure {
    pub delta_notional: f64,
    pub premium_at_risk: f64,
    pub multiplier: f64,
}

#[must_use]
pub fn allowed_side(side: &str) -> bool {
    matches!(side, "long" | "short" | "call" | "put" | "hedge")
}

#[must_use]
pub fn allowed_instrument(instrument: &str) -> bool {
    matches!(instrument, "equity" | "leaps" | "options")
}

#[must_use]
pub fn contract_multiplier(instrument: &str) -> f64 {
    if matches!(instrument, "leaps" | "options") {
        100.0
    } else {
        1.0
    }
}

#[must_use]
pub fn default_exposure(
    side: &str,
    instrument: &str,
    qty: f64,
    price: f64,
    delta_notional: Option<f64>,
    premium_at_risk: Option<f64>,
) -> FillExposure {
    let multiplier = contract_multiplier(instrument);
    let gross = qty.abs() * price.abs() * multiplier;
    let option_like =
        matches!(instrument, "leaps" | "options") || matches!(side, "call" | "put" | "hedge");

    FillExposure {
        delta_notional: delta_notional.unwrap_or(if option_like { 0.0 } else { gross }),
        premium_at_risk: premium_at_risk.unwrap_or(if option_like { gross } else { 0.0 }),
        multiplier,
    }
}

#[must_use]
pub fn realized_pnl(
    side: &str,
    qty: f64,
    avg_price: f64,
    exit_price: f64,
    fees: f64,
    multiplier: f64,
) -> f64 {
    let gross = if side == "short" {
        (avg_price - exit_price) * qty.abs() * multiplier
    } else {
        (exit_price - avg_price) * qty.abs() * multiplier
    };
    gross - fees.max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn equity_fill_defaults_to_delta_notional() {
        let exposure = default_exposure("long", "equity", 40.0, 25.0, None, None);
        assert_eq!(
            exposure,
            FillExposure {
                delta_notional: 1_000.0,
                premium_at_risk: 0.0,
                multiplier: 1.0,
            }
        );
    }

    #[test]
    fn option_fill_defaults_to_premium_at_risk_with_contract_multiplier() {
        let exposure = default_exposure("call", "options", 2.0, 12.5, None, None);
        assert_eq!(
            exposure,
            FillExposure {
                delta_notional: 0.0,
                premium_at_risk: 2_500.0,
                multiplier: 100.0,
            }
        );
    }

    #[test]
    fn explicit_exposure_overrides_defaults() {
        let exposure = default_exposure("call", "leaps", 1.0, 10.0, Some(750.0), Some(1_000.0));
        assert_eq!(
            exposure,
            FillExposure {
                delta_notional: 750.0,
                premium_at_risk: 1_000.0,
                multiplier: 100.0,
            }
        );
    }

    #[test]
    fn realized_pnl_handles_long_and_short_common() {
        assert_eq!(realized_pnl("long", 10.0, 100.0, 112.0, 2.0, 1.0), 118.0);
        assert_eq!(realized_pnl("short", 10.0, 100.0, 88.0, 2.0, 1.0), 118.0);
    }

    #[test]
    fn realized_pnl_uses_option_multiplier() {
        assert_eq!(realized_pnl("call", 1.0, 5.0, 8.0, 1.0, 100.0), 299.0);
    }
}
