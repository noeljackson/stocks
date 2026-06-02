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
       | ticket/fill |
       | execution   |
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

The execution invariant is: every open position must be tied to a thesis and an
actual fill, not merely to an operator intention. A decision can create a trade
ticket, but only a fill creates exposure and advances the thesis to
`position_open`.

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
broad discovery_pool
UNION
active ticker/watchlist universe
UNION
benchmarks needed for regime/relative context
```

The `discovery_pool` is deliberately broad. It is the haystack fed by the
screener, not the operator work queue. Expensive data acquisition and cognition
should prioritize active tickers, watchlists, due source tasks, and ranked
nominations before walking the broad pool.

Current local dev cadence:

```text
FMP price bars          every 30m    active tickers + top candidates + benchmarks
FMP screener/pool       every 24h    broad investible pool
FMP estimates           every 30m    active tickers + top candidates
FMP analyst opinion     every 30m    active tickers + top candidates
CBOE crowd sentiment    every 30m    market-wide sentiment
FMP + Massive news      every 30m    active tickers + top candidates
GDELT/Bing research     on context   selected ticker product/theme queries
XBRL company facts      every 6h     active tickers + top candidates
EDGAR filings           every 30m    active tickers + top candidates via dynamic SEC CIK map
FRED macro              every 30m    macro series
```

The current cadence now aims source checks at the desired 30-minute freshness
SLA for sources that can move intraday. XBRL remains slower because company
facts are large and update through filings, while EDGAR is the intraday filing
watch.

Expensive per-symbol loops use a tiered deep-research universe instead of the
whole screener pool:

```text
rank 0: active ticker/watchlist universe
rank 1: proposed discovery candidates with tier 1/2 rank
limit: provider-specific max symbols per pass
```

The broad screener pool still discovers candidates, but deep data follows
promotion pressure. This prevents a 1,000-symbol pool from starving active
tickers and keeps the source freshness loop within the 30-minute target.

Current dev pass caps:

```text
FMP_PRICE_MAX_SYMBOLS_PER_PASS         125
FMP_ESTIMATES_MAX_SYMBOLS_PER_PASS     100
FMP_OPINION_MAX_SYMBOLS_PER_PASS        75
NEWS_MAX_SYMBOLS_PER_PASS              100
EDGAR_MAX_SYMBOLS_PER_PASS             100
XBRL_MAX_SYMBOLS_PER_PASS              100
```

The target brain loop is:

```text
source due
  -> respect vendor limiter
  -> create/update source_task rows
  -> fetch available rows
  -> persist source_health
  -> update evidence_requirement
  -> publish events for downstream loops
```

`source_task` is the active work queue behind evidence requirements. A missing
requirement says what is needed; source tasks say which provider/action should
run, when it is due, whether it is rate-limited, and what retry state applies.
Satisfied tasks are fresh-until `due_at`, not terminal; when a task becomes due
the owning worker may claim it and re-check the source without making the ticker
look blank. A `fetching` task whose claim is older than 15 minutes is treated as
reclaimable work; this prevents an interrupted worker from leaving evidence
acquisition stuck forever.

The thesis engine also feeds this FSM. If it declines to draft because the
context is too thin and returns structured `missing_evidence`, cognition maps
those requirements back to canonical acquisition keys:

```text
LLM missing_evidence
  price/fundamental/news/estimate/opinion/product-research need
        |
        v
canonical evidence_requirement
        |
        v
source_task rows for the owning provider
        |
        v
