# System model

This project is a single-operator trading intelligence system. It does not try
to be an autonomous hedge fund. It helps one human notice evidence-backed
market inflections earlier, reason about them more consistently, and record
enough evidence to learn from decisions over time. Tech infrastructure is a
current strength/theme, not a hard product boundary; copper, wheat, financials,
staples, energy, healthcare, and any other liquid market can matter when the
evidence creates a falsifiable money-making view.

The shortest description:

```text
market/data events
  -> maintained ticker context
  -> structured theses
  -> risk-gated alerts
  -> human decisions
  -> outcomes/reflection
  -> better future signals/prompts
```

The system's edge is information diffusion. Public facts do not become equally
understood by every market participant at once. The system tries to find the
window between "available" and "obvious to the late retail/passive crowd" for
the relevant market, sector, commodity proxy, or individual equity.

## Operating loop

The desired operating model is continuous scanning with tiered depth. The system
should always be pulling new market, news, estimate, filing, sentiment, and
portfolio evidence for the symbols it can reasonably monitor. Most scans should
not interrupt the operator. They should update evidence, context, thesis state,
and freshness silently, then create attention only when something meaningful
needs judgment.

```text
continuous scan
  -> fresh evidence
  -> updated context
  -> re-evaluated thesis state
  -> attention only when judgment is needed
```

The product loop is built around one selected symbol at a time in the UI, but
the background system is always scanning many symbols and appending event
history.

```text
1. Ingest
   EDGAR, FRED, price, estimates, ratings, news, sentiment, portfolio state

2. Route
   Normalize events, persist them, fan them out to affected tickers and services

3. Maintain context
   Keep each ticker's structural, narrative, and market context fresh

4. Discover
   Scan a broader universe for cheap raw signals, compose those facts into an
   operator-facing interpretation, and propose watchlist adds only when the
   interpretation deserves review

5. Draft/evaluate thesis
   Convert context + technicals + regime into a falsifiable thesis object

6. Apply safety nets
   Risk overlay, condition evaluator, staleness checks, goalpost detector

7. Alert human
   Surface only meaningful transitions, alignments, invalidations, and decisions

8. Record decision/outcome
   Keep every decision and outcome so calibration can improve
```

The durable mental model:

```text
Discovery pool = broad radar
Watchlists     = focused radar
Context        = memory per symbol
Thesis         = active hypothesis
Attention      = "look here now"
Decision       = what the human chose
Outcome        = whether it was right
```

The canonical object lifecycle and attention-kind map lives in
[`docs/LIFECYCLE.md`](LIFECYCLE.md). When changing discovery, attention,
thesis transitions, decisions, positions, or outcomes, update that map in the
same PR.

The scheduler and feedback-loop map lives in
[`docs/BRAIN_LOOPS.md`](BRAIN_LOOPS.md). When changing service intervals,
freshness rules, retry behavior, or queue ownership, update that loop map in
the same PR.

The product/system plan that connects the brain, discovery, cognition, thesis,
attention, decision, position, and reflection loops lives in
[`docs/PRODUCT_PLAN.md`](PRODUCT_PLAN.md). Use it as the implementation spine
when choosing the next work item.

## Scan tiers

Scanning should be broad, but not equally expensive for every symbol. The system
should spend cheap deterministic work on the broad pool and reserve expensive
LLM cognition for names that earn attention.

```text
Tier 3 / discovery_pool
  cheap broad radar
  price/volume
  news headlines and sentiment when available
  estimate/rating snapshots
  lightweight profile/fundamental metadata
  candidate_review attention when signals fire
  research_nomination attention for reasoned unreviewed names with evidence

Tier 2 / watchlisted or confirmed candidate
  evidence requirement tracking
  automatic evidence checklist bootstrap if missing
  regular context maintenance
  richer evidence history
  XBRL/company facts where available
  thesis drafting when blocking evidence is present and context supports a view

Tier 1 / active thesis or position
  deep context maintenance
  scheduled thesis re-evaluation
  thesis condition evaluation
  risk checks
  consensus/fulfillment monitoring
  decision and outcome support
```

