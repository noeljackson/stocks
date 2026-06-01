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
  -> linked to symbol/factor/theme
  -> satisfies or weakens evidence_requirement
  -> available to context/thesis prompts
```

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
broad liquid universe
  -> cheap evidence scan
  -> signal composition
  -> ranking
  -> candidate_review attention only when judgment is useful
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

## Cognition Loop

Cognition is the symbol brain. It creates and maintains current views.

```text
symbol selected by event or scheduler
  -> refresh evidence requirements
  -> fetch due evidence where possible
  -> refresh context bands
  -> draft/reconcile thesis
  -> sharpen conditions
  -> challenge weak claims
  -> persist current view or visible decline reason
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
  -> outrank no-context and broad evidence-bootstrap work in the scheduler
  -> re-evaluate against latest context
  -> no_change: update last_evaluated_at only
  -> changed: append thesis_version_history
  -> contradiction: produce attention
  -> invalidated: transition/disqualification review
```

The system should never create multiple open theses for MU or any other symbol.
New facts reconcile into the canonical current thesis and show as a timeline.

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
```

## Implementation Plan

Do this in order. Do not keep adding UI surfaces over an incoherent loop.

### Phase 1: Make The Existing Brain Legible

```text
source freshness panel
selected-symbol brain status
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
embedded in the cognition service. Rust ingest loops still own FMP, Massive,
EDGAR, XBRL, FRED, and sentiment scoring. Those expensive Rust loops now use a
tiered deep-research universe, not the entire screener pool: active tickers
first, then Tier 1/2 proposed candidates, capped per provider pass. The
remaining scheduler work is to centralize those provider limiters behind the
same task-claiming contract.

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
`last_evaluated_at`, and writes version history when parent coverage materially
changes. The remaining Phase 3 work is the actual factor/LLM thesis generator:
rewriting parent claims from normalized macro, commodity, sector breadth, and
cross-ticker evidence instead of only maintaining coverage state.

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
