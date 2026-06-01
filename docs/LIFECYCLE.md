# Lifecycle Map

This is the canonical map for how a ticker moves from broad radar to context,
thesis, decision, position, and outcome. Update this document when adding a new
attention kind, state transition, event source, or operator decision surface.
The scheduler and feedback-loop view lives in
[`docs/BRAIN_LOOPS.md`](BRAIN_LOOPS.md).

## Main Flow

```text
brain_thesis                                symbol thesis
  macro + sector/theme parent views          child view / ticker expression
       │                                                   ▲
       ├────────────── theme-fit + parent context ─────────┘
       ▼
discovery_pool                              active ticker/watchlist universe
  broad radar                                  focused radar
  price/news/estimates                         richer context + thesis loop
       │                                                   │
       └──────────────┬────────────────────────────────────┘
                      ▼
              cheap raw detectors
              volume, breakout, news, estimates
                      │
          ┌───────────┴───────────┐
          │                       │
          ▼                       ▼
  research nomination             composed interpretation
  reasoned relevant names         raw facts + extension + thesis/watchlist state
          │                       │
          └───────────┬───────────┘
                      ▼
              discovery_candidate
              candidate_review attention
                      │
                      ▼
              operator confirm/reject
                      │
          ┌───────────┴───────────┐
          │                       │
          ▼                       ▼
    reject -> candidate      confirm -> watchlist
    closed, feedback         membership / ticker
    retained                 promotion
                                  │
                                  ▼
                           discovery.confirmed
                                  │
                                  ▼
                        cognition pipeline
                        evidence requirements
                        price / facts / news / estimates
                                  │
                                  ▼
                        context_maintainer
                        thesis_engine
                        sharpen
                        challenge
                        scheduled open-thesis re-evaluation
                                  │
              ┌───────────────────┴───────────────────┐
              │                                       │
              ▼                                       ▼
     ticker_context version                 honest no-edge / waiting decline
     structural/narrative/market            thesis_incomplete attention
     evidence state updated                 missing evidence visible
              │
              ▼
          thesis state machine
          forming -> building_conviction -> armed -> actionable
              │
              ▼
        thesis_actionable attention
              │
              ▼
         operator decision
         enter/exit/skip/resize + side/instrument
              │
              ▼
         trade_ticket
         intended size + risk result
              │
              ▼
         manual fill / broker fill
         append-only position_fill
              │
              ▼
           position
         actual exposure, basis, P/L
              │
              ▼
           outcome
              │
              ▼
        reflection/calibration
        lead time, Brier, prompt/signal learning
```

`research_nomination` candidates use the same `candidate_review` attention kind
as signal candidates. The difference is semantic: a nomination says "this name
belongs in the monitored universe for explicit theme/business/evidence reasons,"
while signal candidates say "new market/evidence behavior crossed a threshold."
Both resolve through confirm/reject.

The thesis object is a state machine, not a draft archive. A symbol may have at
most one open thesis (`forming` through `exiting`) at a time; new evidence
refreshes reconcile into that thesis and append version history. Duplicate
open theses are a data integrity bug, and the database rejects them.

Macro/sector/theme views are separate `brain_thesis` records. They do not
replace ticker theses and they do not make a ticker investable by themselves.
Their job is to explain why a group of tickers is worth monitoring, what parent
evidence is stale or missing, which tickers are candidate expressions, and
which parent conditions would invalidate the theme. Ticker thesis drafting
receives linked parent theses and must either use them as context, reject them
for the specific symbol, or call out contradictions.

## Attention Contract

Attention is not an event log. Attention means "the operator needs to judge
something now." Background services may append many events without creating
attention.

| Kind | Source | Meaning | Resolver |
| --- | --- | --- | --- |
| `candidate_review` | discovery | A composed discovery interpretation or reasoned research nomination deserves confirm/reject. | confirm or reject candidate |
| `thesis_incomplete` | cognition | Context was refreshed but thesis engine declined to invent an edge, or blocking evidence is still missing. | draft thesis / dismiss |
| `thesis_actionable` | thesis transition | A thesis reached actionable and needs a human decision. | record decision |
| `risk_review` | risk | Proposed/recorded intent hit risk warnings or vetoes. | acknowledge / adjust |
| `context_stale` | staler | A thesis depends on stale context. | refresh context |
| `invalidation_hit` | evaluator/staler | Evidence may refute a thesis condition. | review transition/decision |
| `outcome_ready` | reflection | A forecast horizon is ready to score. | score outcome |

