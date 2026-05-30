package router

import "testing"

func TestExtractSymbolEDGAR(t *testing.T) {
	sym, ok := extractSymbol([]byte(`{"ticker":"NVDA","form":"10-K"}`))
	if !ok || sym != "NVDA" {
		t.Fatalf("got (%q,%v) want (NVDA,true)", sym, ok)
	}
}

func TestExtractSymbolLowercaseNormalizes(t *testing.T) {
	sym, ok := extractSymbol([]byte(`{"ticker":"nvda"}`))
	if !ok || sym != "NVDA" {
		t.Fatalf("got (%q,%v) want (NVDA,true) — must uppercase", sym, ok)
	}
}

func TestExtractSymbolAcceptsSymbolKey(t *testing.T) {
	sym, ok := extractSymbol([]byte(`{"symbol":"MU"}`))
	if !ok || sym != "MU" {
		t.Fatalf("got (%q,%v) want (MU,true)", sym, ok)
	}
}

func TestExtractSymbolMarketWide(t *testing.T) {
	for _, body := range []string{
		`{"series":"VIXCLS","value":"15"}`, // FRED
		`{}`,
		`{"ticker":""}`,
	} {
		if sym, ok := extractSymbol([]byte(body)); ok {
			t.Errorf("market-wide %s should yield no symbol, got %q", body, sym)
		}
	}
}

func TestExtractSymbolMalformed(t *testing.T) {
	if _, ok := extractSymbol([]byte(`not json`)); ok {
		t.Fatalf("malformed JSON must not produce a symbol")
	}
}

func TestExtractSymbolStripsWhitespace(t *testing.T) {
	sym, ok := extractSymbol([]byte(`{"ticker":"  amd  "}`))
	if !ok || sym != "AMD" {
		t.Fatalf("got (%q,%v) want (AMD,true) — must trim", sym, ok)
	}
}

func TestRouteSubject(t *testing.T) {
	if got, want := routeSubject("NVDA"), "route.ticker.NVDA"; got != want {
		t.Errorf("got %q want %q", got, want)
	}
}
