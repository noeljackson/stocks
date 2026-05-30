// Package risk implements the deterministic risk overlay (SPEC §7).
//
// Evaluate is a pure function — no I/O, no clock. Service.go wires it into a
// JetStream durable consumer on THESIS/thesis.actionable that loads positions
// from Postgres + the active risk config and publishes risk.veto/risk.warning.
//
// Limit semantics (from config 'risk' v1):
//
//   - single_name_delta_notional_pct   HARD veto (concentrated-specialist
//     still needs a per-name cap)
//   - options_premium_aggregate_pct    HARD veto on total premium-at-risk
//   - cash_floor_pct                   HARD veto if entry would breach the floor
//   - drawdown_brake                   tiered: -10%→0.5x size (warn);
//     -20%→halt_new (veto)
//   - subsector_cluster_pct            SOFT — emits warning, does not veto
//     (tech-infra concentration IS the edge)
//   - concurrent_positions             HARD veto at limit
package risk

import "strconv"

// Config is the parsed body of config (name='risk', active=true).
type Config struct {
	SingleNameDeltaNotionalPct float64           `json:"single_name_delta_notional_pct"`
	OptionsPremiumAggregatePct float64           `json:"options_premium_aggregate_pct"`
	CashFloorPct               float64           `json:"cash_floor_pct"`
	DrawdownBrake              []DrawdownBrake   `json:"drawdown_brake"`
	SubsectorClusterPct        float64           `json:"subsector_cluster_pct"`
	ConcurrentPositions        int               `json:"concurrent_positions"`
}

type DrawdownBrake struct {
	AtPct    float64 `json:"at_pct"`     // threshold (negative number)
	SizeMult float64 `json:"size_mult"`  // 0 if absent → no scaling
	HaltNew  bool    `json:"halt_new"`
}

// Position is an open book item; the live IBKR integration is deferred, so
// for v0 this is hand-populated or computed from the `position` table.
type Position struct {
	Symbol         string
	Cluster        string  // ticker.cluster_id
	Instrument     string  // 'equity' | 'leaps'
	DeltaNotional  float64 // counts toward single-name + subsector caps
	PremiumAtRisk  float64 // counts toward options-aggregate cap
}

// Intent is what the actionable thesis proposes to do.
type Intent struct {
	Symbol         string
	Cluster        string
	Instrument     string  // 'equity' | 'leaps'
	DeltaNotional  float64 // for equity OR delta-equivalent of LEAPS
	PremiumAtRisk  float64 // for LEAPS only
}

// Portfolio is the snapshot the overlay needs to reason about percentages.
type Portfolio struct {
	TotalValue  float64 // denominator for *_pct ratios
	CashPct     float64 // current cash as % of portfolio
	DrawdownPct float64 // negative when underwater (e.g. -12)
}

// Decision is the verdict the overlay returns.
type Decision struct {
	Veto     bool
	Reasons  []string // rule names that triggered the veto (machine-parseable)
	Warnings []string // human-readable warnings (don't block; inform)
	SizeMult float64  // 1.0 by default; reduced by drawdown brake
}

// Evaluate returns whether the proposed intent passes the risk overlay.
// Reasons are stable rule names; Warnings are prose. SizeMult ≤ 1.0.
func Evaluate(intent Intent, positions []Position, port Portfolio, cfg Config) Decision {
	d := Decision{SizeMult: 1.0}

	pct := func(v float64) float64 {
		if port.TotalValue == 0 {
			return 0
		}
		return 100 * v / port.TotalValue
	}

	// --- single-name delta-notional ---
	if cfg.SingleNameDeltaNotionalPct > 0 {
		existing := 0.0
		for _, p := range positions {
			if p.Symbol == intent.Symbol {
				existing += p.DeltaNotional
			}
		}
		projected := pct(existing + intent.DeltaNotional)
		if projected > cfg.SingleNameDeltaNotionalPct {
			d.Veto = true
			d.Reasons = append(d.Reasons, "single_name_delta_notional_pct")
		}
	}

	// --- options premium aggregate ---
	if cfg.OptionsPremiumAggregatePct > 0 {
		agg := 0.0
		for _, p := range positions {
			agg += p.PremiumAtRisk
		}
		projected := pct(agg + intent.PremiumAtRisk)
		if projected > cfg.OptionsPremiumAggregatePct {
			d.Veto = true
			d.Reasons = append(d.Reasons, "options_premium_aggregate_pct")
		}
	}

	// --- cash floor (current AND post-entry) ---
	if cfg.CashFloorPct > 0 {
		// Entry consumes cash equal to delta_notional + premium_at_risk.
		consumed := intent.DeltaNotional + intent.PremiumAtRisk
		postCash := port.CashPct - pct(consumed)
		if port.CashPct < cfg.CashFloorPct || postCash < cfg.CashFloorPct {
			d.Veto = true
			d.Reasons = append(d.Reasons, "cash_floor_pct")
		}
	}

	// --- drawdown brake ---
	// Walk tiers; the deepest matching threshold wins. AtPct is negative
	// (e.g. -10 = 10% drawdown). HaltNew vetoes; SizeMult scales.
	for _, b := range cfg.DrawdownBrake {
		if port.DrawdownPct <= b.AtPct {
			if b.HaltNew {
				d.Veto = true
				d.Reasons = append(d.Reasons, "drawdown_brake_halt")
			}
			if b.SizeMult > 0 && b.SizeMult < d.SizeMult {
				d.SizeMult = b.SizeMult
				d.Warnings = append(d.Warnings,
					"drawdown brake: size scaled to "+ftoa(b.SizeMult)+"x")
			}
		}
	}

	// --- sub-sector cluster (SOFT) ---
	if cfg.SubsectorClusterPct > 0 && intent.Cluster != "" {
		clusterTotal := intent.DeltaNotional
		for _, p := range positions {
			if p.Cluster == intent.Cluster {
				clusterTotal += p.DeltaNotional
			}
		}
		if pct(clusterTotal) > cfg.SubsectorClusterPct {
			d.Warnings = append(d.Warnings,
				"sub-sector "+intent.Cluster+" exposure exceeds "+ftoa(cfg.SubsectorClusterPct)+"%")
		}
	}

	// --- concurrent positions ---
	// Count unique open symbols; if intent's symbol is new and we're at the
	// cap, veto. (Adds to an existing position don't increment.)
	if cfg.ConcurrentPositions > 0 {
		existing := map[string]struct{}{}
		for _, p := range positions {
			existing[p.Symbol] = struct{}{}
		}
		if _, here := existing[intent.Symbol]; !here {
			if len(existing) >= cfg.ConcurrentPositions {
				d.Veto = true
				d.Reasons = append(d.Reasons, "concurrent_positions")
			}
		}
	}

	return d
}

func ftoa(f float64) string { return strconv.FormatFloat(f, 'f', -1, 64) }
