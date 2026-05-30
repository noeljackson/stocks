// Package domain holds core types shared across services (mirrors db schema).
package domain

import "time"

// ThesisState is the per-thesis lifecycle state machine (SPEC §5.3).
type ThesisState string

const (
	StateForming            ThesisState = "forming"
	StateBuildingConviction ThesisState = "building_conviction"
	StateArmed              ThesisState = "armed"
	StateActionable         ThesisState = "actionable"
	StatePositionOpen       ThesisState = "position_open"
	StateExiting            ThesisState = "exiting"
	StateClosed             ThesisState = "closed"
	StateDisqualified       ThesisState = "disqualified"
)

// Regime classification (SPEC §4).
type Regime string

const (
	RegimeRiskOn  Regime = "risk_on"
	RegimeNeutral Regime = "neutral"
	RegimeRiskOff Regime = "risk_off"
)

// Alert is the unit pushed to the UI live feed (SPEC §3 FR7).
type Alert struct {
	ID           int64          `json:"id"`
	ThesisID     *string        `json:"thesis_id,omitempty"`
	Symbol       string         `json:"symbol,omitempty"`
	Kind         string         `json:"kind"` // state_transition|alignment|consensus|risk
	Payload      map[string]any `json:"payload,omitempty"`
	Acknowledged bool           `json:"acknowledged"`
	CreatedAt    time.Time      `json:"created_at"`
}
