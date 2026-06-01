# Brain Loops

This document maps the system as a set of loops. The product goal is not a
static dashboard; it is a continuously running research brain that keeps market
evidence fresh, maintains one current view per ticker, and interrupts the
operator only when judgment is required.

## Top-Level Brain

```text
                broad market + company data
                         |
                         v
                +------------------+
                |  ingest loops    |
                |  price/news/etc  |
                +------------------+
                         |
                         v
                +------------------+
                |  evidence state  |
                |  what we have    |
                |  what is missing |
                +------------------+
                         |
                         v
                +------------------+
                | ticker context   |
                | structural       |
                | narrative        |
                | market           |
                +------------------+
                         |
                         v
                +------------------+
                | current thesis   |
                | one open view    |
                | version history  |
                +------------------+
                         |
              +----------+----------+
              |                     |
              v                     v
       +-------------+       +----------------+
       | attention   |       | evaluation     |
       | operator    |       | conditions     |
       | judgment    |       | consensus      |
       +-------------+       | staleness      |
              |              +----------------+
              v                     |
       +-------------+              |
       | decision    |<-------------+
       +-------------+
              |
              v
       +-------------+
       | position    |
       | outcome     |
       | reflection  |
       +-------------+
```

The invariant is: every active ticker should either have a current thesis, a
visible reason why no thesis exists, or an evidence/source state explaining what
the system is waiting for.

## Data Acquisition Loops

These loops feed the brain. They should be broad enough to cover:

```text
active discovery_pool
UNION
active ticker/watchlist universe
UNION
benchmarks needed for regime/relative context
```

Current local dev cadence:

```text
FMP price bars          every 30m    discovery_pool + benchmarks
FMP screener/pool       every 24h    broad investible pool
FMP estimates           every 30m    scan universe
CBOE crowd sentiment    every 30m    market-wide sentiment
FMP + Massive news      every 30m    scan universe
XBRL company facts      every 6h     scan universe
EDGAR filings           every 30m    scan universe via dynamic SEC CIK map
FRED macro              every 30m    macro series
```

The current cadence now aims source checks at the desired 30-minute freshness
SLA for sources that can move intraday. XBRL remains slower because company
facts are large and update through filings, while EDGAR is the intraday filing
watch. The target brain loop is:

```text
source due
  -> respect vendor limiter
  -> fetch available rows
  -> persist source_health
  -> update evidence_requirement
  -> publish events for downstream loops
```

Open gap: #128 should become the canonical freshness orchestrator. It should
decide what is stale, what source is safe to call next, and when to slow down
because a vendor is rate limiting.

## Discovery Loop

Discovery is the broad radar. It should be cheap, deterministic, and explicit
about why something deserves cognition.

```text
discovery_pool + active tickers
        |
        v
cheap detectors
  volume_anomaly
  base_breakout
  estimate_revision_velocity
  news_sentiment_shift
        |
        v
composition
  raw signals
  price extension
  200-day SMA / available SMA context
  RSI
  watchlist state
  open thesis state
        |
        v
operator interpretation
  early_accumulation
  breakout_confirmation
  extended_momentum
  consensus_arrival
  possible_exhaustion
  existing_thesis_trigger
        |
        v
candidate_review attention
```

Research nominations are a second discovery path:

```text
unreviewed theme-relevant pool member
  + enough available evidence
  + explicit business/theme reason
        |
        v
research_nomination candidate_review
```

What works now:

- The scanner covers the pool plus active ticker universe.
- Raw signals are composed with price-extension context.
- Proactive names are queued as reasoned `research_nomination` items, not
  generic inspections.
- Pending candidates are ranked before review using deterministic signal
  quality, domain fit, proposed tier, and watchlist-classifier confidence.

Current gaps:

- #147: attention status is still too flat for queued/evaluating/deferred/blocked.
- #143/#129: macro and sector theses do not yet steer discovery ranking.

