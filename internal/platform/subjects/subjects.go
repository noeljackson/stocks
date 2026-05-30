// Package subjects holds the canonical NATS subject names (SPEC §3).
package subjects

const (
	// Ingestion (raw, normalized events from adapters).
	IngestFiling = "ingest.filing"
	IngestPrice  = "ingest.price"
	IngestMacro  = "ingest.macro"
	IngestNews   = "ingest.news"

	// Context layer.
	ContextUpdated = "context.updated"
	ContextShift   = "context.shift"

	// Market state / regime.
	RegimeState        = "regime.state"
	RegimeCapitulation = "regime.capitulation"

	// Discovery.
	DiscoveryCandidate = "discovery.candidate"

	// Thesis lifecycle.
	ThesisActionable  = "thesis.actionable"
	ThesisInvalidated = "thesis.invalidated"
	ThesisFulfilled   = "thesis.fulfilled"
	ThesisUpdated     = "thesis.updated"

	// Risk + decision.
	RiskVeto         = "risk.veto"
	RiskWarning      = "risk.warning"
	DecisionRecorded = "decision.recorded"
)

// Stream names (JetStream). Each stream is durable + replayable; consumers
// bind durable cursors against them (see Bus.Consume).
const (
	StreamIngest    = "INGEST"    // ingest.*
	StreamContext   = "CONTEXT"   // context.*
	StreamThesis    = "THESIS"    // thesis.*
	StreamMarket    = "MARKET"    // regime.*, discovery.*
	StreamDecisions = "DECISIONS" // risk.*, decision.*
)

// TickerRoute is the per-ticker routed subject (event router → context maintainer).
func TickerRoute(symbol string) string { return "route.ticker." + symbol }