The broad pool should not receive full LLM thesis generation on every scan. It
should receive enough coverage to detect that a symbol deserves human review.
There are two ways a pool name reaches attention:

```text
signal path:
  price/news/estimate signal -> composed interpretation -> candidate_review

nomination path:
  unreviewed pool member with enough evidence + explicit opportunity reason
        -> research_nomination candidate_review
```

`research_nomination` means "this business or market factor belongs in the
monitored universe for these explicit reasons"; it does not mean a trade signal
fired. The stored `source_ref.nomination_reasons` records theme/sector/factor
fit, business fit, suggested watchlists, and the acceptance effect. Confirming
promotes the symbol into the tracked universe/watchlists and lets cognition
build context and attempt a thesis. Confirmed/watchlisted symbols get deeper
context and thesis workflows.

Open theses are not static notes. The cognition service runs a bounded update
loop that selects active tickers whose context is stale, whose evidence
checklist has not been initialized, whose evidence retry is due, or whose open
thesis has not been re-evaluated within the configured freshness window.
Fresh normalized evidence facts also trigger re-evaluation when they arrive
after the last thesis evaluation or after the last no-thesis decline
(`COGNITION_OPEN_THESIS_MAX_AGE_MINUTES`, default 30). A fresh draft against an
existing open thesis reconciles into the canonical thesis and appends
`thesis_version_history`; it should not create a second open thesis. The
database enforces this with a partial unique index on open thesis rows per
symbol.
Selected sweep targets run with bounded concurrency
(`COGNITION_SWEEP_CONCURRENCY`, default 2), because the context/thesis/sharpen
pipeline can wait on LLM calls and a serial 20-symbol sweep can miss the
30-minute freshness target.

The scan universe for cheap data should be consistent:

```text
active discovery_pool
UNION
active ticker/watchlist universe
```

If discovery scans the broad pool but news, estimates, ratings, or fundamentals
only scan the old seed list, the system degenerates into a price/volume scanner.
That is not the desired product. Data acquisition should follow:

```text
active discovery_pool
UNION
active ticker/watchlist universe
```

When an expected input is absent, the system records an `evidence_requirement`
instead of treating the absence as an investment conclusion.

Consensus crossings are validation and attention signals, not automatic thesis
lifecycle events. If a consensus score crosses for a symbol with no open thesis,
the system records a `thesis_incomplete` attention item and kicks cognition
instead of emitting `thesis.updated`. `thesis.updated` and `thesis.fulfilled`
must carry a real `thesis_id`.

## Core objects

### Ticker

A persistent monitoring object. Tickers have a cluster, tier, tradeability
metadata, and watchlist membership. A ticker can be lightly monitored before it
has any thesis.

```text
ticker
  symbol
  cluster
  tier
  options_eligible
  domain_fit
```

### Watchlist

The operator-facing way to browse the universe. Watchlists are not just manual
folders; discovery can propose adding symbols to them. The UI should treat
`Universe` / `All Tickers` as the replacement for a separate Tickers page.

```text
watchlist
  -> visible navigation group
  -> symbol membership
  -> discovery assignment target
```

### Ticker context

The system's memory for a symbol. It is versioned and append-only.

```text
ticker_context
  structural  -> slow facts: business model, fundamentals, competitive position
  narrative   -> medium-speed facts: catalysts, analyst drift, monitored risks
  market      -> fast facts: price/volume/technicals/sentiment
```

Each band has its own freshness timestamp. That matters because a fresh price
tick must not make stale narrative or lagged 13F data look current.

### Evidence requirement

The retryable acquisition state for missing inputs. Missing data is not a
thesis and not a final answer.

```text
evidence_requirement
  symbol
  requirement_key     price_history | company_facts | recent_news | analyst_estimates | product_research
  source_type         price | fundamentals | news | estimates | web_research
  reason
  priority            blocking | high | medium | low
  blocking_state      missing | fetching | partial | blocked | satisfied
  attempts
  next_retry_at
  last_error
  source_ref
```

