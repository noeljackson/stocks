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
        |     role: leader/challenger/supplier/customer/beneficiary/hedge/candidate/proxy
        |
        +-> brain_thesis_watchlist
              optional explicit list mapping
```

Current parent coverage includes macro regime, AI compute, memory/HBM, optical
networking, enterprise security/identity, copper/industrial metals,
wheat/agriculture/food inflation, financial conditions/credit, energy
supply-demand, consumer staples/margins, and housing/rates/real assets. These
are parent hypotheses, not sector limits: the brain should add or retire themes
based on whether they help find evidence-backed opportunities.

`GET /api/brain` reads those records and shows the operator the current macro
view, active sector/theme views, linked tickers, implied watchlists, queued
discovery nominations, stale/missing parent evidence, and simple
macro/sector-direction contradictions. Ticker thesis drafting receives the
linked parent theses as input, so a symbol-level view can inherit, reject, or
explicitly contradict the parent theme instead of guessing in isolation.

The macro parent thesis now receives deterministic internals before the LLM pass:
market breadth from local daily `price_bar` rows, sector relative strength from
`discovery_pool` sector baskets plus price bars, earnings breadth from recent
`estimate_revision` rows, and a credit trend proxy from FRED HY OAS observations.
These metrics clear the canonical macro requirements
`market_breadth_internals`, `sector_relative_strength`, `earnings_breadth`, and
`credit_internals_trend` when enough local data exists. The Brain tab shows the
headline values so the operator can audit why the macro thesis is active,
neutral, risk-on, or risk-off.

The macro maintainer also builds a deterministic **Dislocation Map** from the
same internals plus sector news attention/sentiment:

```text
Loved / mania       strong RS or attention; true stories may be poor entries
Ignored             improving revisions/RS with low attention
Hated / avoided     weak price/sentiment where evidence is becoming less bad
```

The map is stored in `brain_thesis.source_ref.maintainer.dislocation_map` on the
macro row and versioned through normal parent-thesis history. It is not a trade
recommendation. It tells downstream loops where the market may be emotionally
wrong or inattentive. Discovery ranking boosts ignored/hated improving sectors
and penalizes loved/extended chase setups; ticker thesis drafting receives the
symbol's sector dislocation context so it can say "bullish but loved/extended"
or "ignored with fresh evidence" without confusing direction with entry timing.

## Brain Journal

The Brain Journal is the daily operator-facing narrative layer. It is generated
from deterministic system events first, not from free-form chat memory:

```text
attention_item
thesis_state_history
thesis_version_history
source_task
evidence_item
brain_thesis_version_history
macro dislocation map
        |
        v
brain_journal_entry  append-only, deduped by event_key
        |
        v
GET /api/brain-journal?date=YYYY-MM-DD&page=N&per_page=N
        |
        v
/journal/YYYY-MM-DD daily brain memo + pageable receipts
```

Journal categories match the operator question:

```text
changed              we think this changed
research             needs research
curious              we are curious
crowded_or_extended  loved/manic or technically crowded pockets
ignored_or_hated     indifferent/hated pockets that may deserve work
blocked              missing/rate-limited/failed data that blocked conclusions
```

The journal does not invent evidence. Entries link back to the source row through
`source_kind`, `source_id`, `symbol`, `thesis_id`, `brain_thesis_id`, and
`source_ref`. A later LLM synthesis pass can summarize the deterministic entry
set, but the durable facts are already present before any model writes prose.

The journal page is not meant to feel like a raw event stream. The response also
includes a deterministic `overview` object that reads like a daily analyst memo:

```text
overview
  market            macro/regime posture and missing parent evidence
  top_candidates    thesis-backed names where setup is not saying chase
  wait_for_setup    bullish/active theses where technicals say wait
  risk_flags        bearish, deteriorating, blocked, or contradiction names
  themes            active macro/sector/theme pressure
  news_recap        high-signal news/evidence rows for the date
  research_focus    blocked, queued, or curious work that needs follow-up