## Decision And Execution Contract

Decisions and executions are separate concepts:

```text
decision
  human says enter, exit, skip, resize, confirm, reject, or defer

trade_ticket
  proposed expression of the decision
  thesis_id, symbol, side, instrument, intended_size, risk_result

position_fill
  actual execution fact
  manual now, broker sync later
  append-only fill_id, position_id, qty, price, fees, filled_at

position
  current exposure state derived from fills
  basis, delta_notional, premium_at_risk, opened_at, closed_at, realized_pnl
```

The key invariant is:

```text
thesis actionable
  -> decision may create a proposed/accepted ticket
  -> only an actual fill creates a position
  -> only after that fill may the thesis move to position_open
```

This matters because forecast quality, decision quality, and trade quality are
different. A good thesis that was skipped, a bad thesis that was rejected, and
a good thesis that was filled at a bad price must remain distinguishable in
reflection.

### Attention State Machine

`attention_item.status` is the coarse terminal state (`open`, `resolved`,
`dismissed`). `attention_item.fsm_state` is the operational state the product
uses to explain ownership and retry/resurface behavior.

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

Each state transition appends `attention_state_history`:

```text
attention_id
from_state -> to_state
owner: system | operator | source | cognition | risk
reason
next_retry_at
resurface_at
source_ref
transitioned_at
```

The visible attention queue is not "all rows with `status = open`." It is:

```text
open attention
minus operator_deferred rows whose resurface_at is still in the future
plus resurfaced rows moved back to ready_for_review once resurface_at is due
```

This makes `Defer 7d` a real snooze instead of a terminal dismissal. Deferred
items remain auditable in diagnostics and state history while staying out of
the operator's active queue until they are due.

Producers must set an initial `fsm_state`, `owner`, and `state_reason` when
creating attention. Resolvers must close attention through a transition path
that updates the coarse `status`, moves `fsm_state` to `resolved` or
`dismissed`, and appends `attention_state_history`. Updating `status` alone is
ambiguous and should be treated as a bug.

The canonical manual transition endpoint is:

```text
POST /api/attention/:id/transition
{
  "to_state": "waiting_on_data | ready_for_review | operator_deferred | actionable | resolved | dismissed | blocked",
  "owner": "system | operator | source | cognition | risk",
  "reason": "short transition reason",
  "next_retry_at": "optional timestamp",
  "resurface_at": "optional timestamp",
  "source_ref": { "links": "evidence, source health, candidate, thesis, or decision refs" }
}
```

If `to_state = operator_deferred` and no `resurface_at` is supplied, the API
defaults to a seven-day resurface.

## Discovery Composition

Raw detector firings are facts, not operator interpretations. The scanner first
collects raw facts:

```text
volume_anomaly
base_breakout
estimate_revision_velocity
news_sentiment_shift
```

Then it composes them with price extension and symbol state:

```text
raw hits
+ distance from available-window high
+ distance from 200-day SMA, or the longest available SMA when fewer than 200 daily bars exist
+ RSI
+ open/actionable thesis state
+ watchlist membership
-> interpretation
```

Current deterministic interpretations:

| Interpretation | Meaning |
| --- | --- |
| `early_accumulation` | Evidence suggests a name may be moving before broad consensus. |
| `breakout_confirmation` | A base breakout is confirmed by supporting activity. |
| `extended_momentum` | A volume event is happening after the move is already stretched. |
| `consensus_arrival` | Price extension plus positive news/estimate activity suggests crowd arrival. |
| `possible_exhaustion` | Negative evidence is appearing while the name is extended. |
| `existing_thesis_trigger` | Signal belongs to an already actionable thesis and routes to `thesis_actionable`, not generic discovery. |

This avoids treating "volume spike at/near highs" as the same thing as "early
discovery." It is still deterministic and auditable: raw signals are preserved
in `attention_item.source_ref.raw_signals`.

## Context And Thesis

Confirming a discovery candidate publishes `discovery.confirmed`. The cognition
consumer handles that message:

```text
discovery.confirmed
  -> create/update evidence_requirement state
  -> refresh ticker_context
  -> draft thesis when blocking evidence is present and no active thesis exists
  -> sharpen thesis
  -> challenge thesis
```

Missing evidence is first-class state, not an answer. Context refresh records
whether the required price, fundamentals, news, and estimate inputs exist for a
symbol. Missing rows are stored in `evidence_requirement` with a source type,
reason, priority, blocking state, attempt count, next retry time, and source
reference. Blocking missing evidence stops the context/thesis LLM path and
leaves a visible `thesis_incomplete` item that says the system is waiting for
acquisition.

