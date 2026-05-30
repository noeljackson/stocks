package regime

import (
	"log/slog"
	"testing"

	"github.com/noeljackson/stocks/internal/domain"
)

// newTestService returns a Service with nil DB/Bus — safe because applyObservation
// and parseObservation are pure (no I/O).
func newTestService(t *testing.T) *Service {
	t.Helper()
	return &Service{
		Log:  slog.Default(),
		snap: map[string]float64{},
		last: domain.RegimeNeutral,
	}
}

func TestParseObservationFREDMapping(t *testing.T) {
	obs, ok := parseObservation([]byte(`{"series":"VIXCLS","date":"2026-05-30","value":"28.5"}`))
	if !ok {
		t.Fatal("expected ok")
	}
	if obs.Name != "vix" {
		t.Errorf("name: got %q want vix", obs.Name)
	}
	if obs.Value != 28.5 {
		t.Errorf("value: got %v want 28.5", obs.Value)
	}
}

func TestParseObservationUnknownSeriesPassesThrough(t *testing.T) {
	obs, ok := parseObservation([]byte(`{"series":"FOO","date":"2026-05-30","value":"1.0"}`))
	if !ok {
		t.Fatal("expected ok")
	}
	if obs.Name != "FOO" {
		t.Errorf("unknown series should keep its raw name; got %q", obs.Name)
	}
}

func TestParseObservationRejectsMalformed(t *testing.T) {
	for _, c := range []struct {
		name string
		body string
	}{
		{"not json", `not json`},
		{"empty series", `{"series":"","value":"1"}`},
		{"missing value", `{"series":"VIXCLS"}`},
		{"non-numeric value", `{"series":"VIXCLS","value":"NA"}`},
		{"FRED missing marker", `{"series":"VIXCLS","value":"."}`},
	} {
		t.Run(c.name, func(t *testing.T) {
			if _, ok := parseObservation([]byte(c.body)); ok {
				t.Fatalf("expected !ok for %s", c.name)
			}
		})
	}
}

func TestApplyObservationAccumulatesSnapshot(t *testing.T) {
	cfg := loadSeed(t)
	s := newTestService(t)

	// First observation: only hy_oas_pct (3.0) — risk_on score = 1/3, below 0.5
	r1, snap1, changed1 := s.applyObservation(cfg, observation{Series: "BAMLH0A0HYM2", Name: "hy_oas_pct", Value: 3.0})
	if r1.Regime != domain.RegimeNeutral {
		t.Fatalf("after 1 obs want neutral, got %s (matched=%v)", r1.Regime, r1.Matched)
	}
	if changed1 {
		t.Errorf("first obs neutral→neutral is not a change")
	}
	if got, ok := snap1["hy_oas_pct"]; !ok || got != 3.0 {
		t.Errorf("snapshot missing hy_oas_pct=3.0: %v", snap1)
	}

	// Add SPX positive — risk_on now 2/3, above threshold and beats risk_off (0/3)
	r2, _, changed2 := s.applyObservation(cfg, observation{Name: "spx_vs_sma12m", Value: 5})
	if r2.Regime != domain.RegimeRiskOn {
		t.Fatalf("after 2 obs want risk_on, got %s (matched=%v)", r2.Regime, r2.Matched)
	}
	if !changed2 {
		t.Errorf("neutral→risk_on must register as a change")
	}

	// Re-applying same obs — regime stays, changed must be false.
	_, _, changed3 := s.applyObservation(cfg, observation{Name: "spx_vs_sma12m", Value: 5})
	if changed3 {
		t.Errorf("idempotent re-apply should not register as a change")
	}
}

func TestApplyObservationCapitulationLatch(t *testing.T) {
	cfg := loadSeed(t)
	s := newTestService(t)

	// VIX spike crosses capitulation (>25). Regime stays neutral (no other signals).
	r, _, changed := s.applyObservation(cfg, observation{Name: "vix", Value: 32})
	if !r.Capitulation {
		t.Fatalf("vix=32 should trigger capitulation; matched=%v reasons=%v", r.Matched, r.Reasons)
	}
	if !changed {
		t.Errorf("capitulation false→true must register as a change")
	}
	if r.Regime != domain.RegimeNeutral {
		t.Errorf("capitulation must not by itself imply risk_off")
	}

	// VIX drops below threshold — capitulation must clear.
	r2, _, changed2 := s.applyObservation(cfg, observation{Name: "vix", Value: 12})
	if r2.Capitulation {
		t.Errorf("vix=12 should clear capitulation")
	}
	if !changed2 {
		t.Errorf("capitulation true→false must register as a change")
	}
}
