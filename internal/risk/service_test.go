package risk

import (
	"encoding/json"
	"testing"
)

func TestActionableDecodesMinimalShape(t *testing.T) {
	var a actionable
	if err := json.Unmarshal([]byte(`{
	  "thesis_id":"abc",
	  "symbol":"NVDA",
	  "cluster":"logic_accelerators",
	  "instrument":"equity",
	  "delta_notional":8000
	}`), &a); err != nil {
		t.Fatal(err)
	}
	if a.Symbol != "NVDA" || a.DeltaNotional != 8000 || a.PremiumAtRisk != 0 {
		t.Errorf("decoded: %+v", a)
	}
}

func TestActionableIgnoresExtraFields(t *testing.T) {
	// The thesis engine is free to add fields; the overlay must not break.
	var a actionable
	if err := json.Unmarshal([]byte(`{"symbol":"MU","new_field":"future"}`), &a); err != nil {
		t.Fatal(err)
	}
	if a.Symbol != "MU" {
		t.Errorf("expected MU, got %q", a.Symbol)
	}
}

func TestToIntentMapsFieldsOneToOne(t *testing.T) {
	a := actionable{
		ThesisID: "id", Symbol: "AMD", Cluster: "logic_accelerators",
		Instrument: "leaps", DeltaNotional: 1, PremiumAtRisk: 2,
	}
	got := toIntent(a)
	want := Intent{
		Symbol: "AMD", Cluster: "logic_accelerators", Instrument: "leaps",
		DeltaNotional: 1, PremiumAtRisk: 2,
	}
	if got != want {
		t.Errorf("got %+v want %+v", got, want)
	}
}