Evidence requirements also carry the latest source-health snapshot for the
feeds that can satisfy them. A missing requirement should explain whether the
source has not run, is currently fetching, is rate-limited/failed, produced no
new rows, or succeeded but produced no relevant rows for that symbol. This is
the difference between "no data exists" and "the acquisition loop is still
working." Diagnostics aggregate the same acquisition reasons, so a global
problem like rate limits is visible without opening each ticker.

The cognition sweep refreshes open evidence requirements before selecting
context/thesis targets. That refresh is cheap: it re-counts local rows, reads
`source_health`, updates `blocking_state`/`next_retry_at`/`last_error`, and marks
requirements satisfied when rows have arrived. It does not call an LLM. If a
previous no-thesis decline is older than the newly satisfied evidence, the same
sweep can retry the ticker.

Product/theme web research is a high-priority evidence requirement. Context
refresh runs targeted retrieval before the LLM pass, persists rows in
`research_evidence`, and passes those sources into the narrative band. A thesis
decline should not say "no public data" for a named product/theme unless the
`product_research` requirement shows the retrieval state.

The thesis engine may still decline after evidence is present. A decline is not
failure when there is no measurable edge. Example: a mega-cap can have fresh
context and still get no actionable thesis if the facts are already consensus
and there is no undiffused edge. If the context is substantial but no entry edge
exists, the intended behavior is a monitoring thesis: one current standing view,
not a blank symbol. If an active thesis already exists, fresh draft output
updates that canonical row and appends a `thesis_version_history`
reconciliation event instead of creating a second active thesis. Reconciliation
events are classified as `confirmed_existing_view`, `strengthened_view`,
`weakened_view`, `material_change`, `invalidates_existing_view`, or
`no_change`.

Consensus threshold crossings are validation and attention signals, not
automatic thesis lifecycle progress. When a symbol crosses consensus without an
open thesis, the system records `thesis_incomplete` attention and kicks
cognition. Only crossings attached to a real open thesis may emit
`thesis.updated` or `thesis.fulfilled`.

The cognition service also runs a bounded maintenance sweep over active tickers.
This is what makes the system continuously work instead of depending on manual
refreshes or UI tab opens:

```text
active ticker with no/stale context
  -> refresh ticker_context
  -> if no open thesis, draft monitoring/actionable thesis or record decline

active ticker with no thesis and an old decline
  -> retry after COGNITION_DECLINE_RETRY_HOURS

active ticker with due missing evidence
  -> refresh context/evidence
  -> retry thesis if evidence is now sufficient

active ticker with no evidence checklist rows
  -> initialize evidence_requirement state
  -> refresh context/evidence

active ticker where evidence became satisfied after a decline
  -> retry thesis immediately in the next bounded sweep
```

For open theses, `updated_at` means the thesis content/version changed.
`last_evaluated_at` means the brain loop re-checked the thesis against fresh
context. A no-change re-evaluation updates `last_evaluated_at` only, so the
30-minute loop can prove it ran without creating fake thesis versions.

Runtime knobs:
- `COGNITION_SWEEP_SECONDS` (default 300; set 0 to disable)
- `COGNITION_CONTEXT_MAX_AGE_HOURS` (default 12)
- `COGNITION_OPEN_THESIS_MAX_AGE_MINUTES` (default 30)
- `COGNITION_DECLINE_RETRY_HOURS` (default 6)
- `COGNITION_MAX_SYMBOLS_PER_SWEEP` (default 20)
- `COGNITION_MIN_SYMBOLS_PER_SWEEP` (default 20)

The worker caps nonzero sweep intervals at half the open-thesis freshness window
and floors batch size at `COGNITION_MIN_SYMBOLS_PER_SWEEP`, so stale runtime
config cannot silently starve the open-thesis update loop.

## State Ownership

Use these ownership boundaries when changing behavior:

```text
raw detectors       -> src/discovery/signals.rs
interpretation      -> src/discovery/composer.rs
candidate storage   -> discovery_candidate
attention storage   -> attention_item
context memory      -> ticker_context
parent brain        -> brain_thesis + brain_thesis_ticker/watchlist
thesis lifecycle    -> thesis + thesis_state_history
decision log        -> decision
position state      -> position
outcome scoring     -> outcome
```

Append-only tables remain append-only. State changes go through the canonical
service/API path for that object.
