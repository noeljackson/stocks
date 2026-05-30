package risk

import (
	"encoding/json"
	"testing"
)

// seedRiskConfig matches db/migrations/0002_seed.sql (config name='risk' v1).
const seedRiskConfig = `{
  "single_name_delta_notional_pct": 15,
  "options_premium_aggregate_pct": 20,
  "cash_floor_pct": 20,
  "drawdown_brake": [
    {"at_pct": -10, "size_mult": 0.5},
    {"at_pct": -20, "halt_new": true}
  ],
  "subsector_cluster_pct": 35,
  "concurrent_positions": 7
}`

func mustConfig(t *testing.T) Config {
	t.Helper()
	var c Config
	if err := json.Unmarshal([]byte(seedRiskConfig), &c); err != nil {
		t.Fatalf("config: %v", err)
	}
	return c
}

func basePortfolio() Portfolio {
	return Portfolio{TotalValue: 100_000, CashPct: 50, DrawdownPct: 0}
}

// ---------- happy path ----------

func TestEvaluatePassesUnderAllLimits(t *testing.T) {
	cfg := mustConfig(t)
	d := Evaluate(Intent{Symbol: "NVDA", Cluster: "logic_accelerators", Instrument: "equity", DeltaNotional: 8_000},
		nil, basePortfolio(), cfg)
	if d.Veto {
		t.Fatalf("clean entry must not veto: %+v", d)
	}
	if d.SizeMult != 1.0 {
		t.Errorf("size mult: got %v want 1.0", d.SizeMult)
	}
}

// ---------- single-name concentration ----------

func TestEvaluateVetoesOverSingleNameCap(t *testing.T) {
	cfg := mustConfig(t)
	// Existing 12k notional in NVDA = 12% of 100k; adding 4k → 16% > 15% cap.
	existing := []Position{{Symbol: "NVDA", Cluster: "logic_accelerators", Instrument: "equity", DeltaNotional: 12_000}}
	d := Evaluate(Intent{Symbol: "NVDA", Cluster: "logic_accelerators", Instrument: "equity", DeltaNotional: 4_000},
		existing, basePortfolio(), cfg)
	if !d.Veto {
		t.Fatalf("must veto: existing 12%% + 4%% > 15%% cap. got %+v", d)
	}
	if !containsRule(d.Reasons, "single_name_delta_notional_pct") {
		t.Errorf("reason should cite single-name cap: %v", d.Reasons)
	}
}

func TestEvaluateAllowsAtExactlySingleNameCap(t *testing.T) {
	cfg := mustConfig(t)
	existing := []Position{{Symbol: "NVDA", DeltaNotional: 10_000}}
	d := Evaluate(Intent{Symbol: "NVDA", Instrument: "equity", DeltaNotional: 5_000},
		existing, basePortfolio(), cfg)
	if d.Veto {
		t.Fatalf("exactly at 15%% should pass, not veto: %+v", d)
	}
}

// ---------- options aggregate ----------

func TestEvaluateVetoesOverOptionsAggregate(t *testing.T) {
	cfg := mustConfig(t)
	// Existing premium-at-risk: 18k across the book → 18%. Adding 3k → 21% > 20%.
	existing := []Position{
		{Symbol: "AAPL", Instrument: "leaps", PremiumAtRisk: 10_000},
		{Symbol: "MU", Instrument: "leaps", PremiumAtRisk: 8_000},
	}
	d := Evaluate(Intent{Symbol: "NVDA", Instrument: "leaps", PremiumAtRisk: 3_000},
		existing, basePortfolio(), cfg)
	if !d.Veto {
		t.Fatalf("options agg 21%% should veto. got %+v", d)
	}
	if !containsRule(d.Reasons, "options_premium_aggregate_pct") {
		t.Errorf("reason should cite options agg cap: %v", d.Reasons)
	}
}

// ---------- cash floor ----------

func TestEvaluateVetoesBelowCashFloor(t *testing.T) {
	cfg := mustConfig(t)
	p := basePortfolio()
	p.CashPct = 18 // already below the 20% floor
	d := Evaluate(Intent{Symbol: "NVDA", Instrument: "equity", DeltaNotional: 1_000},
		nil, p, cfg)
	if !d.Veto {
		t.Fatalf("below cash floor must veto: %+v", d)
	}
	if !containsRule(d.Reasons, "cash_floor_pct") {
		t.Errorf("reason should cite cash floor: %v", d.Reasons)
	}
}