Context maintenance updates this table before expensive LLM work. Blocking
requirements stop context/thesis draft attempts. Non-blocking requirements
travel into the thesis prompt and UI so the operator can see why the view is
weak.

### Research evidence

The web/product evidence layer covers named product and theme claims that
symbol-tagged vendor news often misses: accelerator SKUs, benchmark posts,
deployment reports, customer adoption, and competitive roadmap evidence.

```text
research_evidence
  symbol
  query
  url / title / publisher / published_at
  retrieved_at
  provider
  source_type
  credibility
  tags
```

Context refresh runs targeted retrieval before the LLM pass and feeds the
retrieved rows into `ticker_context.narrative`. The first providers are GDELT
Doc 2.0 plus Bing News RSS fallback because neither requires a key; paid
semantic search can replace or augment them if recall is too weak. A thesis
should not claim "no public data" for a named
product unless the `product_research` evidence requirement shows the retrieval
state.

Search-provider output is not trusted just because the query targeted a symbol.
The research layer promotes only rows whose result text matches the ticker, a
company alias, or a specific product term. Accepted rows carry
`source_ref.relevance`; unvetted legacy web-research evidence is filtered out of
context, thesis, and parent-brain evidence loaders.

### Thesis

The thesis is the primary product object. It is not a note or a vibe; it is a
versioned, falsifiable state-machine object.

```text
thesis
  symbol
  state
  edge_rationale
  forecast
  conviction_conditions
  trigger_conditions
  invalidation_conditions
  fulfillment_conditions
  version history
```

The original edge rationale and invalidation conditions are preserved. Later
edits can refine the thesis, but weakening invalidation is flagged by the
goalpost detector.

### Decision

A decision records what the human did or declined to do. It is append-only and
is used later for calibration and reflection.

```text
decision
  thesis_id
  action proposed
  user choice
  sizing/rationale
  timestamp
```

### Outcome

An outcome scores a thesis or prediction after time has passed. This is how the
system learns whether it was early, calibrated, or just noisy.

## Data flow

The system is event-driven. Services communicate through NATS JetStream and
write durable state to Postgres.

```text
                 +----------------+
                 | external data  |
                 | EDGAR/FRED/... |
                 +-------+--------+
                         |
                         v
                  ingest.* events
                         |
                         v
        +----------------+----------------+
        | NATS JetStream durable streams  |
        +-------+------------+------------+
                |            |
                v            v
          route.ticker.>   regime.*
                |            |
                v            v
        ticker context   market_state
                |            |
                +------+-----+
                       |
                       v
                   thesis.*
                       |
                 +-----+------+
                 |            |
                 v            v
              risk.*      alerts/SSE
                 |            |
                 +-----+------+
                       |
                       v
                  operator UI
                       |
                       v
                 decision.*
```

Postgres is the durable system of record. JetStream is the replayable event
fabric. Replaying history through improved prompts, indicators, or evaluators is
part of the design.

## Pipeline in detail

The pipeline has two connected lanes:

```text
data lane
  external source -> ingest_event -> normalized store -> NATS subject

reasoning lane
  routed events -> context/thesis/risk/reflection -> alerts and decisions
```

Every stage should either persist append-only evidence, emit a typed event, or
both. A service that only mutates hidden state is a design smell.

### 1. Ingestion

Ingestion adapters pull data from vendors and public sources, normalize the
payload, write an append-only raw/normalized record, and publish an event.

Examples:

```text
SEC/XBRL facts        -> ingest.filing / ingest.fundamental
FRED macro series     -> ingest.macro
FMP daily OHLCV       -> ingest.price
FMP estimates/grades  -> ingest.estimate / ingest.rating
Massive/FMP news      -> ingest.news
IBKR portfolio state  -> position.updated
```

