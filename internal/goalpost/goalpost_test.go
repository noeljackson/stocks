package goalpost

import (
	"encoding/json"
	"testing"
)

// helper: parse a JSON conditions array
func conds(t *testing.T, body string) Conditions {
	t.Helper()
	var c Conditions
	if err := json.Unmarshal([]byte(body), &c); err != nil {
		t.Fatalf("conds: %v", err)
	}
	return c
}

// ---------- baseline ----------

func TestAnalyzeIdenticalIsClean(t *testing.T) {
	c := conds(t, `[{"type":"quantitative","name":"gm_collapse","expr":"gross_margin < 45"}]`)
	r := Analyze(c, c)
	if r.Weakened {
		t.Errorf("identical conditions must not be weakened: %+v", r)
	}
	if r.NeedsReview {
		t.Errorf("identical conditions must not need review")
	}
}

func TestAnalyzeEmptyOriginalNoWeakening(t *testing.T) {
	// No prior goalpost → there's nothing to move.
	r := Analyze(nil, conds(t, `[{"type":"quantitative","name":"new","expr":"x < 10"}]`))
	if r.Weakened {
		t.Errorf("first-version conditions cannot weaken anything: %+v", r)
	}
}

func TestAnalyzeAllConditionsDroppedIsWeakened(t *testing.T) {
	original := conds(t, `[{"type":"quantitative","name":"gm","expr":"gross_margin < 45"}]`)
	r := Analyze(original, nil)
	if !r.Weakened {
		t.Fatalf("removing all invalidation conditions is the canonical goalpost-move: %+v", r)
	}
	if len(r.Dropped) != 1 || r.Dropped[0] != "gm" {
		t.Errorf("dropped should list 'gm': %v", r.Dropped)
	}
}

// ---------- dropped ----------

func TestAnalyzeDroppedConditionIsWeakened(t *testing.T) {
	original := conds(t, `[
	  {"type":"quantitative","name":"gm","expr":"gross_margin < 45"},
	  {"type":"narrative","name":"hyperscale_cut","assertion":"Top-3 hyperscalers cut capex >15%"}
	]`)
	updated := conds(t, `[
	  {"type":"quantitative","name":"gm","expr":"gross_margin < 45"}
	]`)
	r := Analyze(original, updated)
	if !r.Weakened {
		t.Fatalf("dropping hyperscale_cut is a weakening: %+v", r)
	}
	if len(r.Dropped) != 1 || r.Dropped[0] != "hyperscale_cut" {
		t.Errorf("dropped: %v", r.Dropped)
	}
}

// ---------- loosened thresholds ----------

func TestAnalyzeLooseningLTIsWeakened(t *testing.T) {
	// `<` operator: raising the right-hand number makes invalidation HARDER
	// (gross_margin would have to fall further to fire) → weakening.
	original := conds(t, `[{"type":"quantitative","name":"gm","expr":"gross_margin < 45"}]`)
	updated := conds(t, `[{"type":"quantitative","name":"gm","expr":"gross_margin < 30"}]`)
	r := Analyze(original, updated)
	if !r.Weakened {
		t.Fatalf("'< 45' → '< 30' is a textbook loosening: %+v", r)
	}
	if len(r.Loosened) != 1 || r.Loosened[0] != "gm" {
		t.Errorf("loosened should cite gm: %v", r.Loosened)
	}
}

func TestAnalyzeLooseningGTIsWeakened(t *testing.T) {
	// `>` operator: RAISING the RHS makes invalidation HARDER (value would have
	// to exceed a higher bar to trip) → weakening.
	original := conds(t, `[{"type":"quantitative","name":"churn","expr":"churn_rate > 12"}]`)
	updated := conds(t, `[{"type":"quantitative","name":"churn","expr":"churn_rate > 20"}]`)
	r := Analyze(original, updated)
	if !r.Weakened {
		t.Fatalf("'> 12' → '> 20' (raising the bar) is a loosening: %+v", r)
	}
}

func TestAnalyzeTighteningGTIsClean(t *testing.T) {
	// `>` operator: LOWERING the RHS makes invalidation EASIER → strictening.
	original := conds(t, `[{"type":"quantitative","name":"churn","expr":"churn_rate > 12"}]`)
	updated := conds(t, `[{"type":"quantitative","name":"churn","expr":"churn_rate > 8"}]`)
	r := Analyze(original, updated)
	if r.Weakened {
		t.Errorf("'> 12' → '> 8' (lowering the bar) is a tightening: %+v", r)
	}
}

func TestAnalyzeTighteningLTIsClean(t *testing.T) {
	// Tightened: '< 45' → '< 50' = easier to fire → STRICTER on the thesis.
	original := conds(t, `[{"type":"quantitative","name":"gm","expr":"gross_margin < 45"}]`)
	updated := conds(t, `[{"type":"quantitative","name":"gm","expr":"gross_margin < 50"}]`)
	r := Analyze(original, updated)
	if r.Weakened {
		t.Errorf("'< 45' → '< 50' is a tightening, not a loosening: %+v", r)
	}
}