func TestEvaluateVetoesIfEntryWouldBreachCashFloor(t *testing.T) {
	cfg := mustConfig(t)
	p := basePortfolio()
	p.CashPct = 22 // above floor today
	// Adding 5k of delta_notional from cash = 5% drag → drops cash to 17% < 20% floor.
	d := Evaluate(Intent{Symbol: "NVDA", Instrument: "equity", DeltaNotional: 5_000},
		nil, p, cfg)
	if !d.Veto {
		t.Fatalf("entry that would drop cash below 20%% floor must veto: %+v", d)
	}
}

// ---------- drawdown brake ----------

func TestDrawdownBrakeHalvesSize(t *testing.T) {
	cfg := mustConfig(t)
	p := basePortfolio()
	p.DrawdownPct = -12 // below -10 → 0.5x
	d := Evaluate(Intent{Symbol: "NVDA", Instrument: "equity", DeltaNotional: 5_000},
		nil, p, cfg)
	if d.Veto {
		t.Fatalf("drawdown -12%% should warn + scale, not veto: %+v", d)
	}
	if d.SizeMult != 0.5 {
		t.Errorf("size mult: got %v want 0.5", d.SizeMult)
	}
	if len(d.Warnings) == 0 {
		t.Errorf("drawdown brake must record a warning")
	}
}

func TestDrawdownBrakeHaltsBelowSecondTier(t *testing.T) {
	cfg := mustConfig(t)
	p := basePortfolio()
	p.DrawdownPct = -21 // below -20 → halt_new
	d := Evaluate(Intent{Symbol: "NVDA", Instrument: "equity", DeltaNotional: 1_000},
		nil, p, cfg)
	if !d.Veto {
		t.Fatalf("drawdown < -20%% must halt new entries: %+v", d)
	}
}

// ---------- sub-sector soft warning ----------

func TestSubsectorClusterIsWarningNotVeto(t *testing.T) {
	cfg := mustConfig(t)
	// 35% cluster exposure already; new entry pushes over.
	existing := []Position{
		{Symbol: "NVDA", Cluster: "logic_accelerators", Instrument: "equity", DeltaNotional: 14_000},
		{Symbol: "AMD", Cluster: "logic_accelerators", Instrument: "equity", DeltaNotional: 14_000},
		{Symbol: "AVGO", Cluster: "logic_accelerators", Instrument: "equity", DeltaNotional: 7_000},
	}
	d := Evaluate(Intent{Symbol: "TSM", Cluster: "logic_accelerators", Instrument: "equity", DeltaNotional: 2_000},
		existing, basePortfolio(), cfg)
	if d.Veto {
		t.Fatalf("sub-sector cap is SOFT — must not veto. got %+v", d)
	}
	if len(d.Warnings) == 0 {
		t.Errorf("sub-sector breach should produce a warning")
	}
}

// ---------- concurrent positions ----------

func TestConcurrentPositionsCap(t *testing.T) {
	cfg := mustConfig(t)
	pos := make([]Position, 7)
	for i := range pos {
		pos[i] = Position{Symbol: "POS" + string(rune('A'+i)), Instrument: "equity", DeltaNotional: 1_000}
	}
	d := Evaluate(Intent{Symbol: "EIGHTH", Instrument: "equity", DeltaNotional: 1_000},
		pos, basePortfolio(), cfg)
	if !d.Veto {
		t.Fatalf("8th concurrent position must veto: %+v", d)
	}
	if !containsRule(d.Reasons, "concurrent_positions") {
		t.Errorf("reason should cite concurrent positions: %v", d.Reasons)
	}
}

// ---------- helpers ----------

func containsRule(rs []string, sub string) bool {
	for _, r := range rs {
		if len(r) >= len(sub) && indexOf(r, sub) >= 0 {
			return true
		}
	}
	return false
}

func indexOf(s, sub string) int {
	for i := 0; i+len(sub) <= len(s); i++ {
		if s[i:i+len(sub)] == sub {
			return i
		}
	}
	return -1
}