entries             paged receipts behind the memo
```

This distinction matters operationally: a ticker can be bullish as a thesis but
still not be a good entry. The journal should make that separation explicit
instead of forcing the operator to infer it from scattered events.
The Brain drawer should link to this page rather than embedding the journal;
the drawer is for live macro/sector/attention state, while the journal is the
pageable historical readout.

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
FMP_LIVE_BAR_MAX_SYMBOLS_PER_PASS       25
FMP_LIVE_BAR_INTERVAL_SECS              60
FMP_LIVE_BAR_ENTITLEMENT_BACKOFF_SECS  3600
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
reclaimable work, even if its `due_at` was pushed forward. A source health row
that is still `running` after the same window is no longer treated as active
fetching; evidence sync turns it back into queued acquisition work. This
prevents an interrupted worker from leaving evidence acquisition stuck forever.

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
firecrawl_search                  Python source_task worker
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
- Research nominations have an explicit review budget: only the best 100 open
  nominations remain in `candidate_review`. Lower-ranked proactive suggestions
  are superseded automatically so the operator is not asked to review the whole
  screener pool. Real signal candidates such as breakouts, exhaustion, and
  estimate/news shifts are not part of that nomination budget and are always
  returned by the pending-candidate API alongside the top nominations.
- Pending candidates are ranked before review using deterministic signal
  quality, domain fit, active parent-theme fit, proposed tier, and
  watchlist-classifier confidence. The API includes the linked parent themes so
  the discovery tab can explain why a candidate is more than a raw alert.

Current status and gaps:

- #129: macro and sector theses exist as records, the cognition sweep maintains
  their coverage/freshness state, and a bounded parent-thesis LLM pass can
  rewrite parent claims from linked ticker/evidence state when evidence changes
  or the parent view is stale. Parent records now cover tech infrastructure,
  commodities, financials, energy, staples, and housing/rates. Reflection
  snapshots the relevant macro/sector/theme links into ticker prediction claims
  when a thesis becomes actionable, so calibration can report parent-theme
  expression results separately from global ticker-thesis calibration. Ticker
  thesis detail responses also expose their linked parent themes, roles,
  direction, conviction, and rationale so the UI can show the macro/theme ->
  ticker chain without forcing an operator to switch tabs. Local macro internals
  now cover breadth, sector relative strength, earnings revision breadth, and HY
  OAS trend; richer external factor coverage for inventories, flows, commodity
  fundamentals, and deeper credit quality buckets is separate data-source work.
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
  provider source_task rows_seen result is newer than thesis evaluation / no-thesis decline
  normalized evidence_item is newer than thesis evaluation / no-thesis decline
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
if thesis reconciliation materially changes/weakens/invalidates the view:
  upsert thesis_review attention for the operator
        |
        v
write cognition_run with status, reason, blockers, retry
        |
        v
refresh parent brain theses after ticker SLA work
```

Current defaults:

```text
COGNITION_SWEEP_SECONDS                300
COGNITION_CONTEXT_MAX_AGE_HOURS        12
COGNITION_OPEN_THESIS_MAX_AGE_MINUTES  30
COGNITION_DECLINE_RETRY_HOURS          6
COGNITION_MAX_SYMBOLS_PER_SWEEP        20
COGNITION_MIN_SYMBOLS_PER_SWEEP        20
COGNITION_SWEEP_CONCURRENCY            2
COGNITION_EVIDENCE_SYNC_LIMIT          200
```

The worker treats the freshness target as a guardrail: nonzero sweep intervals
are capped at half the open-thesis freshness window, with an upper cap of 300s
and lower cap of 60s. Batch size is floored by
`COGNITION_MIN_SYMBOLS_PER_SWEEP`. This prevents stale environment config like
`900s/5 symbols` from silently breaking the 30-minute product SLA.

Sweep execution is bounded-concurrent, not serial.
`COGNITION_SWEEP_CONCURRENCY` controls how many selected symbols may run the
context/thesis/sharpen/challenge pipeline at once. The default is 2 because a
single symbol can spend real time in LLM and source-refresh work; serially
processing 20 stale theses can already exceed the 30-minute target.

Parent brain/theme thesis maintenance runs after ticker target execution inside
the sweep. It is important context, but it should not block stale open ticker
theses from entering the update loop.

Sweep priority is evidence-first. Fresh provider rows for an open thesis come
first, then fresh normalized `evidence_item` facts, then stale open theses,
then bootstrap work. A provider row trigger is a completed `source_task` with
`state='satisfied'` and `source_ref.result='rows_seen'`. Routine `no_rows`
checks still refresh source health/task state, but they do not force a thesis
LLM pass by themselves. `fetching`, `failed`, and rate-limited work stays
visible in source health/tasks without forcing a thesis LLM pass.
A normalized evidence delta is a newly available fact such as news, a filing,
estimate revision, price-action event, product research, context shift, or
regime change. The delta clock is `evidence_item.updated_at`, not merely
`observed_at`, so later sentiment scoring or relevance/source-ref enrichment
can also wake cognition. Each scheduled run records a `sweep_reason` such as
`source_task_changed`, `evidence_item_changed`, `open_thesis_due`,
`context_missing`, or `evidence_retry_due` into the pipeline source reference
so the UI/audit trail can explain why the symbol was touched.

The worker still reserves a small number of each sweep's slots for bootstrap
work (`COGNITION_BOOTSTRAP_SYMBOLS_PER_SWEEP`, default 5), because otherwise a
large set of stale open theses can starve new watchlist/discovery symbols and
leave them stuck at `initialize_evidence`.

