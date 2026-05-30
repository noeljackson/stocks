// Package regime classifies the macro regime (risk_on | neutral | risk_off)
// from a snapshot of named indicators against a versioned ruleset (SPEC §4).
//
// The classifier is a pure function: given (inputs, config) → (regime,
// capitulation, evaluated_rules). The service wrapper (service.go) handles
// I/O — subscribing to ingest.macro, maintaining the latest-value map,
// persisting market_state, publishing regime.state.
package regime

import (
	"fmt"
	"strconv"
	"strings"

	"github.com/noeljackson/stocks/internal/domain"
)

// Config is the parsed body of config (name='regime', active=true).
// See db/migrations/0002_seed.sql for the v1 shape.
type Config struct {
	States       []string                     `json:"states"`
	Rules        map[string]map[string]string `json:"rules"`        // regime → indicator → "<op><num>"
	Capitulation struct {
		AnyOf []string `json:"any_of"`
	} `json:"capitulation"`
}

// Result is what the classifier returns.
type Result struct {
	Regime       domain.Regime  `json:"regime"`
	Capitulation bool           `json:"capitulation"`
	Indicators   map[string]float64 `json:"indicators"`     // inputs seen (echoed back)
	Matched      map[string]float64 `json:"matched"`        // regime → fraction of its rules satisfied
	Reasons      []string           `json:"reasons,omitempty"`
}

// Classify evaluates cfg against the input snapshot.
//
// Decision: for each non-neutral state in cfg.Rules, score = (# rules satisfied
// using indicators we actually have) / (# rules total). The state with the
// highest score wins iff its score is >= 0.5 AND strictly greater than the
// other state's score. Otherwise neutral. This degrades gracefully when some
// inputs aren't ingested yet (SPX, breadth) — it never fabricates conviction.
func Classify(cfg Config, inputs map[string]float64) Result {
	out := Result{
		Regime:     domain.RegimeNeutral,
		Indicators: inputs,
		Matched:    map[string]float64{},
	}
	for state, rules := range cfg.Rules {
		sat, total := 0, 0
		for name, expr := range rules {
			total++
			v, ok := inputs[name]
			if !ok {
				continue
			}
			ok, err := evalExpr(v, expr)
			if err != nil {
				out.Reasons = append(out.Reasons, fmt.Sprintf("rule %s.%s: %v", state, name, err))
				continue
			}
			if ok {
				sat++
			}
		}
		score := 0.0
		if total > 0 {
			score = float64(sat) / float64(total)
		}
		out.Matched[state] = score
	}
	on, off := out.Matched["risk_on"], out.Matched["risk_off"]
	switch {
	case on >= 0.5 && on > off:
		out.Regime = domain.RegimeRiskOn
	case off >= 0.5 && off > on:
		out.Regime = domain.RegimeRiskOff
	}
	// Capitulation: any expression we can evaluate that is true.
	for _, e := range cfg.Capitulation.AnyOf {
		name, expr, ok := splitNameExpr(e)
		if !ok {
			continue
		}
		v, have := inputs[name]
		if !have {
			continue
		}
		fired, err := evalExpr(v, expr)
		if err == nil && fired {
			out.Capitulation = true
			out.Reasons = append(out.Reasons, fmt.Sprintf("capitulation: %s", e))
			break
		}
	}
	return out
}

// splitNameExpr parses "name<op><num>" (e.g. "vix>25", "put_call>1.10") into
// ("name", "<op><num>"). The compound forms in the seed config such as
// "vix9d_over_vix>1.10" are treated as a single input name plus an expr; the
// ingest layer is responsible for synthesizing such derived inputs.
func splitNameExpr(s string) (name, expr string, ok bool) {
	for i, r := range s {
		if r == '>' || r == '<' || r == '=' {
			return strings.TrimSpace(s[:i]), strings.TrimSpace(s[i:]), i > 0
		}
	}
	return "", "", false
}

// evalExpr evaluates "<op><number>" against v. Supported ops: > >= < <= == !=.
func evalExpr(v float64, expr string) (bool, error) {
	expr = strings.TrimSpace(expr)
	if expr == "" {
		return false, fmt.Errorf("empty expr")
	}
	var op string
	switch {
	case strings.HasPrefix(expr, ">="):
		op, expr = ">=", expr[2:]
	case strings.HasPrefix(expr, "<="):
		op, expr = "<=", expr[2:]
	case strings.HasPrefix(expr, "=="):
		op, expr = "==", expr[2:]
	case strings.HasPrefix(expr, "!="):
		op, expr = "!=", expr[2:]
	case strings.HasPrefix(expr, ">"):
		op, expr = ">", expr[1:]
	case strings.HasPrefix(expr, "<"):
		op, expr = "<", expr[1:]
	default:
		return false, fmt.Errorf("unknown op in %q", expr)
	}
	n, err := strconv.ParseFloat(strings.TrimSpace(expr), 64)
	if err != nil {
		return false, fmt.Errorf("bad number: %w", err)
	}
	switch op {
	case ">":
		return v > n, nil
	case ">=":
		return v >= n, nil
	case "<":
		return v < n, nil
	case "<=":
		return v <= n, nil
	case "==":
		return v == n, nil
	case "!=":
		return v != n, nil
	}
	return false, fmt.Errorf("unreachable")
}

// FREDSeriesToIndicator maps raw FRED series IDs to the indicator names the
// classifier rules reference. Unmapped series are still kept in the snapshot
// under their raw ID (useful for telemetry; the classifier just ignores them).
var FREDSeriesToIndicator = map[string]string{
	"VIXCLS":       "vix",
	"BAMLH0A0HYM2": "hy_oas_pct",
	"DGS10":        "us10y",
	"DGS3MO":       "us3m",
}