thesis_incomplete attention = waiting_on_data / owner=source
```

That distinction matters operationally: `waiting_on_data` means the system
should fetch or retry sources; `ready_for_review` means the system found no
edge with sufficient evidence and the operator may dismiss or keep monitoring.
Product, customer, commodity, benchmark, roadmap, and other theme-specific
requests map to `product_research`, so a DELL/AMD-style "need public adoption
evidence" decline creates web-research pressure instead of becoming inert text.

Commodity parent themes can use tradable proxies as the first price-history
slice. For example, CPER/WEAT/XME price bars can satisfy
`commodity_price_history`, while inventories, USDA reports, weather, COT, and
China demand remain separate missing evidence until direct providers exist.

Due `source_task` rows also feed the tiered deep-research universe ahead of
ordinary active tickers and proposed candidates. This means missing/stale
evidence creates provider work pressure even before every Rust ingest adapter
fully claims and completes task rows itself.

Current ownership:

```text
source_task action                owner
fmp_price_backfill                Rust ingest fmp_price loop claims/completes
fmp_news                          Rust ingest news loop claims/completes
massive_news                      Rust ingest news loop claims/completes
llm_sentiment_scoring             Rust ingest news loop scorer claims/completes when configured
fmp_analyst_estimates             Rust ingest fmp_estimates loop claims/completes
fmp_price_target_consensus        Rust ingest fmp_analyst_opinion loop claims/completes
fmp_grades_historical             Rust ingest fmp_analyst_opinion loop claims/completes
fmp_price_target_news             Rust ingest fmp_analyst_opinion loop claims/completes
sec_company_tickers_cik_lookup    Rust XBRL loop claims/completes
sec_companyfacts_xbrl             Rust XBRL loop claims/completes
sec_edgar_submissions             Rust EDGAR loop claims/completes
fred_macro                        Rust FRED loop claims/completes benchmark task
cboe_crowd_sentiment              Rust CBOE loop claims/completes benchmark task
gdelt_doc_search                  Python source_task worker
bing_news_rss_search              Python source_task worker
```

Provider backoff is now applied at planning time across every task that shares
the same provider key. If `fmp_estimates` records a 429 with `retry_after_at`,
new or recurring `fmp_price_backfill`, `fmp_news`, and analyst-opinion tasks are
held in `rate_limited` until that retry time instead of being claimed by another
worker path immediately.

Remaining gap: #128 should make future market/factor adapters claim and
complete their `source_task` rows directly. The main price/news/estimate/
opinion/XBRL/EDGAR/FRED/CBOE loops now own task rows as well as source health.

## Execution Loop

Execution is the bridge between cognition and a real portfolio.

```text
thesis.state = actionable
        |
        v
thesis_actionable attention
        |
        v
decision: enter / skip / defer / resize / exit
        |
        v
trade_ticket
  side
  instrument
  intended_size
  risk_result
        |
        v
position_fill
  manual source now
  broker source later
  qty, price, fees, filled_at
        |
        v
position current state
  basis
  delta_notional
  premium_at_risk
  realized/unrealized P/L
        |
        v
risk + evaluator + reflection
```

Current status:

```text
manual fill entry        live
append-only fills        live
position basis/P&L UI    live
broker sync              pending #25
partial exits            pending
trade-quality scoring    pending reflection extension
```

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
  quality, domain fit, active parent-theme fit, proposed tier, and
  watchlist-classifier confidence. The API includes the linked parent themes so
  the discovery tab can explain why a candidate is more than a raw alert.

Current gaps:

- #129: macro and sector theses exist as records, the cognition sweep maintains
  their coverage/freshness state, and a bounded parent-thesis LLM pass can
  rewrite parent claims from linked ticker/evidence state when evidence changes
  or the parent view is stale. Remaining work is broader factor data coverage
  for commodities, breadth, credit, and earnings.
- Discovery ranking reads `brain_thesis_ticker` fit today; it still needs
  active parent-thesis direction and contradiction penalties.

## Technical State Loop

The chart state should be computed separately from the thesis. The thesis says
what the system believes; technical state says what the chart/regime is doing
and what similar historical states did next.

```text
daily + intraday bars
        |
        v
technical package
  SMA 20/50/100/200-day distance
  RSI 14 by 30m/2h/4h/1d/1w
  time spent in current RSI/extension zone
  last 50-day and 200-day SMA crosses
  analog events with forward return/drawdown paths
        |
        v
technical_state
  constructive | extended | base_building | deteriorating | unknown
        |
        +-> thesis prompt context
        +-> chat analyst context
        +-> attention/risk review
        +-> decision timing quality
```

This avoids turning `forecast.direction = up` into an implied entry. ENTG can be
fundamentally bullish while still technically extended above the 200-day SMA.
The correct product output is both facts at once.

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
  open thesis is older than COGNITION_OPEN_THESIS_MAX_AGE_MINUTES
  no context exists
  context is missing market data
  evidence checklist is missing
  missing evidence retry is due
  context is older than COGNITION_CONTEXT_MAX_AGE_HOURS
  evidence became satisfied after a decline
  no-thesis decline is older than COGNITION_DECLINE_RETRY_HOURS
        |
        v
sync open evidence rows from current source_health/counts
        |
        v
run the same cognition pipeline for selected tickers
        |
        v
write cognition_run with status, reason, blockers, retry
```

Current defaults:

```text
COGNITION_SWEEP_SECONDS                300
COGNITION_CONTEXT_MAX_AGE_HOURS        12
COGNITION_OPEN_THESIS_MAX_AGE_MINUTES  30
COGNITION_DECLINE_RETRY_HOURS          6
COGNITION_MAX_SYMBOLS_PER_SWEEP        20
COGNITION_MIN_SYMBOLS_PER_SWEEP        20
COGNITION_EVIDENCE_SYNC_LIMIT          200
```