Adding a ticker to a watchlist publishes the same `discovery.confirmed` kickoff
as confirming a discovery candidate. Watchlists are part of the active operating
universe, so a manual add should immediately start context/evidence/thesis work
instead of waiting for the next broad sweep.

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

`thesis_review` attention is the operator-facing cue for a changed standing
view. It is emitted for `weakened_view`, `material_change`, and
`invalidates_existing_view` reconciliations. It is intentionally separate from
`thesis_actionable`, which means a trade decision is ready.

```text
cognition target priority
  0 source_task rows_seen newer than open thesis evaluation
  1 normalized evidence newer than open thesis evaluation
  2 open thesis due for re-evaluation
  3 missing context
  4 context exists but market context is blank
  5 evidence checklist missing
  6 evidence retry due for a no-thesis symbol
  7 source_task rows_seen newer than no-thesis decline
  8 normalized evidence newer than no-thesis decline
  9 stale context
 10 older decline retry / maintenance
```

What works now:

- Confirming a candidate starts cognition through `discovery.confirmed`.
- Active tickers are swept without requiring the UI to open them.
- Evidence checklists are bootstrapped for old tickers.
- Open theses are explicitly due for re-evaluation after 30 minutes.
- Fresh source-task `rows_seen` outcomes trigger a cognition pass immediately
  when they are newer than the open thesis evaluation or newer than the last
  no-thesis decline. This gives the brain a data-change edge in addition to the
  30-minute clock edge without burning LLM passes on routine `no_rows` checks.
- Fresh normalized evidence facts trigger the same immediate pass when they are
  newer than the open thesis evaluation or newer than the last no-thesis
  decline. This catches facts created by news, filings, research, price-action,
  context-shift, and regime producers even when they are not represented as a
  source task.
- Fresh drafts reconcile into one canonical open thesis per symbol.
- Draft/reconcile runs attach the latest normalized evidence facts to the
  thesis via `thesis_evidence`, so the current view can be inspected back to
  concrete source rows.
- Malformed draft JSON is retried through the same audited prompt path instead
  of crashing the cognition update loop on the first bad model response.
- Dev cognition sweep runs every 5 minutes over up to 20 active symbols with
  bounded concurrency 2 by default, so a larger universe is not starved behind
  a serial five-symbol batch or a single slow LLM call.
- Each sweep now refreshes open evidence requirements from the latest source
  health before selecting cognition targets. That lets provider success,
  failures, no-new-row passes, and newly satisfied rows move tickers forward
  without waiting for an operator to open the ticker.
- Every pipeline attempt is visible through `cognition_run`, Diagnostics, and
  the selected-symbol Brain card. Failed, blocked, declined, no-change, drafted,
  and reconciled runs are all recorded.
- Cognition startup reclaims orphaned `running` rows from a prior process, and
  each sweep also reclaims stale `running` rows older than
  `COGNITION_RUNNING_RECLAIM_MINUTES` (default 30). Reclaimed rows become
  `failed` with an explicit `reclaimed_by` source reference instead of
  lingering as fake active work.
- Diagnostics also separates source acquisition work into due source tasks,
  stale `fetching` tasks, and provider/action pressure. Due tasks are normal
  backlog; stale fetching tasks are provider work that did not finish inside
  the reclaim window; provider/action pressure shows whether the bottleneck is
  broad `fmp_news` fetching, a single `sec_companyfacts_xbrl` failure, or a
  provider-wide rate-limit pause.
- The Source health diagnostics table renders stale `running` provider rows as
  `stale running`, so the operator can distinguish an active pass from a pass
  whose worker exceeded the reclaim window.

Current gaps:

- #128: source loops, normalized evidence producers, and cognition still
  coordinate through shared tables rather than a single top-level controller
  process, but provider outcomes and evidence deltas now feed cognition target
  selection and the run ledger makes the coordination observable.
- #128: provider-wide retry gates now pause source tasks, and due source tasks
  move their symbols to the front of the expensive ingest scan universe. The
  FMP price, estimates, analyst-opinion, news, XBRL, EDGAR, FRED, and CBOE
  loops now claim/complete their source tasks directly.
- #136: evidence requirements and source tasks are synchronized from source
  health. Remaining producer adoption work is for future factor/commodity
  adapters and attention retry/blocked transitions.
- #130: product/theme web retrieval has a first GDELT/Bing-backed slice; paid
  semantic search may still be needed if recall is too weak.
- #93: normalized evidence items cover news, estimate revisions, analyst
  price-target events, discovery price action, product/theme web research, and
  context shifts. Context and thesis prompts receive those facts directly,
  thesis drafting/reconciliation records `thesis_evidence` links for the
  current fact set, and the thesis UI shows those linked facts. Remaining work
  is more selective LLM-picked support/contradiction labels.