// ---------- added ----------

func TestAnalyzeAddedConditionIsClean(t *testing.T) {
	original := conds(t, `[{"type":"quantitative","name":"gm","expr":"gross_margin < 45"}]`)
	updated := conds(t, `[
	  {"type":"quantitative","name":"gm","expr":"gross_margin < 45"},
	  {"type":"quantitative","name":"capex","expr":"capex_growth < 0"}
	]`)
	r := Analyze(original, updated)
	if r.Weakened {
		t.Errorf("adding new ways to be wrong is strengthening, not weakening: %+v", r)
	}
	if len(r.Added) != 1 || r.Added[0] != "capex" {
		t.Errorf("added: %v", r.Added)
	}
}

// ---------- narrative ----------

func TestAnalyzeNarrativeChangeNeedsReview(t *testing.T) {
	// We can't parse natural language without an LLM; surface for human review.
	original := conds(t, `[{"type":"narrative","name":"moat","assertion":"AMD cannot match HBM scale before 2027"}]`)
	updated := conds(t, `[{"type":"narrative","name":"moat","assertion":"AMD cannot match HBM scale before mid-2028"}]`)
	r := Analyze(original, updated)
	if !r.NeedsReview {
		t.Fatalf("narrative changes must be flagged for human review: %+v", r)
	}
}

func TestAnalyzeNarrativeIdenticalNoReview(t *testing.T) {
	c := conds(t, `[{"type":"narrative","name":"moat","assertion":"AMD cannot match HBM scale before 2027"}]`)
	r := Analyze(c, c)
	if r.NeedsReview {
		t.Errorf("identical narrative must NOT trigger review: %+v", r)
	}
}

// ---------- rewrite (no overlap) ----------

func TestAnalyzePureRewriteFlagsReview(t *testing.T) {
	// No condition name overlaps. We have no way to compare algorithmically.
	original := conds(t, `[{"type":"quantitative","name":"gm","expr":"gross_margin < 45"}]`)
	updated := conds(t, `[{"type":"quantitative","name":"yoy_rev","expr":"yoy_revenue_growth < 5"}]`)
	r := Analyze(original, updated)
	if !r.NeedsReview {
		t.Fatalf("name-disjoint rewrite must be flagged for review: %+v", r)
	}
	// A rewrite is conservatively a drop (of the old) + add (of the new). The
	// drop alone is a weakening signal — moving from gm-tracked to rev-tracked
	// without keeping the original is exactly the goalpost behavior we guard.
	if !r.Weakened {
		t.Errorf("rewrite that drops an existing invalidation IS weakening: %+v", r)
	}
}

// ---------- mixed signals ----------

func TestAnalyzeMixedSignalsWeakensIfAnyWeaken(t *testing.T) {
	original := conds(t, `[
	  {"type":"quantitative","name":"gm","expr":"gross_margin < 45"},
	  {"type":"quantitative","name":"capex","expr":"capex_growth < 0"}
	]`)
	updated := conds(t, `[
	  {"type":"quantitative","name":"gm","expr":"gross_margin < 30"},
	  {"type":"quantitative","name":"capex","expr":"capex_growth < 0"},
	  {"type":"quantitative","name":"share_loss","expr":"market_share_pct < 30"}
	]`)
	r := Analyze(original, updated)
	if !r.Weakened {
		t.Fatalf("ANY weakening signal must dominate: %+v", r)
	}
	if len(r.Loosened) != 1 || len(r.Added) != 1 {
		t.Errorf("expected 1 loosened + 1 added: %+v", r)
	}
}

// ---------- expression parsing ----------

func TestParseExprLooseningTable(t *testing.T) {
	// (oldExpr, newExpr, looser?)
	cases := []struct {
		old, new string
		looser   bool
	}{
		// `<` raised RHS = looser
		{"x < 45", "x < 30", true},
		{"x < 45", "x < 50", false},
		{"x < 45", "x < 45", false},
		// `<=` same shape
		{"x <= 45", "x <= 30", true},
		// `>` raised RHS = looser (value must exceed a higher bar)
		{"x > 12", "x > 20", true},
		{"x > 12", "x > 8", false},
		// `>=` same shape
		{"x >= 12", "x >= 20", true},
		// different field/operator → cannot determine
		{"x < 45", "y < 45", false}, // different field; treat as not-looser (caller handles via name match)
		// `==` and `!=` cannot meaningfully loosen — treat as not-looser
		{"x == 5", "x == 6", false},
	}
	for _, c := range cases {
		got := exprIsLooserThan(c.new, c.old)
		if got != c.looser {
			t.Errorf("exprIsLooserThan(%q vs %q) = %v, want %v", c.new, c.old, got, c.looser)
		}
	}
}