The worker treats the freshness target as a guardrail: nonzero sweep intervals
are capped at half the open-thesis freshness window, with an upper cap of 300s
and lower cap of 60s. Batch size is floored by
`COGNITION_MIN_SYMBOLS_PER_SWEEP`. This prevents stale environment config like
`900s/5 symbols` from silently breaking the 30-minute product SLA.

Sweep priority is opinion-first. A stale open thesis outranks broad bootstrap
work, because the current standing view is what the operator is relying on.
Each scheduled run records a `sweep_reason` such as `open_thesis_due`,
`context_missing`, or `evidence_retry_due` into the pipeline source reference so
the UI/audit trail can explain why the symbol was touched.

Every event-driven or scheduled attempt writes `cognition_run`:

```text
cognition_run
  symbol
  trigger
  sweep_reason
  status = running | blocked_on_evidence | declined | drafted |
           reconciled | no_change | failed
  reason
  context_version
  thesis_id + thesis_classification
  evidence_open_count + evidence_blocking_count
  started_at + finished_at + next_retry_at
```

This is the durable "what did the brain just do?" ledger. Context and thesis
timestamps show what changed; `cognition_run` shows that cognition actually ran,
why it ran, what blocked it, and when it will retry.

```text
cognition target priority
  0 open thesis due for re-evaluation
  1 missing context
  2 context exists but market context is blank
  3 evidence checklist missing
  4 evidence retry due for a no-thesis symbol
  5 stale context
  6 older decline retry / maintenance
```

What works now:

- Confirming a candidate starts cognition through `discovery.confirmed`.
- Active tickers are swept without requiring the UI to open them.
- Evidence checklists are bootstrapped for old tickers.
- Open theses are explicitly due for re-evaluation after 30 minutes.
- Fresh drafts reconcile into one canonical open thesis per symbol.
- Draft/reconcile runs attach the latest normalized evidence facts to the
  thesis via `thesis_evidence`, so the current view can be inspected back to
  concrete source rows.
- Malformed draft JSON is retried through the same audited prompt path instead
  of crashing the cognition update loop on the first bad model response.
- Dev cognition sweep runs every 5 minutes over up to 20 active symbols by
  default, so a larger universe is not starved behind a five-symbol batch.
- Each sweep now refreshes open evidence requirements from the latest source
  health before selecting cognition targets. That lets provider success,
  failures, no-new-row passes, and newly satisfied rows move tickers forward
  without waiting for an operator to open the ticker.
- Every pipeline attempt is visible through `cognition_run`, Diagnostics, and
  the selected-symbol Brain card. Failed, blocked, declined, no-change, drafted,
  and reconciled runs are all recorded.

Current gaps:

- #128: source loops and cognition still coordinate through `source_task` rather
  than a single top-level controller process, but the run ledger now makes that
  coordination observable.
- #128: provider-wide retry gates now pause source tasks, and due source tasks
  move their symbols to the front of the expensive ingest scan universe. The
  FMP price, estimates, analyst-opinion, news, XBRL, EDGAR, FRED, and CBOE
  loops now claim/complete their source tasks directly.
- #136: evidence requirements and source tasks are synchronized from source
  health. Remaining producer adoption work is for future factor/commodity
  adapters and attention retry/blocked transitions.
- #130: product/theme web retrieval has a first GDELT/Bing-backed slice; paid
  semantic search may still be needed if recall is too weak.
- #93: normalized evidence items now have a first slice. News, estimate
  revisions, and analyst price-target events are backfilled and new ingest rows
  create `evidence_item` records. Context and thesis prompts now receive those
  facts directly, thesis drafting/reconciliation records `thesis_evidence`
  links for the current fact set, and the thesis UI shows those linked facts.
  The remaining work is producer coverage for filings, price action,
  regime/context shifts, and more selective LLM-picked support/contradiction
  labels.

Selected-symbol status now exposes the first slice of #128:

```text
symbol overview
  -> /api/brain-status?symbol=MU
  -> source freshness
     - last checked or evaluated
     - last changed
     - retry_after when rate-limited/failed
     - source-specific detail: session coverage, latest publication, version/state
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
- The UI shows linked evidence facts for the current thesis, ordered by
  evidence weight and recency.
- Consensus crossings without a thesis now create `thesis_incomplete`
  attention instead of fake `thesis.updated` events.
- The draft-thesis prompt now asks for `forecast.technical_state` so fresh
  thesis runs can separate direction from chart regime.
- `/api/technical-state?symbol=SYMBOL` now serves a derived technical package
  from stored bars, and the symbol detail panel exposes it as a Technical tab.

Current gaps:

- The technical-state package is not persisted yet and is not automatically
  attached to thesis/context/chat prompt inputs.
- #141: reconciliation history needs a clearer operator-facing timeline.
- #96: theses should declare known unknowns.
- #97: stale evidence should reduce confidence, not just show as a warning.
- #90: separate system confidence from human conviction.
- #13: challenge pass needs clearer surfaced adversarial flags.

## Chat Analyst Loop

The chat analyst loop is for operator questions over current system state. It
should not create a competing thesis path.

```text
operator question
        |
        v
