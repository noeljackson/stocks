# Product Plan

This is the target product design for the trading intelligence system. It
connects the market brain, discovery, cognition, thesis, attention, decision,
position, and validation loops into one operating model.

The product is not a screener, dashboard, or chat wrapper. It is a continuously
running research brain for one operator. It should keep evidence fresh, build
current views, surface only useful judgment points, and learn from outcomes.

## Product Thesis

```text
fresh evidence
  -> explicit market views
  -> ticker-level standing thesis
  -> risk-aware human decision
  -> position/outcome tracking
  -> calibration and better future scanning
```

The edge is not "technology." The edge is finding evidence-backed opportunities
before they become obvious. Technology infrastructure is a current strength,
but copper, wheat, financials, staples, energy, healthcare, credit, rates, and
any other liquid market matter when they can produce a falsifiable money-making
view.

The product should answer five questions at all times:

```text
What is happening in markets?
Why does the system care?
Which symbols express it?
What is the current standing view for each symbol?
What decision, if any, does the human need to make now?
```

## Object Model

```text
market_factor
  macro, rate, credit, commodity, currency, sector, theme

brain_thesis
  current parent view for a factor/sector/theme
  examples: macro regime, copper, wheat, AI compute, regional banks

symbol
  tradable equity/ETF/proxy or monitored company

evidence_requirement
  what the system needs before it may claim anything

evidence_item
  normalized fact with source, timestamp, symbol/factor, and confidence

ticker_context
  maintained memory for one symbol
  structural, narrative, market bands

technical_state
  current multi-timeframe chart regime and historical analogs
  examples: 200-day SMA cross history, RSI 14 percentile/duration, extension state

thesis
  one current standing view for a symbol
  monitoring, actionable, position_open, exiting, closed, disqualified

attention_item
  a judgment/work item with owner and state

decision
  human action/inaction recorded at a point in time

trade_ticket
  proposed expression with side, instrument, intended size, and risk result

position_fill
  actual execution record; manual first, broker sync later

position
  actual exposure tied to a thesis

outcome
  forward score of forecast, decision quality, and lead-time
```

Hard invariants:

```text
one open thesis per symbol
no blank active ticker without a reason
no data gap without retry/acquisition state
no LLM claim without evidence references
no operator interruption without a needed judgment
no trade decision without risk context
no position_open thesis without an actual fill
no outcome without preserving what was known at decision time
no bullish/bearish thesis treated as an entry signal without technical/risk state
```

## Brain Hierarchy

The brain is layered. Parent views explain the market; ticker views express
them.

```text
Macro Brain
  rates
  liquidity
  inflation
  growth
  credit
  risk appetite
       |
       v
Factor / Sector / Theme Brain
  copper and industrial metals
  wheat and agriculture inflation
  AI compute infrastructure
  memory and HBM
  financials and credit
  energy and geopolitics
  staples and pricing power
       |
       v
Expression Layer
  equities
  ETFs/proxies
  suppliers
  customers
  beneficiaries
  hedges
       |
       v
Ticker Thesis
  current symbol-level view
  conditions
  triggers
  invalidation
  trade expression
```

Parent theses do not automatically make ticker theses investable. A ticker
thesis must either inherit the parent view, reject it for symbol-specific
reasons, or explicitly contradict it.

Example:

```text
Wheat thesis: supply shock may support crop prices and food inflation
  -> beneficiaries: WEAT, ADM, BG, MOS, CF, DE
  -> harmed names: food manufacturers with weak pricing power
  -> macro effect: inflation/rates pressure
  -> ticker thesis: ADM only if evidence shows margin/volume leverage
```

## End-To-End Loop