The ingestion rule is simple: never overwrite the past. If a vendor revises
data, record the new observation with its own ingestion time.

### 2. Routing

The router fans broad ingest events into subjects that downstream services can
consume without knowing every vendor shape.

```text
ingest.*
  -> route.ticker.NVDA
  -> route.ticker.AMD
  -> route.ticker.2454.TW
  -> route.market
```

Ticker-routed events feed context maintenance and discovery. Market-routed
events feed regime, consensus, and cross-market signals.

### 3. Market state

The regime service consumes market inputs and writes `market_state`.

```text
route.market
  -> indicator snapshot
  -> regime classification
  -> market_state row
  -> regime.state event when state changes
```

Regime does not create a thesis by itself. It modulates whether a thesis is
allowed to become actionable, how aggressive sizing should be, and whether
tactical dip-buy conditions are valid.

### 4. Context maintenance

Context is the maintained memory for a ticker. The context maintainer combines
recent routed events with prior context and emits a new version when the state
materially changes.

```text
route.ticker.SYMBOL
  + previous ticker_context
  + prompt version
  -> new ticker_context version
  -> evidence_item(kind=context_shift) when structural/narrative/market changes
  -> context.updated / context.shift
```

The three context bands move at different speeds:

```text
structural  slow     business quality, end markets, fundamentals
narrative   medium   catalysts, analyst drift, competitive developments
market      fast     price, volume, technicals, sentiment
```

Freshness is part of the output. A stale narrative band is a decision input, not
just an ops warning. When the maintained context changes, the maintainer writes
a normalized `context_shift` evidence item pointing at the new
`ticker_context` version. Thesis and decision replay can then distinguish
source facts from the system's interpretation of those facts.

The cognition service owns automatic maintenance. It reacts immediately to
`discovery.confirmed`, but it also periodically sweeps active tickers so the
Universe does not rot while waiting for a user to open a tab:

```text
ticker.status = active
  + no context OR context older than COGNITION_CONTEXT_MAX_AGE_HOURS
  -> refresh context

active ticker with no open thesis
  + no decline OR decline older than COGNITION_DECLINE_RETRY_HOURS
  -> draft thesis
  -> persist monitoring/actionable thesis OR record thesis_incomplete reason

active ticker with an open thesis
  + source_task or evidence_item newer than last_evaluated_at OR freshness due
  -> refresh context
  -> update the canonical thesis row when the view changes
  -> append a thesis_version_history reconciliation event
  -> do not create a second active thesis

active ticker with due missing evidence
  -> refresh evidence/context
  -> retry thesis when blocking requirements are satisfied

active ticker where evidence became satisfied after a decline
  -> retry thesis on the next bounded sweep
```

This means "no data" and transient provider failures are retryable operating
states, not final product states.

### 5. Discovery

Discovery scans the wider universe for cheap signals that might deserve deeper
attention.

```text
price/volume anomaly
estimate or rating drift
news/catalyst burst
relative strength
domain-fit classification
  -> discovery_candidate
  -> discovery.candidate
```

Discovery does not silently promote a symbol into the operator's focus. It
creates a candidate with evidence and proposed watchlist assignments. The human
confirms or rejects the assignment, and that correction becomes part of the
learning signal.

### 6. Thesis drafting and sharpening

The thesis engine turns maintained context into a structured thesis when there
is a plausible edge.

```text
latest ticker_context
  + market_state
  + relevant events
  + prompt hash
  -> thesis draft
  -> prediction rows for calibration
```

A valid thesis must explain what is not yet priced, what would increase
conviction, what would trigger action, what would invalidate it, and what would
count as fulfillment/consensus arrival.

Sharpen/challenge passes improve the object without changing its purpose:

```text
sharpen   -> proposes clearer measurable conditions
challenge -> flags weak rationale, missing evidence, or self-deception risk
```

### 7. Condition evaluation

The evaluator and staleness service continuously inspect thesis conditions.