scope router
  symbol | theme | macro | technical | decision
        |
        v
load evidence package
  brain_thesis
  ticker_context
  technical_state
  current thesis + version history
  evidence_items
  evidence_requirement/source_task
  decision/position state if relevant
        |
        v
chat-analyst prompt
        |
        +-> answer with citations
        +-> create missing source tasks when evidence is absent
        +-> flag material thesis changes for reconciliation
        +-> flag action implications for attention/review packet
```

Rules:

```text
no answer without evidence refs or explicit missing-data statement
no silent thesis mutation
no direct trade decision
all calls through prompt registry with llm_invocation audit
```

Current implementation:

- `POST /api/chat-analyst` loads the symbol evidence package, including
  `technical_state`, current thesis/history, parent brain theses, evidence
  facts, requirements/source tasks, decisions, and positions.
- The route invokes `prompts/chat-analyst.md` through the audited prompt
  registry and records `llm_invocation`.
- Slow provider calls time out and return a deterministic evidence-only
  fallback after recording a timeout `llm_invocation` row with the prompt hash.
- Requested missing evidence is converted into `evidence_requirement` and
  `source_task` rows for the selected symbol.
- The right-side workspace has an Analyst tab for symbol, technical, and
  decision questions.

Current gaps:

- Theme/macro-only analyst questions are package-light compared with symbol
  questions.
- Material thesis changes are returned as `thesis_impact`; they do not yet
  auto-create reconciliation packets.
- Action implications are returned as `attention_request`; they do not yet
  auto-create review packets.

## Evaluation And Safety Loops

These loops run after a thesis exists.

```text
condition evaluator
  every EVAL_INTERVAL_SECS
  reads v_condition
  evaluates pending and inconclusive conditions
  marks satisfied/refuted/inconclusive status

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
  freshness, nominations, contradictions, and maintainer coverage. The
  cognition sweep evaluates parent rows from linked ticker/source coverage and
  records material changes in `brain_thesis_version_history`.
- #129 remaining work: parent theses now have a bounded LLM rewrite pass from
  normalized linked evidence; the remaining gap is broader macro/factor data
  coverage and more direct commodity/breadth feeds.
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
  parent brain maintainer evaluates source/ticker coverage on cognition sweep
  parent thesis beneficiary/proxy symbols become active ticker mappings
  commodity proxy roles identify CPER/WEAT/XME as factor-price expressions
  commodity proxy price bars can satisfy parent commodity_price_history gaps
  ticker thesis prompt receives linked parent theses
  bounded parent-thesis LLM pass rewrites macro/sector/theme claims when due
  selected-symbol brain status and next action
  Defer 7d attention snooze/resurface
  attention producer initial state/owner adoption
  attention transition API/helper
  attention UI grouped by state/owner
  transition history for attention resolutions
  open-thesis last_evaluated_at freshness loop without no-change version churn
  evidence requirements carry source-health acquisition state
  source_task rows track acquisition action/state/due time
  satisfied source_task rows requeue when freshness is due
  provider-wide rate-limit pauses are applied to all matching source tasks
  due source_task symbols are prioritized by expensive ingest loops
  FMP price loop claims/completes fmp_price_backfill source tasks
  FMP estimates loop claims/completes fmp_analyst_estimates source tasks
  FMP analyst opinion loop claims/completes consensus, grades, and target-news source tasks
  news loop claims/completes FMP, Massive, and configured LLM sentiment source tasks
  XBRL loop claims/completes SEC CIK lookup and companyfacts source tasks
  EDGAR loop claims/completes SEC submission source tasks
  FRED loop claims/completes macro benchmark source task
  CBOE loop claims/completes crowd sentiment benchmark source task
  active ticker evidence sync bootstraps newly added requirement keys
  Python source_task worker claims and runs due web research tasks
  cognition sweep refreshes open evidence rows before choosing targets

missing
  full producer adoption for attention retry/blocked transitions
  broader macro/factor/commodity data coverage for parent thesis generation
  persisted technical_state history and prompt-loop ingestion
  chat analyst routed prompt loop
  paid semantic research provider if GDELT recall is insufficient
  real broker/position execution state
  review packets
  decision replay
```

## Highest-Leverage Next Work

1. #128: make freshness orchestration real.
2. #147: finish producer adoption for waiting/retry/blocked attention states.
3. #143/#129: add macro and sector theses.
4. Remaining #130 uplift: improve product/theme evidence recall for real forward views.
5. #131/#25/#5: link decisions to real positions/fills.