```text
                 +----------------------+
                 |  scheduler / clock   |
                 +----------+-----------+
                            |
                            v
                 +----------------------+
                 |  source acquisition  |
                 |  rate limit + retry  |
                 +----------+-----------+
                            |
                            v
                 +----------------------+
                 |  evidence layer      |
                 |  requirements/items  |
                 +----------+-----------+
                            |
            +---------------+----------------+
            |                                |
            v                                v
 +----------------------+          +----------------------+
 | market brain         |          | discovery            |
 | macro/factor/theme   |          | signals/nominations  |
 +----------+-----------+          +----------+-----------+
            |                                |
            +---------------+----------------+
                            |
                            v
                 +----------------------+
                 | cognition            |
                 | context + thesis     |
                 +----------+-----------+
                            |
            +---------------+----------------+
            |                                |
            v                                v
 +----------------------+          +----------------------+
 | attention            |          | evaluator/staler     |
 | review/decision work |          | conditions/freshness |
 +----------+-----------+          +----------+-----------+
            |                                |
            +---------------+----------------+
                            |
                            v
                 +----------------------+
                 | decision + risk      |
                 +----------+-----------+
                            |
                            v
                 +----------------------+
                 | ticket + fill        |
                 | manual/broker exec   |
                 +----------+-----------+
                            |
                            v
                 +----------------------+
                 | position + outcome   |
                 +----------+-----------+
                            |
                            v
                 +----------------------+
                 | reflection learning  |
                 +----------------------+
```

Every loop must be observable. The UI should show the current state, owner,
freshness, next retry, and reason instead of making the operator infer why
nothing happened.

## Execution Loop

The system is not useful if it only records theoretical decisions. It must know
whether the operator actually owns exposure, at what basis, and against which
thesis.

```text
actionable thesis
  -> thesis_actionable attention
  -> operator opens decision drawer
  -> side + instrument + intended exposure
  -> deterministic risk evaluation against open positions and portfolio frame
  -> trade_ticket
  -> manual fill now / broker fill later
  -> append-only position_fill
  -> position current state
  -> risk overlay uses real open exposure
  -> evaluator watches invalidation and fulfillment conditions
  -> exit decision + fill
  -> closed position
  -> outcome/reflection separates thesis quality from trade quality
```

Manual fills are the first bridge. Broker sync is still the target execution
source, but the product must already behave like a real book: filled entries
create positions, open positions show basis and P/L, and exits close exposure.

## Scheduler And Acquisition

The scheduler is the system's metabolism. It should not be a set of independent
services that happen to run. It should own freshness, pacing, and retries.

```text
source_task
  source_type
  scope: factor | symbol | universe | benchmark
  target_id
  due_at
  priority
  provider
  limiter_key
  attempts
  next_retry_at
  state: queued | fetching | satisfied | no_rows | rate_limited | failed | blocked
```

`satisfied` means fresh until `due_at`. It does not mean the system is done
checking that source. When `due_at` passes, the task is eligible to be claimed
again so missing or stale evidence becomes work automatically.

Target freshness:

```text
price bars             <= 30m where provider supports it
intraday chart bars    on demand + cache
news                   <= 30m
analyst estimates      <= 30m to 1d depending on vendor cadence
ratings/price targets  <= 30m when wired
EDGAR filings          <= 30m
XBRL company facts     <= 6h or filing-triggered
FRED macro             <= 30m to 1d depending on series
commodity prices       <= 30m to 1d depending on source
USDA/weather/COT       source-native cadence
```

Provider rate limits are scheduler state, not random failures:

```text
429 / Retry-After
  -> pause provider queue
  -> mark affected tasks rate_limited
  -> set next_retry_at
  -> keep other providers running
  -> show diagnostics globally and per symbol/factor
```

If a ticker says "no data," the product must distinguish:

```text
not requested yet
queued
fetching
provider rate-limited
provider failed
provider returned no rows
rows exist but not enough for requirement
requirement satisfied
```

## Evidence Layer

The evidence layer converts raw vendor rows into facts the brain can reason
over.

```text
raw source row
  -> normalized evidence_item
  -> thesis_evidence link when a view uses or updates from it
  -> linked to symbol/factor/theme
  -> satisfies or weakens evidence_requirement
  -> available to context/thesis prompts
```

