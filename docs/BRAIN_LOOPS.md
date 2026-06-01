# Brain Loops

This document maps the system as a set of loops. The product goal is not a
static dashboard; it is a continuously running research brain that keeps market
evidence fresh, maintains one current view per ticker, and interrupts the
operator only when judgment is required.

The higher-level product plan lives in [`docs/PRODUCT_PLAN.md`](PRODUCT_PLAN.md).
This file remains the lower-level loop map and current implementation status.

## Top-Level Brain

```text
                macro + sector/theme theses
                brain_thesis + mappings
                         |
                         v
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

The top-down layer is now first-class state:

```text
brain_thesis
  macro thesis
  sector/theme theses
  freshness + missing evidence + invalidation conditions
        |
        +-> brain_thesis_ticker
        |     role: leader/challenger/supplier/customer/beneficiary/hedge/candidate
        |
        +-> brain_thesis_watchlist
              optional explicit list mapping
```

`GET /api/brain` reads those records and shows the operator the current macro
view, active sector/theme views, linked tickers, implied watchlists, queued
discovery nominations, stale/missing parent evidence, and simple
macro/sector-direction contradictions. Ticker thesis drafting receives the
linked parent theses as input, so a symbol-level view can inherit, reject, or
explicitly contradict the parent theme instead of guessing in isolation.

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
GDELT/Bing research     on context   selected ticker product/theme queries
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

- #129: macro and sector theses exist as records, but no cognition service is
  generating/updating them yet.
- Discovery ranking reads watchlist/domain/signal quality today; it still needs
  a theme-fit score from `brain_thesis_ticker` and active parent-thesis
  direction.

## Attention Loop

Attention is not an event log. It means "the operator or system needs to make
progress here."

Implemented first slice:

```text
ready_for_review -> resolved
ready_for_review -> dismissed
ready_for_review -> operator_deferred --hidden until resurface_at--> ready_for_review
any open state -> canonical transition endpoint -> history row
```

Full target state machine:

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

- #147: backend transition helpers exist; remaining work is broader producer
  adoption for long-running work between `evaluating`, `waiting_on_data`, and
  `blocked`.
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
sync open evidence rows from current source_health/counts
        |
        v
run the same cognition pipeline for selected tickers
```

Current defaults:

```text
COGNITION_SWEEP_SECONDS                900
COGNITION_CONTEXT_MAX_AGE_HOURS        12
COGNITION_OPEN_THESIS_MAX_AGE_MINUTES  30
COGNITION_DECLINE_RETRY_HOURS          6
COGNITION_MAX_SYMBOLS_PER_SWEEP        5
COGNITION_EVIDENCE_SYNC_LIMIT          200
```

What works now:

- Confirming a candidate starts cognition through `discovery.confirmed`.
- Active tickers are swept without requiring the UI to open them.
- Evidence checklists are bootstrapped for old tickers.
- Open theses are explicitly due for re-evaluation after 30 minutes.
- Fresh drafts reconcile into one canonical open thesis per symbol.
- Dev cognition sweep runs every 5 minutes over up to 20 active symbols by
  default, so a larger universe is not starved behind a five-symbol batch.
- Each sweep now refreshes open evidence requirements from the latest source
  health before selecting cognition targets. That lets provider success,
  failures, no-new-row passes, and newly satisfied rows move tickers forward
  without waiting for an operator to open the ticker.

Current gaps:

- #128: the sweep is bounded and passive; it is not yet a full freshness SLA.
- #136: evidence requirements are synchronized from source health, but fetch
  actions are not yet a dedicated per-source task queue.
- #130: product/theme web retrieval has a first GDELT/Bing-backed slice; paid
  semantic search may still be needed if recall is too weak.
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

- #143 first slice: Brain tab exists with macro/sector theses, mappings,
  freshness, nominations, and contradictions. Remaining work is the cognition
  service that updates those parent theses from live macro/sector evidence.
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
  product/theme web research retrieval
  thesis draft/reconcile
  one open thesis per symbol
  chart intervals + SMA/RSI display
  condition evaluator
  consensus scoring
  risk/reflection event consumers

partially working
  30-minute brain freshness
  evidence acquisition FSM
  attention FSM
  thesis reconciliation timeline
  decision/outcome validation

implemented first slice
  Brain tab for macro and sector/theme theses
  first-class brain_thesis records and ticker/watchlist mappings
  ticker thesis prompt receives linked parent theses
  selected-symbol brain status and next action
  Defer 7d attention snooze/resurface
  attention producer initial state/owner adoption
  attention transition API/helper
  attention UI grouped by state/owner
  transition history for attention resolutions
  open-thesis last_evaluated_at freshness loop without no-change version churn
  evidence requirements carry source-health acquisition state
  cognition sweep refreshes open evidence rows before choosing targets

missing
  full producer adoption for attention retry/blocked transitions
  macro/sector thesis generation and scheduled re-evaluation
  paid semantic research provider if GDELT recall is insufficient
  analyst price targets/recommendations
  real broker/position execution state
  review packets
  decision replay
```

## Highest-Leverage Next Work

1. #128: make freshness orchestration real.
2. #147: finish producer adoption for waiting/retry/blocked attention states.
3. #143/#129: add macro and sector theses.
4. #116 and the remaining #130 uplift: improve evidence depth for real forward views.
5. #131/#25/#5: link decisions to real positions/fills.