Selected-symbol status now exposes the first slice of #128:

```text
symbol overview
  -> /api/brain-status?symbol=MU
  -> source freshness
     - last checked or evaluated
     - last changed
     - retry_after when rate-limited/failed
     - source-specific detail: session coverage, latest publication, version/state
     - symbol-scoped source_task rows for each source group
     - analyst opinion freshness: price target snapshots, recommendation mix,
       and recent target-change events
  -> normalized evidence freshness
     - latest evidence_item.updated_at for the symbol
     - whether that evidence is newer than the open thesis evaluation
     - whether that evidence is newer than the last no-thesis decline
  -> evidence rows/open/blocking/due
  -> context age
  -> thesis age
  -> open attention count
  -> deterministic status + next_action
```

That endpoint is intentionally derived from existing tables. It does not
mutate state and it does not replace the orchestrator. Its job is to make the
brain loop legible: every ticker should say `fresh`, `due`, `stale`,
`waiting_on_evidence`, or `blocked`, plus the next system action. A provider may
be globally fresh while a symbol still has queued or rate-limited work; the
selected-symbol Brain card therefore shows both the provider freshness row and
the ticker's local source tasks. Normalized evidence has its own row because a
source can be checked successfully while the important operator question is
"did a fact change after the thesis last digested this symbol?" If the latest
`evidence_item.updated_at` is newer than the open thesis evaluation, the next
action is `reevaluate_after_evidence_update`; if there is no thesis and the
latest evidence is newer than the last declined attempt, the next action is
`draft_after_evidence_update`. That mirrors the cognition sweep's
`evidence_delta` trigger so the operator explanation and the scheduler's work
selection use the same freshness signal.

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
- Draft/reconcile runs persist `known_unknowns` on the thesis: explicit
  material unanswered questions, what to watch for, and the evidence source or
  check date when known. If the model omits them, the engine derives them from
  missing evidence so an active ticker is not silently uncertainty-free.
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
- #97: thesis detail now computes deterministic input freshness from market
  source checks, ticker context age, estimate age, and recent-news coverage.
  The score, component penalties, and confidence cap are shown beside the
  structural substance checklist, and promotion into `actionable` is blocked
  below the high-confidence freshness threshold. The staleness loop emits
  `context_stale` attention when an actionable-or-later thesis is still relying
  on narrative context older than 30 days. News-derived consensus components
  now require at least three articles in the last 14 days instead of treating
  thin coverage as a usable signal.
- #90 implemented: thesis rows now carry machine `system_confidence` plus
  `system_confidence_components`; decision rows carry operator
  `human_conviction` plus freeform `reason`. Replay snapshots preserve both so
  outcomes can later compare system/human agreement and override quality.
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
  snapshots parent macro/sector/theme links into prediction.claim
  groups calibration by parent theme + ticker expression role
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
- #129 implemented: parent theses have deterministic maintenance, a bounded LLM
  rewrite pass from normalized linked evidence, calibration snapshots, and
  selected-ticker thesis detail links. Remaining macro/factor/commodity breadth
  belongs to data-source coverage work, not the parent-thesis product model.
- #89: review packet pattern for every attention-resolution flow.
- #126: explicit workflow rail.
- #82: terminology still needs simplification across UI/docs.
- #119: symbol alerts tab should not show unrelated global alerts.
- #118: live event stream in Vite dev still has tracked rough edges.
- #128: selected-symbol freshness status is visible, and global Diagnostics
  now expose provider/action source-task pressure with due counts, stale
  fetching counts, next due time, and sample symbols. The remaining gap is that
  the top-level orchestrator still does not actively schedule every stale
  source.

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
  broad parent themes for financials, energy, staples, and housing/rates
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
  thesis_review attention for material thesis reconciliation changes
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
  Brain Journal materializes daily entries from deterministic event tables

missing
  full producer adoption for attention retry/blocked transitions
  broader macro/factor/commodity data coverage for parent thesis generation
  persisted technical_state history and prompt-loop ingestion
  chat analyst routed prompt loop
  LLM synthesis pass over deterministic Brain Journal entries
  paid semantic research provider if GDELT recall is insufficient
  real broker/position execution state
  review packets
  decision replay
```

## Highest-Leverage Next Work

1. #128: make freshness orchestration real.
2. #147: finish producer adoption for waiting/retry/blocked attention states.
3. Remaining #130 uplift: improve product/theme evidence recall for real forward views.
4. #131/#25/#5: link decisions to real positions/fills.
5. Add the optional Brain Journal synthesis prompt over deterministic entries.