Current implementation: `evidence_item` exists for discrete news facts,
estimate revisions, analyst price-target events, SEC filing metadata, discovery
price-action signals, macro-regime changes, CBOE crowd sentiment,
product/theme research, and context shifts. It is backfilled from existing
rows, updated by ingest, and visible in the selected-symbol Evidence tab.
Context and thesis prompts receive the normalized fact stream before raw vendor
details, so claims can be tied back to source facts. Draft/reconcile runs attach
recent facts to the current thesis through `thesis_evidence`, and the thesis
detail panel shows those linked facts next to the rationale.

Evidence requirement examples:

```text
price_history
  Need daily OHLCV bars before evaluating technical setup.

recent_news
  Need recent narrative evidence before claiming the market has new information.

analyst_estimates
  Need consensus/revision snapshots before evaluating estimate drift.

commodity_price_history
  Need copper/wheat price history before building commodity-factor theses.

inventory_or_supply_data
  Need inventories, crop reports, weather, or production data before claiming
  commodity tightness.
```

The UI wording should avoid contradictions. `blocking` is priority/severity;
`missing`, `waiting`, `satisfied`, and `failed` are state.

## Market Brain Loop

The market brain maintains parent views.

```text
macro/factor evidence
  rates, credit, inflation, commodities, breadth, earnings revisions
        |
        v
brain thesis update
  direction: bull | bear | neutral | mixed
  state: missing | forming | actionable | invalidated | stale
  missing evidence
  invalidation conditions
  affected watchlists/tickers
        |
        v
downstream context
  ticker thesis prompt receives parent views
  discovery ranking receives theme fit
```

The brain should have one current view per major factor or theme, with timeline
history. It should not create many competing parent theses unless they are
separate factors.

Minimum parent thesis set:

```text
macro regime
rates/liquidity
credit/financial conditions
inflation/commodities
copper/industrial metals
wheat/agriculture/food inflation
AI compute infrastructure
memory/HBM
optical/networking
energy
financials
staples
healthcare
consumer cycle
```

## Discovery Loop

Discovery answers: "What should the brain inspect next?"

```text
broad liquid universe / discovery_pool
  -> cheap evidence scan
  -> signal composition
  -> ranking
  -> candidate_review attention only when judgment is useful
```

The discovery pool is intentionally large. It is the haystack, not the work
queue. The operator-facing work queue is ranked nominations and attention:

```text
discovery_pool
  broad FMP screener inventory; many names are expected

discovery_candidate / nomination
  ranked reason to inspect a subset

ticker/watchlist universe
  active operating set with freshness, context, thesis, and decisions
```

Signals are facts:

```text
volume anomaly
base breakout
estimate revision velocity
rating/price-target change
news burst/sentiment shift
relative strength
commodity/factor exposure change
parent theme contradiction
```

Composition turns facts into interpretation:

```text
raw signals
+ price extension
+ SMA period-specific context, e.g. 20/50/200-day
+ RSI
+ news/estimate/fundamental freshness
+ parent brain thesis direction
+ watchlist/thesis/position state
-> ranked interpretation
```

Discovery categories:

```text
proactive_research_nomination
  queued because the business/factor deserves first-pass cognition

signal_candidate
  queued because new market/evidence behavior crossed a threshold

existing_thesis_update
  routed to the current thesis instead of generic discovery

consensus_or_exhaustion
  routed to validation/exit review
```

The queue should not be a flat list of "research nomination medium 44." It
should show:

```text
rank
why queued
source signals
parent theme/factor fit
freshness
what confirmation does
what rejection teaches
```

## Technical Analysis Loop

Technical analysis is a separate evidence package, not a replacement for the
thesis.

```text
daily + intraday bars
  -> SMA ribbon and distance by window
  -> RSI by interval
  -> extension/base/trend regime
  -> historical analogs
       last 200-day SMA crosses
       prior RSI 14 regimes like today
       time since RSI entered this zone on 1d/4h/2h/30m
       forward returns and drawdowns after similar states
  -> technical_state consumed by thesis, analyst chat, attention, and decisions
```

This prevents the ENTG failure mode:

```text
thesis direction: bullish
technical_state: extended +26% vs 200-day SMA
decision implication: standing view can remain bullish, but timing quality is poor
```

Target technical-state fields:

```text
symbol
as_of
intervals: 30m, 2h, 4h, 1d, 1w
sma: 20/50/100/200-day for daily context
rsi14 by interval
distance from 52-week high/low
last_crosses: 50-day/200-day SMA up/down
analog_events: similar RSI/extension regimes with forward paths
state: constructive | extended | base_building | deteriorating | unknown
```

Current implementation: `/api/technical-state?symbol=SYMBOL` derives the
package on demand from stored daily and intraday bars and the symbol detail
panel shows it as a first-class Technical tab. It is not persisted yet, and the
thesis/context prompt loops do not automatically ingest this API package yet.

Ticker theses should reference `technical_state`, but they should not bury this
analysis in prose. A bullish thesis can coexist with an extended chart; an
actionable decision requires both thesis evidence and acceptable timing/risk.

## Cognition Loop

Cognition is the symbol brain. It creates and maintains current views.

```text
symbol selected by event or scheduler
  -> refresh evidence requirements
  -> fetch due evidence where possible
  -> react immediately when normalized evidence_items changed
  -> refresh context bands
  -> draft/reconcile thesis
  -> sharpen conditions
  -> challenge weak claims
  -> persist current view or visible decline reason
  -> write cognition_run ledger row
```

No active ticker should be blank:

```text
has actionable edge
  -> current actionable/armed thesis

has real context but no entry edge
  -> monitoring thesis

has insufficient evidence
  -> thesis_incomplete with acquisition state and retry

has no measurable edge after fresh evidence
  -> declined attempt with reason, evidence age, retry policy
```

Open theses are on a maintenance loop:

```text
open thesis older than freshness target
or normalized evidence newer than thesis evaluation
  -> outrank no-context and broad evidence-bootstrap work in the scheduler
  -> re-evaluate against latest context
  -> no_change: update last_evaluated_at only
  -> changed: append thesis_version_history
  -> contradiction: produce attention
  -> invalidated: transition/disqualification review
```

Thesis substance separates two questions:

```text
structural substance
  -> are forecast, conditions, invalidation, trigger, sizing, and fulfillment present?

input freshness
  -> are market regime/crowd, ticker context, estimates, and news current enough
     to trust the claim?
```

The thesis detail response carries `substance.freshness_score`,
`substance.freshness_status`, component scores, penalties, and an optional
`confidence_cap`. A thesis can be structurally complete while freshness caps it
at medium/low. Promotion into `actionable` is blocked when the freshness score
is below the high-confidence threshold, so stale evidence cannot silently become
a trade recommendation.

The system should never create multiple open theses for MU or any other symbol.
New facts reconcile into the canonical current thesis and show as a timeline.

Every cognition attempt is durable:

```text
cognition_run
  trigger + sweep_reason
  status + reason
  evidence blockers + retry time
  context version
  thesis id + reconciliation classification
```

Diagnostics and the selected-symbol Brain card show this ledger so a stale or
blank ticker can explain whether cognition ran, what it tried, what blocked it,
and when the next retry is due.

## Chat Analyst Loop

The chat analyst is a routed prompt loop over system state. It is not a general
chatbot and it should not become a second thesis engine.

```text
operator asks a question
  -> classify scope: symbol | theme | macro | technical | decision
  -> load current evidence package
       parent brain theses
       ticker context
       technical_state
       current thesis + history
       normalized evidence_items
       evidence requirements/source tasks
       decisions/positions when relevant
  -> answer with citations and uncertainty
  -> if data is missing, create evidence_requirement/source_task
  -> if answer changes standing view, flag thesis reconciliation
  -> if answer implies action, flag attention/review packet
```

Rules:

```text
answer from current evidence or say what is missing
never silently mutate a thesis
never turn a chat answer into a trade decision
write audit rows for prompt, context refs, source rows, tokens, and latency
surface follow-up work as source tasks or attention, not hidden prose
```

Current implementation: `/api/chat-analyst` invokes `prompts/chat-analyst.md`
through the audited prompt registry, loads symbol evidence packages including
`technical_state`, queues requested missing evidence into
`evidence_requirement`/`source_task`, and exposes the flow in the right-side
Analyst tab. Slow provider calls return an audited deterministic fallback.
Reconciliation packets and action review packets are still structured outputs,
not automatic writes.