```text
v_condition
  -> resolve target metric
  -> compare observed value to condition target
  -> mark satisfied/refuted/stale
  -> emit warning when stale or dangerous
```

Condition status should support transitions, not bypass them. A condition being
satisfied is evidence; the state machine still controls what can happen next.

### 8. Thesis state transitions

The thesis state machine is the canonical path from idea to action to outcome.

```text
forming
  -> building_conviction
  -> armed
  -> actionable
  -> position_open
  -> exiting
  -> closed
```

At any point a thesis can become `disqualified` when the idea is invalidated or
the object is not good enough to keep alive.

Promotion gates are substance gates:

```text
forming -> building_conviction
  requires forecast + conviction conditions

building_conviction -> armed
  requires trigger + invalidation conditions

armed -> actionable
  requires trigger readiness

position_open -> exiting
  requires fulfillment/exit conditions
```

Code should use `thesis::substance::promotion_allowed()` and the transition
endpoint rather than mutating `thesis.state` directly.

### 9. Risk and alerting

When a thesis becomes actionable, the risk overlay evaluates it independently.

```text
thesis.actionable
  -> risk overlay
  -> risk.ok | risk.warning | risk.veto
  -> gateway alert/feed
```

The risk overlay constrains the proposal. It does not decide whether the idea
is intellectually good; it decides whether the proposed action fits portfolio,
liquidity, concentration, drawdown, options, and regime constraints.

### 10. Consensus and reflection

Consensus is both an exit signal and a validation anchor.

```text
estimate saturation
rating/news coverage
retail attention
price extension
  -> consensus_score
  -> thesis.fulfilled when thresholds cross
  -> outcome row
  -> calibration metrics
```

Reflection scores what happened after a thesis or prediction. The point is not
to celebrate wins; it is to learn whether the system was early, calibrated, and
useful versus the relevant passive benchmark for that decision.

## Service families

Rust owns deterministic event processing and the operator gateway.

```text
gateway     REST, SSE, embedded Svelte SPA
ingest      vendor adapters and append-only ingest events
router      fan-out from ingest subjects to ticker/market subjects
regime      market-state classifier
risk        independent risk overlay
goalpost    detects thesis edits that weaken invalidation
staler      warns on stale or past-deadline thesis conditions
evaluator   evaluates quantitative/narrative thesis conditions
consensus   computes consensus/fulfillment anchors
discovery   promotes candidate symbols from cheap-wide signals
reflection  scores predictions and outcomes
```

Python owns LLM-heavy cognition and research workflows.

```text
context_maintainer  refreshes ticker_context
thesis_engine       drafts thesis objects from context
sharpen             improves thesis conditions
challenge           critiques weak theses
classify            assigns discovery candidates to watchlists
```

All LLM calls must go through the prompt registry wrappers so invocation,
prompt hash, tokens, and latency are auditable.

## Operator UI model

The UI should be a workstation, not an admin console. The durable interaction
model is:

```text
selected symbol
  -> chart
  -> selected-symbol panel
  -> context
  -> theses
  -> alerts
  -> decisions
```

Target shell:

```text
+------------------------------------------------------------------------------+
| Top bar: symbol/search, range, regime, stream status, actions                |
+--------------------------------------------------------------+---------------+
| Main chart: candles, thesis markers, alert markers, regime overlays          |
|                                                              | Right panel   |
|                                                              | Watchlist     |
|                                                              | Symbol detail |
|                                                              | Context       |
|                                                              | Theses        |
|                                                              | Alerts        |
|                                                              | Decisions     |
+--------------------------------------------------------------+---------------+
| Bottom drawer: events, discovery queue, decisions, calibration, diagnostics  |
+------------------------------------------------------------------------------+
```

Watchlists should be selected through a dropdown. Symbol membership should use
checkmarked watchlist menus, similar to TradingView. This keeps ticker browsing,
manual watchlist edits, and discovery candidate assignment under one mental
model.

## Decision process

The human decision process is deliberately separated from the machine pipeline.
The system can propose, warn, veto, and record. It does not silently trade.

