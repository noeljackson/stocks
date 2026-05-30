// Package router fans ingest events out to per-ticker subjects so that
// downstream services (the Python context maintainer in particular) can
// subscribe with a single durable consumer rather than dispatching N
// subject patterns themselves.
//
// Pure logic: extractSymbol(payload) → (symbol, ok). Service: durable
// consumer on INGEST/ingest.*, republishes to route.ticker.<SYMBOL> on
// the TICKER stream when a symbol is present (market-wide events drop).
package router

import (
	"encoding/json"
	"strings"
)

// extractSymbol looks for a "ticker" or "symbol" string field in the JSON
// payload and returns the upper-cased + trimmed value. Returns ok=false for
// malformed JSON, missing fields, or empty/whitespace values — those events
// are market-wide and should not be per-ticker routed.
func extractSymbol(payload []byte) (string, bool) {
	var m map[string]any
	if err := json.Unmarshal(payload, &m); err != nil {
		return "", false
	}
	for _, key := range []string{"ticker", "symbol"} {
		v, ok := m[key]
		if !ok {
			continue
		}
		s, ok := v.(string)
		if !ok {
			continue
		}
		s = strings.ToUpper(strings.TrimSpace(s))
		if s == "" {
			continue
		}
		return s, true
	}
	return "", false
}

func routeSubject(symbol string) string { return "route.ticker." + symbol }