Example questions:

```text
ENTG: Is the bullish thesis contradicted by extension above the 200-day SMA?
MU: What changed since the last thesis version?
Copper: Are proxy prices confirming the parent thesis, or do inventories disagree?
AMD: Search current public MI325X/MI400 adoption evidence and tell me what is missing.
```

## Thesis Model

The thesis is the core product object.

```text
symbol
direction: bull | bear | neutral
state: forming | building_conviction | armed | actionable | position_open | exiting | closed | disqualified
kind: monitoring | actionable_edge | decline_attempt
edge rationale
bull case
bear case
forecast
technical_state link
conviction conditions
trigger conditions
invalidation conditions
fulfillment/consensus conditions
known unknowns
evidence links
last_evaluated_at
version history
```

State transition principle:

```text
forming
  enough to describe the situation

building_conviction
  forecast and evidence requirements are concrete

armed
  trigger/invalidation/fulfillment conditions are measurable

actionable
  trigger is ready and risk should evaluate

position_open
  actual exposure exists

exiting
  invalidation, fulfillment, or manual exit is in progress

closed/disqualified
  outcome can be scored
```

Declined theses should be visible in history. They explain why the system did
not invent a claim and what would make it retry.

## Attention Loop

Attention is a state machine for work and judgment. It is not a feed.

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

Each attention item should answer:

```text
why does this exist?
who owns the next step?
what evidence is attached?
what happens if I confirm?
what happens if I reject?
when will it retry or resurface?
```

Attention kinds:

```text
candidate_review      confirm/reject monitored universe membership
thesis_incomplete     evidence/context/thesis blocker needs visibility
thesis_actionable     trade decision is ready
risk_review           risk warning/veto needs human acknowledgment
context_stale         context refresh is overdue or blocked
invalidation_hit      thesis may be refuted
outcome_ready         forecast can now be scored
brain_stale           parent macro/sector/theme view needs update
```

## Operator Workflow

The UI should have one understandable path from top-level market view to trade
outcome.

```text
1. Brain
   What is the system's current macro/sector/factor map?

2. Watchlists / Discovery
   What symbols are already tracked, and what new names are ranked for review?

3. Symbol Workspace (/symbol/$ticker)
   What does the chart, context, evidence, thesis, alerts, and decision history
   say about this symbol?

4. Review Packet
   Why did the system ask for judgment, what evidence supports it, and what are
   the allowed actions?

5. Decision
   Confirm/reject/defer a candidate, transition a thesis, or accept/skip a
   trade proposal.

6. Position Monitor
   Track actual exposure, thesis conditions, risk state, and exit reasons.

7. Outcome
   Score forecast, decision quality, and lead-time.
```

TradingView-like shell:

```text
+--------------------------------------------------------------------------------+
| Top: symbol search | watchlist dropdown | interval | regime | brain freshness  |
+---------------------------------------------------------------+----------------+
| Chart: candles, volume, SMA ribbon, RSI, thesis/attention marks| Right rail     |
|                                                               | watchlist rows |
|                                                               | thesis status  |
|                                                               | direction      |
|                                                               | evidence       |
|                                                               | actions        |
+---------------------------------------------------------------+----------------+
| Bottom tabs: Brain | Context | Thesis | Events | Decisions | Diagnostics       |
+--------------------------------------------------------------------------------+
```

Watchlist UX:

```text
watchlist dropdown
  All / Discovery / user lists / system lists

ticker row
  symbol, name, last price, thesis status, direction, freshness, attention count

filters
  thesis status, thesis direction, technical state, attention owner, freshness

membership menu
  checkmarked lists
  add/remove without leaving symbol
```

## Decision And Execution Loop

The system proposes. Risk constrains. The human decides. Positions prove whether
the decision mattered.

```text
actionable thesis
  -> risk check
  -> human decision
  -> manual/broker-confirmed position
  -> condition monitoring
  -> exit/invalidation/fulfillment
  -> outcome
```