## Attention Loop

Attention is not an event log. It means "the operator or system needs to make
progress here."

Current simplified state:

```text
open -> resolved
open -> dismissed
```

Desired state machine:

```text
queued
  -> evaluating
  -> waiting_on_data
  -> ready_for_review
  -> operator_deferred
  -> actionable
  -> resolved
  -> dismissed
  -> blocked
```

Each transition should record:

```text
from_state
to_state
owner          system | operator | source | cognition | risk
reason
next_retry_at
resurface_at
source_ref
```

Current gaps:

- #147: implement the real attention FSM.
- #121: defer needs to actually resurface later.
- #89: each attention item should open a review packet with the same resolve
  grammar.
- #126: the workspace needs an explicit operator workflow rail.

## Cognition Loop

Cognition is the ticker brain. It reacts to explicit promotion and also runs a
bounded maintenance sweep.

Event-driven path:

```text
candidate confirmed
        |
        v
discovery.confirmed
        |
        v
cognition consumer
  -> refresh evidence_requirement
  -> refresh ticker_context
  -> draft or reconcile thesis
  -> sharpen thesis
  -> challenge thesis
```

Scheduled path:

```text
every COGNITION_SWEEP_SECONDS
        |
        v
select up to COGNITION_MAX_SYMBOLS_PER_SWEEP active tickers where:
  no context exists
  evidence checklist is missing
  context is older than COGNITION_CONTEXT_MAX_AGE_HOURS
  open thesis is older than COGNITION_OPEN_THESIS_MAX_AGE_MINUTES
  missing evidence retry is due
  evidence became satisfied after a decline
  no-thesis decline is older than COGNITION_DECLINE_RETRY_HOURS
        |
        v
run the same cognition pipeline
```

Current defaults:

```text
COGNITION_SWEEP_SECONDS                900
COGNITION_CONTEXT_MAX_AGE_HOURS        12
COGNITION_OPEN_THESIS_MAX_AGE_MINUTES  30
COGNITION_DECLINE_RETRY_HOURS          6
COGNITION_MAX_SYMBOLS_PER_SWEEP        5
```

What works now:

- Confirming a candidate starts cognition through `discovery.confirmed`.
- Active tickers are swept without requiring the UI to open them.
- Evidence checklists are bootstrapped for old tickers.
- Open theses are explicitly due for re-evaluation after 30 minutes.
- Fresh drafts reconcile into one canonical open thesis per symbol.
- Dev cognition sweep runs every 5 minutes over up to 20 active symbols by
  default, so a larger universe is not starved behind a five-symbol batch.

Current gaps:

- #128: the sweep is bounded and passive; it is not yet a full freshness SLA.
- #136: evidence requirements exist, but fetch actions are not a full per-source
  acquisition FSM yet.
- #130: product/theme web retrieval is missing, so the LLM cannot fetch external
  articles when local evidence is thin.
- #93: normalized evidence items are still missing; context/thesis use raw table
  slices rather than a first-class fact layer.

Selected-symbol status now exposes the first slice of #128:

```text
symbol overview
  -> /api/brain-status?symbol=MU
  -> source freshness
  -> evidence rows/open/blocking/due
  -> context age
  -> thesis age
  -> open attention count
  -> deterministic status + next_action
```

That endpoint is intentionally derived from existing tables. It does not
mutate state and it does not replace the orchestrator. Its job is to make the
brain loop legible: every ticker should say `fresh`, `due`, `stale`,
`waiting_on_evidence`, or `blocked`, plus the next system action. The remaining
#128 work is to make the same decision object drive active source fetches and
cognition jobs instead of only explaining what should happen next.

## Thesis Loop

The thesis is the current standing view. A symbol should not accumulate multiple
competing open theses.

```text
latest context + evidence + prior thesis
        |
        v
draft/reconcile
        |
        v
one open thesis per symbol
        |
        v
thesis_version_history
        |
        v
operator sees:
  current thesis
  retired thesis history
  declined attempts
```