### Candidate decision

Discovery candidate review answers: "Should this symbol enter our monitored
universe, and where?"

```text
discovery_candidate proposed
  -> LLM/watchlist classifier proposes lists
  -> UI shows evidence + checkmarked list assignments
  -> human confirms or rejects
```

Confirming a candidate:

```text
candidate.status = confirmed
selected watchlist memberships are created
symbol becomes easy to revisit from the watchlist UI
classification/decision is kept for later learning
```

Rejecting a candidate:

```text
candidate.status = rejected
reason/evidence remains in history
future classifiers can learn boundary corrections
```

### Thesis decision

Thesis review answers: "Is this a real thesis, and what state is it allowed to
enter?"

```text
draft thesis
  -> inspect edge rationale
  -> inspect forecast and conditions
  -> sharpen/challenge if needed
  -> transition only when substance gates pass
```

The user should not need to infer whether a thesis is vague. The UI should show
the missing substance directly:

```text
forecast missing
conviction condition too vague
trigger condition lacks target/deadline/evidence source
invalidation condition missing
fulfillment condition missing
```

The transition decision is separate from the trade decision. Moving a thesis to
`armed` means "watch this for action." Moving it to `actionable` means "the
system believes the trigger is ready and risk should evaluate it."

### Trade decision

Trade decision answers: "Given the thesis and risk overlay, what will the human
do?"

```text
actionable thesis
  -> risk.ok / risk.warning / risk.veto
  -> human chooses:
       accept / reduce / defer / skip / reject
  -> decision row is appended
```

Decision rows should capture enough information for later evaluation:

```text
thesis_id
proposed action
user choice
sizing
rationale
timestamp
risk result visible at decision time
```

The expected behavior by risk result:

```text
risk.ok
  user may accept, defer, reduce, or skip

risk.warning
  user may continue, but the warning must remain attached to the decision

risk.veto
  UI should block accept/open-position flows
  user can still record skip/reject/defer
```

### Position and exit decision

When the human opens a position, the thesis moves into position tracking.

```text
decision accepted
  -> position_open after manual/broker-confirmed position exists
  -> conditions continue to be evaluated
  -> invalidation/fulfillment alerts are surfaced
```

Exit decisions are driven by thesis conditions, not by hindsight editing.

```text
invalidation condition satisfied
  -> thesis should move toward exiting or disqualified

fulfillment/consensus condition satisfied
  -> thesis should move toward exiting/closed

manual exit
  -> record the decision and preserve the original thesis for scoring
```

### Outcome decision

Outcome scoring answers: "Was the system useful?"

```text
closed / fulfilled / invalidated thesis
  -> outcome row
  -> Brier/calibration score when forecast is scorable
  -> lead-time-to-consensus when consensus is observed
  -> decision quality compared with the relevant passive benchmark
```

Bad outcomes are first-class data. The system should make it easy to see when a
thesis was late, uncalibrated, overruled by risk, or rejected correctly by the
human.

## Safety model

The system deliberately separates powers:

```text
thesis engine proposes
risk overlay constrains
human decides
reflection scores
```

No service both proposes and approves its own trade. Risk and integrity checks
are deterministic where possible, and append-only histories make later review
possible.

Important safety nets:

```text
risk overlay       position and portfolio limits
goalpost detector  flags self-serving thesis edits
staleness service  warns when deadlines/data go stale
condition evaluator resolves thesis conditions against evidence
prompt audit       records which prompt version produced each LLM output
```

## Validation model

Backtesting is not the primary proof. The edge depends on forward information
diffusion and self-built point-in-time data.

Primary validation:

```text
lead-time-to-consensus
  alert timestamp -> consensus/fulfillment timestamp

forecast calibration
  thesis forecast -> realized outcome

decision quality
  realized choices -> relevant passive benchmarks
```

If the system does not improve decision quality after enough forward use, the
right answer is to reassess it, not to add complexity.
