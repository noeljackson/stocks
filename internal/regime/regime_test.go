package regime

import (
	"encoding/json"
	"testing"

	"github.com/noeljackson/stocks/internal/domain"
)

const seedConfigBody = `{
  "states": ["risk_on","neutral","risk_off"],
  "rules": {
    "risk_on":  {"spx_vs_sma12m": ">0", "hy_oas_pct": "<5", "breadth_pct_above_200d": ">50"},
    "risk_off": {"spx_vs_sma12m": "<0", "hy_oas_pct": ">7", "breadth_pct_above_200d": "<35"}
  },
  "capitulation": {"any_of": ["vix>25", "put_call>1.10"]}
}`

func loadSeed(t *testing.T) Config {
	t.Helper()
	var c Config
	if err := json.Unmarshal([]byte(seedConfigBody), &c); err != nil {
		t.Fatalf("seed unmarshal: %v", err)
	}
	return c
}

func TestClassifyRiskOnFullSignals(t *testing.T) {
	cfg := loadSeed(t)
	r := Classify(cfg, map[string]float64{
		"spx_vs_sma12m":          5.0,
		"hy_oas_pct":             3.2,
		"breadth_pct_above_200d": 65,
		"vix":                    14,
	})
	if r.Regime != domain.RegimeRiskOn {
		t.Fatalf("want risk_on, got %s (matched=%v)", r.Regime, r.Matched)
	}
	if r.Capitulation {
		t.Fatalf("unexpected capitulation")
	}
}

func TestClassifyRiskOff(t *testing.T) {
	cfg := loadSeed(t)
	r := Classify(cfg, map[string]float64{
		"spx_vs_sma12m":          -2,
		"hy_oas_pct":             8.5,
		"breadth_pct_above_200d": 30,
		"vix":                    30,
	})
	if r.Regime != domain.RegimeRiskOff {
		t.Fatalf("want risk_off, got %s (matched=%v)", r.Regime, r.Matched)
	}
	if !r.Capitulation {
		t.Fatalf("want capitulation true (vix=30)")
	}
}

func TestClassifyDegradesToNeutralWithoutSPX(t *testing.T) {
	cfg := loadSeed(t)
	// Only HY OAS is in risk-on territory; that's 1/3 of the rules → 0.33 score,
	// below the 0.5 threshold. Must not call risk_on.
	r := Classify(cfg, map[string]float64{"hy_oas_pct": 3.0})
	if r.Regime != domain.RegimeNeutral {
		t.Fatalf("want neutral, got %s (matched=%v)", r.Regime, r.Matched)
	}
}

func TestClassifyTieGoesNeutral(t *testing.T) {
	cfg := loadSeed(t)
	// HY OAS = 6 satisfies neither risk_on (<5) nor risk_off (>7); SPX/breadth absent.
	// Both states score 0; result must be neutral.
	r := Classify(cfg, map[string]float64{"hy_oas_pct": 6.0})
	if r.Regime != domain.RegimeNeutral {
		t.Fatalf("want neutral on zero score, got %s", r.Regime)
	}
}

func TestEvalExprOps(t *testing.T) {
	cases := []struct {
		v    float64
		expr string
		want bool
	}{
		{5, ">3", true}, {5, ">=5", true}, {5, "<10", true}, {5, "<=5", true},
		{5, "==5", true}, {5, "!=5", false}, {5, ">5", false},
	}
	for _, c := range cases {
		got, err := evalExpr(c.v, c.expr)
		if err != nil {
			t.Fatalf("evalExpr(%v,%q) err: %v", c.v, c.expr, err)
		}
		if got != c.want {
			t.Errorf("evalExpr(%v,%q) = %v, want %v", c.v, c.expr, got, c.want)
		}
	}
}

func TestEvalExprBadOp(t *testing.T) {
	if _, err := evalExpr(1, "~5"); err == nil {
		t.Fatal("expected error for unknown op")
	}
}
