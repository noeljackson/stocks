// Package goalpost is the integrity guard for thesis invalidation conditions
// (SPEC §5.3). When a thesis is revised, Analyze compares the new
// invalidation_conditions against the immutable_original and flags whether
// the goalpost has been moved — i.e. whether the thesis became HARDER to
// invalidate (looser thresholds, dropped conditions). Pure function; the
// caller persists the verdict to thesis_version_history.weakens_invalidation
// and/or emits an alert.
//
// Decisions:
//   - Conditions are matched by stable `name`. A condition that disappears
//     by name is "Dropped" (a weakening signal).
//   - Quantitative threshold loosening is detected only for the operators
//     <, <=, >, >= against a literal number, with identical LHS field.
//     `==` / `!=` and asymmetric or compound expressions are not auto-judged.
//   - Narrative ("type":"narrative") changes are never auto-judged — they
//     surface as NeedsReview so a human (or future LLM pass) can adjudicate.
//   - A rewrite (no name overlap between original and updated) is conservatively
//     treated as drop + add → Weakened AND NeedsReview.
package goalpost

import (
	"strconv"
	"strings"
)

// Condition mirrors the JSON shape stored in thesis.invalidation_conditions.
// The thesis engine is free to add fields; we only depend on the four below.
type Condition struct {
	Type      string `json:"type"`              // "quantitative" | "narrative"
	Name      string `json:"name"`              // stable identifier for diffing
	Expr      string `json:"expr,omitempty"`    // quantitative; e.g. "gross_margin < 45"
	Assertion string `json:"assertion,omitempty"` // narrative
}

type Conditions []Condition

// Report is the verdict on a thesis version transition.
type Report struct {
	Weakened    bool     `json:"weakened"`
	NeedsReview bool     `json:"needs_review"`
	Dropped     []string `json:"dropped,omitempty"`  // names present in original, missing in updated
	Loosened    []string `json:"loosened,omitempty"` // names whose threshold became easier
	Added       []string `json:"added,omitempty"`    // names present in updated, missing in original
	Reasons     []string `json:"reasons,omitempty"`  // human-readable summary lines
}

// Analyze compares two condition sets and produces an integrity verdict.
func Analyze(original, updated Conditions) Report {
	r := Report{}

	origByName := index(original)
	updByName := index(updated)

	// Dropped (in original, missing in updated).
	for name := range origByName {
		if _, ok := updByName[name]; !ok {
			r.Dropped = append(r.Dropped, name)
		}
	}
	// Added (in updated, missing in original).
	for name := range updByName {
		if _, ok := origByName[name]; !ok {
			r.Added = append(r.Added, name)
		}
	}
	// Modified (same name, different content).
	for name, oc := range origByName {
		uc, ok := updByName[name]
		if !ok {
			continue
		}
		switch {
		case oc.Type == "quantitative" && uc.Type == "quantitative":
			if exprIsLooserThan(uc.Expr, oc.Expr) {
				r.Loosened = append(r.Loosened, name)
				r.Reasons = append(r.Reasons,
					"loosened "+name+": "+oc.Expr+" → "+uc.Expr)
			}
		case oc.Type == "narrative" || uc.Type == "narrative":
			if oc.Assertion != uc.Assertion || oc.Type != uc.Type {
				r.NeedsReview = true
				r.Reasons = append(r.Reasons,
					"narrative changed for "+name+" — human review")
			}
		}
	}

	// Composite verdict.
	if len(r.Dropped) > 0 || len(r.Loosened) > 0 {
		r.Weakened = true
	}
	for _, n := range r.Dropped {
		r.Reasons = append(r.Reasons, "dropped invalidation condition: "+n)
	}
	// Pure rewrite (originals dropped, all-new added): flag for review even if
	// already marked weakened.
	if len(original) > 0 && len(r.Dropped) == len(original) && len(r.Added) > 0 {
		r.NeedsReview = true
	}
	return r
}

func index(cs Conditions) map[string]Condition {
	m := make(map[string]Condition, len(cs))
	for _, c := range cs {
		if c.Name == "" {
			continue // unnamed conditions are not diffable
		}
		m[c.Name] = c
	}
	return m
}

// exprIsLooserThan reports whether newExpr is a loosened version of oldExpr
// over the same LHS field and operator family. Recognised forms:
//
//	"<field> < <num>"   "<field> <= <num>"   → looser = larger num
//	"<field> > <num>"   "<field> >= <num>"   → looser = smaller num
//
// Any unparseable form returns false (we don't fabricate verdicts).
func exprIsLooserThan(newExpr, oldExpr string) bool {
	nf, nop, nn, ok1 := parseSimpleExpr(newExpr)
	of, oop, on, ok2 := parseSimpleExpr(oldExpr)
	if !ok1 || !ok2 {
		return false
	}
	if nf != of {
		return false // different field — can't compare
	}
	// Normalise operator families.
	family := func(op string) string {
		switch op {
		case "<", "<=":
			return "<"
		case ">", ">=":
			return ">"
		}
		return op
	}
	if family(nop) != family(oop) {
		return false
	}
	switch family(nop) {
	case "<":
		// "field < N" invalidates if field drops below N. Lower N → field must
		// fall further → harder to trip → LOOSER goalpost.
		return nn < on
	case ">":
		// "field > N" invalidates if field exceeds N. Higher N → field must
		// climb further → harder to trip → LOOSER goalpost.
		return nn > on
	}
	return false
}

// parseSimpleExpr splits "<field> <op> <number>" with whitespace tolerance.
// op ∈ { <, <=, >, >=, ==, != }. LHS may be any non-space token (no spaces).
func parseSimpleExpr(s string) (field, op string, num float64, ok bool) {
	s = strings.TrimSpace(s)
	// Search for the first operator occurrence; check longer ops first.
	for _, candidate := range []string{"<=", ">=", "==", "!=", "<", ">"} {
		idx := strings.Index(s, candidate)
		if idx <= 0 {
			continue
		}
		lhs := strings.TrimSpace(s[:idx])
		rhs := strings.TrimSpace(s[idx+len(candidate):])
		if lhs == "" || rhs == "" || strings.ContainsAny(lhs, " \t") {
			continue
		}
		n, err := strconv.ParseFloat(rhs, 64)
		if err != nil {
			continue
		}
		return lhs, candidate, n, true
	}
	return "", "", 0, false
}