State path:

```text
forming
  -> building_conviction
  -> armed
  -> actionable
  -> position_open
  -> exiting
  -> closed

any state -> disqualified
```

What works now:

- The DB enforces one non-closed/non-disqualified thesis per symbol.
- Duplicate open theses were retired into history.
- The UI shows one current thesis and a separate retired-history section.
- Consensus crossings without a thesis now create `thesis_incomplete`
  attention instead of fake `thesis.updated` events.

Current gaps:

- #141: reconciliation history needs a clearer operator-facing timeline.
- #96: theses should declare known unknowns.
- #97: stale evidence should reduce confidence, not just show as a warning.
- #90: separate system confidence from human conviction.
- #13: challenge pass needs clearer surfaced adversarial flags.

## Evaluation And Safety Loops

These loops run after a thesis exists.

```text
condition evaluator
  every EVAL_INTERVAL_SECS
  reads v_condition
  marks condition status

staler
  every STALER_INTERVAL_SECS
  detects stale/deadline problems
  emits warning attention/events

consensus
  every CONSENSUS_INTERVAL_SECS
  scores active tickers
  if open thesis exists:
    thesis.updated / thesis.fulfilled
  if no open thesis exists:
    thesis_incomplete attention + cognition kickoff

goalpost
  on thesis.updated
  checks invalidation weakening
  emits risk.warning if goalposts moved

risk
  on thesis.actionable
  checks position/risk constraints
  emits veto/warning

reflection
  on actionable / fulfilled / invalidated
  records predictions/outcomes/calibration
```

Current dev cadence:

```text
condition evaluator  20s dev, 60s default
staler               30s dev, 300s default
consensus            30s dev, 300s default
goalpost             event-driven
risk                 event-driven
reflection           event-driven
```

Current gaps:

- #131/#25/#5: real broker position/fill state is missing.
- #94: decision replay is missing, so we cannot yet reconstruct exactly what
  the operator saw when deciding.
- #4: validation harness is still incomplete until enough outcomes close.
- #63: inconclusive conditions need retry semantics when new data arrives.
- #64: actionable payload should carry forecast context.

## UI Loop

The UI should follow the operator's actual path:

```text
watchlist / ranked attention
        |
        v
symbol workspace
  chart
  context
  evidence
  current thesis
  alerts
  decisions
        |
        v
review packet
        |
        v
confirm / defer / reject / decide
```

Current gaps:

- #143: Brain tab for macro and sector theses.
- #89: review packet pattern for every attention-resolution flow.
- #126: explicit workflow rail.
- #82: terminology still needs simplification across UI/docs.
- #119: symbol alerts tab should not show unrelated global alerts.
- #118: live event stream in Vite dev still has tracked rough edges.
- #128: selected-symbol freshness status is visible, but the top-level
  orchestrator is still not actively scheduling every stale source.

## Current Status Summary

```text
implemented or mostly working
  data ingest loops
  discovery pool scanner
  research nominations
  evidence requirement rows
  context refresh
  thesis draft/reconcile
  one open thesis per symbol
  chart intervals + SMA/RSI display
  condition evaluator
  consensus scoring
  risk/reflection event consumers

partially working
  30-minute brain freshness
  evidence acquisition FSM
  thesis reconciliation timeline
  decision/outcome validation

implemented first slice
  selected-symbol brain status and next action

missing
  attention FSM
  macro/sector brain
  external research retrieval
  analyst price targets/recommendations
  real broker/position execution state
  review packets
  decision replay
```

## Highest-Leverage Next Work

1. #128: make freshness orchestration real.
2. #147: replace flat attention status with an FSM.
3. #143/#129: add macro and sector theses.
4. #130 and #116: improve evidence depth for real forward views.
5. #131/#25/#5: link decisions to real positions/fills.