The system needs real execution state:

```text
decision accepted
  -> intended order / manual execution note
  -> broker fill or manual fill
  -> position tied to thesis_id
  -> risk overlay reads real position
  -> outcome compares actual entry/exit, not theoretical thesis price
```

Until broker sync exists, manual position entry must be explicit and visible as
manual state.

## Reflection Loop

Reflection makes the product better.

```text
closed thesis / expired forecast / rejected candidate
  -> score forecast calibration
  -> score lead-time to consensus
  -> compare decision to relevant passive benchmark
  -> classify error
  -> update signal/prompt/watchlist learning inputs
```

Error types:

```text
late
too early
bad evidence
missing data
wrong parent thesis
bad ticker expression
risk veto saved loss
human override helped
human override hurt
no edge, correctly declined
```

## Data Source Plan

Current wired core:

```text
FMP price / intraday / estimates / screener / news
Massive news sentiment
SEC EDGAR/XBRL
FRED
GDELT/Bing research
z.ai LLM cognition
```

Needed to support the broader brain:

```text
commodity prices
  copper, wheat, oil, gas, gold, dollar proxies, ETFs/futures where legal

commodity fundamentals
  inventories, COT positioning, USDA crop reports, weather/geopolitical risk

analyst price targets and ratings
  rating changes, target revisions, recommendation drift

earnings calendar/transcripts
  upcoming catalysts and management commentary

options/positioning
  IV, skew, OI, unusual activity when LEAPS workflows matter

broker positions/fills
  actual exposure, entry/exit, realized PnL
```

Commodity/factor data should not be forced into an equity-only schema. The plan
needs a `market_factor` or equivalent abstraction, plus proxy mappings from
factor to tradable expressions.

## Tests And Acceptance Criteria

This product is not working until these flows pass deterministically.

Core system tests:

```text
active ticker with no thesis
  -> scheduler refreshes evidence
  -> cognition creates monitoring/actionable thesis or visible decline reason

missing evidence
  -> source task queued
  -> retry/backoff visible
  -> satisfied state moves ticker forward

open thesis older than 30m
  -> cognition re-evaluates
  -> no fake duplicate thesis

volume spike near all-time high
  -> composed as extended/consensus/exhaustion when appropriate
  -> not generic discovery

research nomination
  -> rank differs by evidence, theme fit, and source quality
  -> confirmation says exactly what happens

candidate rejected
  -> retained in history with reason
  -> does not keep interrupting unless new evidence changes

trade accepted
  -> decision recorded
  -> position state required or manual placeholder explicit
```

UI tests:

```text
all primary buttons clickable
confirm/reject/defer candidate
watchlist dropdown and checkmarked membership menu
symbol route /symbol/$ticker
chart intervals update bars without changing meaning of interval
SMA ribbon labels include periods
RSI panel visible
thesis tab shows current, declined, and retired views
evidence rows cannot say blocking+satisfied ambiguously
diagnostics explain source freshness and rows ingested
```

Example acceptance scenarios:

```text
NVDA
  must have at least a monitoring thesis if fresh context exists

MSFT
  cannot be blank; monitoring thesis or visible blocker

OKTA
  events without thesis should produce thesis_incomplete or monitoring thesis

DELL
  "no data" must show source acquisition state and retry path

MU
  multiple open theses are not allowed; history reconciles into one view

CRDO
  thesis.updated must reference a visible thesis_id or become thesis_incomplete

copper / wheat
  appear as parent brain factors with evidence requirements and tradable proxies

ENTG
  bullish thesis and extended technical state must be shown separately
```

## Implementation Plan

Do this in order. Do not keep adding UI surfaces over an incoherent loop.

### Phase 1: Make The Existing Brain Legible

```text
source freshness panel
selected-symbol brain status
checked/changed/evaluated timestamps per source
evidence requirement wording
declined/retired thesis history
one open thesis timeline
attention state/owner everywhere
```

Goal: every symbol explains what the system knows, what it is waiting for, and
what happens next.

### Phase 2: Make Freshness Active

```text
source_task table / queue
provider limiter integration
retry/backoff states
evidence acquisition FSM
scheduler-owned due work
diagnostics from task state
```

Goal: stale or missing data creates work automatically. The UI should never be
the trigger for basic acquisition.

Current implementation note: the first active source-task worker owns
Python-native web research (`gdelt_doc_search`, `bing_news_rss_search`) and is
embedded in the cognition service. Rust ingest loops own source tasks for FMP,
Massive, XBRL, EDGAR, FRED, CBOE, and sentiment scoring. Those expensive Rust
loops now use a tiered deep-research universe or benchmark-scoped macro tasks
instead of the entire screener pool. Future factor/commodity adapters should
follow that direct task-claiming contract from the start.

### Phase 3: Build The Parent Brain

```text
market_factor abstraction
macro/factor/theme brain thesis generator
scheduled parent thesis re-evaluation
commodity and sector evidence requirements
theme-to-ticker/proxy mappings
Brain tab as operating map, not static seed data
```

Goal: ticker cognition starts from current macro/factor/sector context instead
of isolated symbol context.

Current implementation note: `brain_thesis` records are now actively maintained
by the cognition sweep. The maintainer refreshes macro/source and linked ticker
coverage, turns beneficiary/proxy lists into active ticker mappings, updates
`last_evaluated_at`, writes version history when parent coverage materially
changes, and runs a bounded parent-thesis LLM pass when evidence changes or the
parent view is stale. That pass rewrites the parent summary, core claim,
evidence, open questions, invalidation conditions, and beneficiary/loser lists
from normalized linked evidence. Reflection now snapshots the active parent
macro/sector/theme links into ticker prediction claims when a thesis becomes
actionable, so parent-theme expression calibration can be reported separately
from global ticker-thesis calibration. The remaining Phase 3 work is broader
factor coverage: normalized commodity prices/fundamentals, sector breadth,
credit, earnings breadth, and better parent-direction inputs.

### Phase 4: Improve Discovery Ranking

```text
broad liquid screener
factor exposure mappings
signal composition with parent brain state
rank reasons with evidence freshness
candidate review packets
learn from confirm/reject
```

Goal: discovery queue is ranked, diverse, and explains why the system cares.

### Phase 5: Make Ticker Cognition Reliable

```text
monitoring thesis fallback
decline retry policy
targeted web/product/commodity research
known unknowns
challenge pass surfaced in UI
confidence separated from conviction
technical_state generated independently from thesis prose
chat analyst prompt loop for operator questions
```

Goal: active tickers are never blank and never forced into fake conviction.

### Phase 6: Connect Decisions To Real Positions

```text
manual fill workflow
IBKR bridge
position/thesis linkage
risk overlay from real exposure
exit decision workflow
decision replay snapshot
```

Goal: outcomes measure actual decisions and positions, not theoretical notes.

### Phase 7: Close The Learning Loop

```text
outcome scoring
lead-time-to-consensus
forecast calibration
parent-theme expression calibration
error taxonomy
signal/prompt feedback reports
benchmark by relevant passive alternative
```

Goal: know whether the product makes decisions better.

## Highest-Leverage Next PRs

```text
1. Product scope and plan docs
   Lock this plan into docs and prompts so implementation stops drifting.

2. Evidence acquisition FSM
   Turn missing evidence into scheduled work with retry/backoff, not static text.

3. Brain parent thesis service
   Generate/update macro, factor, commodity, and sector theses on a schedule.

4. Review packets and attention FSM adoption
   Every attention card should explain why it exists and how it resolves.

5. Monitoring thesis + declined history UX
   No blank active ticker; no hidden declines.

6. Commodity/factor data model
   Add copper/wheat/factor data and proxy mappings without abusing equity tables.

7. Decision-to-position workflow
   Tie accepted ideas to actual exposure and outcome scoring.
```

## Product Quality Bar

The system is good only when this is true:

```text
The operator can open the app,
see what the brain currently believes,
understand why each symbol is queued or tracked,
inspect one current thesis per ticker,
make a risk-aware decision,
and later know whether that decision was early, useful, and calibrated.
```

Anything else is decoration.
