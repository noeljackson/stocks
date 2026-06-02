# Thesis-Driven Trading Intelligence System — Specification v1

> Status: Design complete (interview-derived). Ready for implementation.
> Product framing: **Product A — Investment Intelligence Amplifier.** The human is the alpha source; the system makes the human faster and more disciplined. Autonomous execution (Product B) is explicitly deferred to v2.

---

## 0. Core Premises (the spine — do not lose these)

These are the load-bearing decisions. Every requirement below serves them.

- **Product A, not B.** v1 produces *trade ideas for a human to evaluate and execute manually*. It does **not** trade autonomously. Success = better/faster discretionary decisions, **not** autonomous P&L.
- **The edge, named honestly.** Be *earlier than your own past information bandwidth* — and earlier than the retail crowd — at spotting **evidence-backed market inflections before the FOMO**. The mechanism is **information diffusion**: public facts are not instantly *priced*; the gap between "available" and "fully diffused" is the trading window (cf. post-earnings drift). Tech infrastructure is an important current theme, not a hard boundary. Copper, wheat, financials, staples, energy, healthcare, and any other liquid market can matter when the evidence creates a falsifiable money-making view. The benchmark is the relevant passive alternative for the decision (broad market, sector ETF, commodity proxy, or sector beta) — *not* institutions. Expected outcome is **modest and compounding (~9–13% annualized target)**, not 20%+.
- **The edge is not predictive.** It is (1) information synthesis the user couldn't do manually at scale, (2) systematic discipline most retail never applies, (3) a cost structure the user controls. None claim to out-forecast institutions.
- **Core product statement (verbatim):** *"LLM combining technical analysis with informational context to generate trade ideas for me to evaluate and execute manually. The technical scanner detects events; the context layer maintains qualitative state per ticker; the LLM combines them into trade ideas. The system's value is producing better trade ideas than I'd generate manually, faster than I could synthesize the inputs myself."*
- **North star — flexibility.** The system must get *better with more data*: pluggable indicators/signals/prompts, an extensible taxonomy, and a recorded rationale on every decision so the corpus becomes a learning signal.
- **Validation is forward and observable.** The primary edge **cannot be backtested** (no clean point-in-time data; LLM hindsight contamination). It is validated forward by **lead-time-to-consensus** + **forecast calibration**, benchmarked against the relevant passive alternative for each decision.

---

## 1. System Overview

A single-operator, thesis-driven trading **intelligence** platform that continuously maintains synthesized, structured context on a curated universe of liquid equities and market factors, scans a broader universe to *discover* emerging theses before they reach consensus, and uses an LLM to combine that qualitative context with technical/regime signals into structured trade ideas. Each idea is a versioned **thesis** with an explicit edge rationale, falsifiable forecast, and machine- and narrative-evaluable conditions for conviction, entry, invalidation, and exit. In v1 the system **alerts a human who decides and executes manually**; an independent risk overlay enforces hard limits; a reflection layer compares thesis to outcome and feeds the corpus that tunes discovery over time. The system is built so the context layer is valuable as a standalone thinking tool even before any trade is placed, and so that semi-autonomous execution can be added later (v2) by changing *who pulls the trigger*, not the underlying objects.

---

## 2. Functional Requirements

1. **Continuous context maintenance.** For Tier-1 tickers, ingest and LLM-synthesize a persistent, structured, multi-band per-ticker context that evolves over time (not a point-in-time snapshot).
2. **Universe discovery.** Continuously scan a bounded-but-broad "haystack" with cheap signals to surface candidate tickers the user does not already track, and *promote* them into deeper monitoring. The system **maintains its own universe**; the watchlist is an output, not a hand-curated input.
3. **Tiered scope.** Maintain three tiers (deep context / light monitoring / price-only) with explicit, automated promotion and demotion.
4. **Market state & regime.** Classify a market regime from configurable indicators and emit a capitulation flag; use regime to gate tactical strategies and modulate thesis-driven sizing/timing.
5. **Thesis generation.** When context + technical + regime align, generate a structured thesis (the system's primary output) and advance it through a defined lifecycle state machine.
6. **Condition evaluation.** Continuously evaluate each active thesis's conviction/trigger/invalidation/fulfillment conditions, both quantitative (over indicators) and narrative (LLM over context).
7. **Alerting.** Emit alerts only on *significant context shifts* — state-machine transitions, completed alignment, or consensus arrival.
8. **Decision capture.** Present actionable theses to the user (alert / confirm modes in v1) and record the user's decision and the actual trade.
9. **Position tracking.** Track open positions against their thesis conditions; surface invalidation/fulfillment.
10. **Independent risk overlay.** Validate every proposed trade and continuously monitor the portfolio against hard limits, structurally separate from the idea generator, with veto authority.
11. **Reflection.** After every closed *or disqualified* thesis, compare thesis to outcome, record the post-mortem, and feed signal-weighting back to discovery.
12. **Integrity / anti-self-deception.** Theses are append-only and versioned; the original edge rationale and invalidation conditions are immutable; the system flags revisions that *weaken* the original invalidation ("tell us if we're nuts").
13. **Extensibility.** Indicators, discovery signals, and prompts are versioned registry components; regime classifier, promotion rule, and thesis conditions are config expressions over them. Every alert/decision records which component+config versions produced it.
14. **Self-built point-in-time corpus.** All ingested data is stored append-only and ingestion-timestamped; history is never overwritten.
15. **Validation harness.** Measure lead-time-to-consensus per alert and forecast calibration per thesis; benchmark realized decisions against the relevant passive alternative.

---

## 3. Architecture

Event-driven services over **NATS (JetStream)**; **Postgres** for state and the append-only event/raw store; **Rust** for deterministic services, ingestion, indicator/risk engines, and the UI gateway; **Python** for LLM/ML/research work. All services are stateless where possible and replay durable JetStream streams (so improved prompts/indicators can reprocess history — this is how the system "gets better with data").

| Service | Lang | Responsibility | Key in/out (NATS subjects) |
|---|---|---|---|
| **Ingestion adapters** | Rust (Python for parsing/research where needed) | One adapter per source (EDGAR, FRED, price, news, estimates, transcripts, options). Append-only raw write + normalized event. | out: `ingest.filing`, `ingest.price`, `ingest.news`, `ingest.estimate`, `ingest.transcript`, … |
| **Event router** | Rust | Dedup, fan-out, route ingested events to relevant ticker contexts and to discovery. | in: `ingest.*`; out: `route.ticker.<sym>`, `route.market` |
| **Context maintainer** | Python (LLM) | Maintain the 3-band per-ticker context for Tier-1 (and first-pass for promoted Tier-2). Synthesize structural & narrative bands; market band is raw. Emit shifts. | in: `route.ticker.>`; out: `context.updated`, `context.shift` |
| **Market state / regime** | Rust | Compute indicators (registry); classify regime; emit capitulation flag; sub-sector relative strength. Config-driven. | in: `route.market`; out: `regime.state`, `regime.capitulation`, `subsector.rs` |
| **Discovery scanner** | Rust + Python | Run cheap signal detectors over the haystack; promote candidates; trigger first context pass + tier change. | in: `ingest.*`, `subsector.rs`; out: `discovery.candidate`, `tier.promote/demote` |
| **Thesis engine (opportunity + analysis)** | Python (LLM) | The heart. Form/advance theses (state machine); evaluate conditions (quant via indicator engine, narrative via LLM); generate ideas; run goalpost detector on revisions. | in: `context.*`, `regime.*`, `discovery.*`; out: `thesis.actionable`, `thesis.invalidated`, `thesis.fulfilled`, `thesis.updated` |
| **Risk overlay** | Rust | **Independent** veto layer. Pre-trade check + continuous portfolio monitor against hard limits. Cannot be bypassed by the thesis engine. | in: `thesis.actionable`, position state; out: `risk.veto`, `risk.warning`, `risk.ok` |
| **Decision / alert layer + UI gateway** | Rust + Svelte SPA | Embed the built Svelte SPA (`rust-embed`); serve REST (actions) + SSE (live feed off NATS); deliver alerts (alert/confirm); capture decisions and trades. | in: `thesis.*`, `risk.*`; out: `decision.recorded`; HTTP/SSE ↔ browser |
| **Execution & position tracking** | Python (ib_insync) / Rust | v1: sync broker positions/fills, track positions vs thesis conditions. v2: place orders gated by risk overlay. | broker API ↔ `position.updated` |
| **Reflection** | Python (LLM) | Post-trade/post-disqualification analysis; thesis-vs-outcome; corpus; feed signal weights to discovery. | in: `thesis.fulfilled/invalidated`, `decision.recorded`; out: `reflection.recorded`, `signal.weight.update` |
| **Config / registry** | Rust + Postgres + prompt files | Versioned indicator sets, signal sets, prompts, thresholds, taxonomy. Serves all services. | request/reply `config.get`, `config.version`; prompt name/hash audit |

**Architectural invariants**
- **Provider abstraction:** all LLM calls go through one interface; provider/model is swappable (model upgrades, fallback, A/B). (User has elected to use subscription-based LLM access for v1 — this abstraction keeps that decision reversible.)
- **Separation of powers:** the thesis engine *proposes*; the risk overlay *constrains*; the human *decides*. No component both proposes and approves.
- **Append-only everywhere:** raw data, context versions, thesis versions, decisions — all immutable history with ingestion timestamps.
- **Everything that's a "signal" is a registry component** addressed by name+version; composites are config.

---

## 4. Data Requirements

| Source | Provides | Frequency | Cost | Criticality |
|---|---|---|---|---|
| SEC EDGAR | 10-K/Q, 8-K, S-1, 13F, Form 4 | event | free | **Critical (MVP)** |
| FRED | rates, HY OAS, yield curve, macro | daily | free | **Critical (MVP, regime)** |
| Price/volume (Tiingo/Stooq free tier) | OHLCV | daily | free | **Critical (MVP)** |
| Free news (IR feeds, RSS) | headlines | continuous | free | Important (MVP, low quality) |
| Sentiment (AAII, CNN F&G, put/call) | regime + capitulation inputs | daily | free/cheap | Important (regime) |
| **Analyst estimate revisions** (FMP / Finnhub / Zacks) | estimates + revision trend | daily | ~$20–60/mo | **High — #1 leading signal for the edge** |
| Earnings-call transcripts (FMP / Finnhub) | call text | quarterly | bundled | High (Phase 2 #2) |
| Cleaner price + fundamentals (Tiingo / Polygon) | OHLCV + fundamentals | daily/intraday | ~$30–100/mo | Medium (Phase 2 #3) |
| Structured/entity-tagged news (Benzinga / Marketaux) | tagged news | continuous | ~$50–100/mo | Medium (Phase 2 #4) |
| Options (Polygon / ORATS) | chains, OI, IV | daily | ~$$ | Medium (when LEAPS active) |

**Budget:** MVP **< $50/mo** (free tier only); full prosumer **$200–500/mo**, scaled up *in ladder order as the edge proves out*.
**Quality stance:** the user does **not** buy point-in-time data. The system *accumulates* it via append-only, ingestion-timestamped storage. Free data's lack of PIT/survivorship-bias-free history is acceptable because (a) the primary edge is forward-only anyway and (b) recording-as-it-arrives yields native point-in-time. Backtesting is **not** a primary requirement (the discovery edge is not backtestable).

---

## 5. Schema Specifications

> Storage: **plain Postgres (PG17+)**. **Typed core + JSONB** for the flexible/extensible parts (3-band context, condition expressions). Time-series tables (price, indicators, market_state, ingest log) use **BRIN indexes** on the time column — the data is small at this scale (≈150k daily bars/year across the haystack), so vanilla Postgres is ample. TimescaleDB is a deferred, drop-in option (`CREATE EXTENSION` + `create_hypertable()`, no schema rewrite) if volume ever justifies it. Money as `numeric`/decimal, never float.

### 5.1 Ticker (persistent monitoring object)
```
ticker(
  symbol            text primary key,
  cluster           text,              -- taxonomy sub-sector (seed-extensible)
  tier              int,               -- 1 | 2 | 3
  status            text,              -- active | archived
  options_eligible  bool,              -- passes chain-liquidity gate
  added_at, last_promoted_at, last_demoted_at  timestamptz
)
```

### 5.2 Ticker context (3 bands, each with its own freshness)
```
ticker_context(
  symbol            text references ticker,
  structural        jsonb,  structural_as_of   timestamptz,  -- quarters: fundamentals,
                                                              -- competitive position, end-market
                                                              -- growth, LAGGED 13F/short interest
  narrative         jsonb,  narrative_as_of    timestamptz,  -- days-weeks: themes, analyst
                                                              -- trajectory, pending catalysts,
                                                              -- monitored risks
  market            jsonb,  market_as_of       timestamptz,  -- daily: price/vol, technicals,
                                                              -- LIVE options/flow, sentiment
                                                              -- (cheap feeds, NOT LLM-synthesized)
  version           int,
  primary key (symbol, version)         -- append-only; latest = max(version)
)
```
*Rule:* lagged positioning (13F, short interest) lives in **structural**, never market — avoids the staleness bug where a price tick makes quarter-old data look fresh.

### 5.3 Thesis (the lifecycle object = the §3 state machine)
```
thesis(
  thesis_id         uuid primary key,
  symbol            text references ticker,
  cluster           text,
  cluster_thesis    text,              -- parent theme, e.g. "AI capex -> memory demand"
  state             text,              -- forming | building_conviction | armed |
                                       -- actionable | position_open | exiting |
                                       -- closed | disqualified
  -- THE WHY (the idea)
  bull_case         text,
  bear_case         text,
  edge_rationale    text not null,     -- REQUIRED: what is not yet priced & why still diffusing
  historical_analogs jsonb,            -- corpus links
  -- VALIDATION INSTRUMENT (does NOT drive exits)
  forecast          jsonb,             -- {direction, magnitude_rough, horizon}
  -- CONDITIONS (each: {type: quantitative|narrative, expr|assertion, evaluator})
  conviction_conditions  jsonb,        -- building_conviction -> armed
  trigger_conditions     jsonb,        -- armed -> actionable (technicals+macro+subsector RS)
  invalidation_conditions jsonb,       -- -> exiting/disqualified (quant AND narrative)
  fulfillment_conditions  jsonb,       -- exit = consensus arriving (also lead-time anchor)
  -- EXECUTION LINKAGE (advisory)
  conviction_tier   text,              -- high | medium | low -> size band (human sets final)
  instrument        text,              -- equity | leaps
  intended_size     jsonb,
  -- INTEGRITY
  version           int,
  immutable_original jsonb,            -- frozen edge_rationale + invalidation @ v1
  created_at, updated_at  timestamptz
)
thesis_state_history(thesis_id, from_state, to_state, rationale, at)   -- append-only
thesis_version_history(thesis_id, version, diff, rationale, weakens_invalidation bool, at)
```
*Two strategies, one object:* discovery theses run the full lifecycle; **tactical dip-buys are "born `armed`"** (skip `building_conviction`; go to `actionable` when `risk_on` + capitulation fire on a watchlist name).

### 5.4 Market state / regime
```
market_state(
  as_of             timestamptz primary key,
  regime            text,              -- risk_on | neutral | risk_off
  capitulation      bool,
  indicators        jsonb,             -- {name@version: value} for every registered indicator
  subsector_rs      jsonb,             -- relative strength per taxonomy cluster
  config_version    text
)
```

### 5.5 Execution / position state
```
position(
  position_id       uuid primary key,
  thesis_id         uuid references thesis,
  symbol            text,
  instrument        text,              -- equity | leaps
  qty, avg_price,
  delta_notional    numeric,           -- counts toward 15% name cap
  premium_at_risk   numeric,           -- counts toward options aggregate cap
  opened_at, closed_at, realized_pnl
)
decision(decision_id, thesis_id, action, user_choice, sizing, at)  -- append-only
```

---

## 6. Core Algorithms

Two computed rules are load-bearing enough to specify here rather than defer to implementation. Both are **config-versioned** (weights/thresholds pluggable, per the §3 registry principle) and both **propose to the human, who corrects** — and corrections become training signal (the "better with data" loop).

### 6.1 Tier-1 Inclusion — domain-fit score → promotion

A ticker is Tier-1-eligible when it passes the **hard tradeability filters** AND its **domain-fit score** clears the threshold AND it has an **active discovery signal or a user flag**.

**Hard tradeability filters (necessary — haystack membership):**
- US-listed
- Market cap ≥ $1B (else routed to the sub-floor, equity-only, smaller-size sleeve)
- Avg daily dollar volume ≥ liquidity floor (default $5M/day)

**Domain-fit score (0–100, config v1 weights):**

| Component | Max pts | Source |
|---|---|---|
| Primary business in a seed/emerging cluster | 40 | LLM classification + filings |
| Domain revenue exposure ≥ threshold (default 40%) | 25 | fundamentals / filings |
| Value-chain adjacency to an existing Tier-1 thesis (supplier/customer/competitor) | 20 | context-layer relations |
| Circle-of-competence affinity (cluster the user has engaged: open/closed theses or manual flags) | 15 | corpus / decisions |

**Promotion (Tier-1, ~25–30 cap):** domain-fit ≥ threshold (default 60) AND (active discovery signal OR user flag). The system **proposes** promotion with the score breakdown; the user confirms/rejects. Confirmations up-weight and rejections down-weight the circle-of-competence component — this is how the system **learns the user's actual edge boundary instead of guessing it**. **Demotion** per §2 (no actionable signal in ~6 months AND no open position).

*Implements the user's intent directly: he is not the taxonomy expert; the system proposes membership from the seed clusters and he corrects it.*

### 6.2 Consensus Formation — exit trigger AND validation anchor

The most-reused event in the system: it fires **fulfillment/exit** for discovery theses ("sell it to the crowd") and is the **validation anchor** for lead-time (`lead_time = t(consensus) − t(alert)`). Must be observable and timestamped.

**Consensus score (0–100, config v1 weights):**

| Component | Source | MVP (free)? |
|---|---|---|
| Analyst coverage/upgrade expansion (N initiations/upgrades in window) | estimate-revision feed | Phase 2 |
| Estimate-revision saturation (the early inflection is now the base case) | estimate-revision feed | Phase 2 |
| Mainstream-media coverage (source-tier shift: specialist → mainstream) | news + LLM source-tier tag | partial |
| Retail attention surge (social volume, frothy call-skew, falling put/call) | sentiment + options | partial |
| Price extension / technical exhaustion (distance above MAs, RSI, parabolic volume) | market band | **yes** |

**Two thresholds, independent:**
- **Measurement threshold** (default 60) — first crossing timestamps "consensus formed" for lead-time accounting.
- **Exit threshold** (default 70) — crossing fires `fulfillment_conditions` → `exiting`. (Tactical/dip theses instead use their own fulfillment: index recovery to within X% of the pre-drawdown high.)

**Graceful degradation:** MVP computes the free components (price extension, basic news volume); the full version adds coverage/estimates/social. **The elegant loop:** the #1 data feed (estimate revisions) is *both* the entry leading signal *and* a consensus/exit component — the same data tells you when you're early and when the crowd has arrived.

---

## 7. Integration Points

- **Broker — Interactive Brokers (default).** v1: read positions/fills for the tracking layer; manual order entry by the user. v2: order placement, gated by the risk overlay. Bridge via **`ib_insync` (Python)** as a small dedicated service (Tradier is the fallback if a simpler options-first API is preferred).
- **LLM provider — behind one interface.** v1 uses the user's subscription-based access (user's decision, recorded). The abstraction permits a drop-in switch to the API (with prompt caching + Batch + model tiering) without touching call sites. Suggested model tiering when on API: **Opus** for deep thesis synthesis, **Sonnet** for routine context updates, **Haiku** for triage/classification.
- **Data vendors — common adapter interface.** Each source implements `fetch -> normalize -> append-only store + emit`. MVP adapters: EDGAR, FRED, free price, free news. Phase-2 adapters: FMP/Finnhub (estimates + transcripts), Tiingo/Polygon (price+fundamentals), Benzinga (news), Polygon/ORATS (options).
- **Messaging — NATS JetStream.** Durable, replayable streams per subject family; enables reprocessing history through improved components.

---

## 8. Risk and Operational Requirements

**Framing:** the user is a **concentrated specialist by design**. The product edge is not sector purity; it is finding evidence-backed opportunities that can make money before they become obvious. The risk overlay does **not** force sector diversification for its own sake. It controls name-level blowup, total drawdown, leverage, liquidity, and regime, and it surfaces concentration when a theme becomes crowded.

**Hard limits (config v1, independent veto):**

| Limit | Value | Type |
|---|---|---|
| Single name (delta-adjusted notional) | 15% | hard veto |
| Aggregate options premium-at-risk | 15–20% | hard veto |
| Cash floor | 20% | hard |
| Drawdown brake | −10% → new sizing ×0.5; −20% → halt new entries | hard |
| Regime throttle | `risk_off` → throttle thesis entries, halt tactical | hard |
| Sub-sector cluster concentration | ~35% | **soft / warning** |
| Concurrent positions | 5–7 | guideline (tracking-quality limit) |

**Sizing:** conviction-tiered (high+strong 8–12%; high+borderline 4–6%; medium 3–5%; below → skip), modified by correlation/sector/drawdown. **Conviction is advisory** — proposed by the thesis engine, bounded by risk overlay, **final size set by the human**.
**Options accounting (two views):** delta-notional → name cap; premium → options-loss cap. LEAPS only on names passing the chain-liquidity gate (bid-ask < ~10% of premium, OI floor, real volume); sub-floor discovery names are **equity-only**.
**Auto-execution:** **none in v1** (confirm everything). v2 grants per-trade-type auto-execution only after v1 demonstrates that type's `actionable` alerts were high-precision over the 18–24mo evaluation.
**Monitoring/alerting (ops):** ingestion liveness per source; context staleness per band per Tier-1 name; LLM cost/usage; regime transitions; every hard-limit breach. Alerts on significant context shifts only (state transition / completed alignment / consensus arrival).

---

## 9. Build Sequence

Each phase is independently useful; the system can stop at any phase and still deliver value.

- **Phase 0 — foundation (weeks).** NATS/JetStream, Postgres schema, provider abstraction, config/registry, ingestion framework, append-only store. *Gate: events flow end-to-end and are replayable.*
- **Phase 1 — context layer (months 1–6).** Free-source ingestion; 3-band ticker context; LLM context maintainer over a ~10-name seed Tier-1 (incl. SPY); state-machine skeleton. *Gate: context is accurate and current enough that the user **actually consults it** to answer "what's the state of X?" — useful with zero trading.*
- **Phase 2 — discovery + alerts + tracking (months 6–9).** Discovery scanner over the haystack; regime service; thesis engine emitting `actionable` alerts; manual position tracking; risk overlay pre-trade checks. *Gate: alerts are relevant; lead-time-to-consensus is measurable; false-positive rate tolerable.*
- **Phase 3 — iterate (months 9–15).** Tune signals/prompts against the recorded corpus; add paid data in ladder order; build reflection layer + signal-weight feedback. *Gate: lead-time + calibration positive in aggregate; **beats the relevant passive benchmark** on decision quality.*
- **v2 — semi-autonomous execution (after 18–24 months of real use).** Per-trade-type auto-execution, earned only by demonstrated alert precision; same objects, risk overlay becomes the hard gate on the execution path.

**Validation harness (primary metrics):**
1. **Lead-time-to-consensus** — per alert: timestamp(alert) → timestamp(consensus/fulfillment). Forward, point-in-time. ~30–50 data points/yr (alert-level), far more than trade-level.
2. **Forecast calibration** — per thesis: logged forecast vs realized (scoring only; never drives exits).
3. **Benchmark** — realized decisions vs the relevant passive alternative, not vs zero.

**Kill criterion:** if after ~12 months the system is not materially beating the relevant passive alternatives on decision quality, it is **reassessed, not extended**. No sunk-cost continuation. (Even on kill, the context layer remains a useful research tool — but that is a consolation, not the goal.)

---

## 10. Open Questions

> Resolved since the v1 draft: **Tier-1 inclusion test** and **consensus-formation** are now specified in §6 (Core Algorithms).

1. **Market-cap floor** for the haystack — recorded default **$1B** with a sub-floor "watch-only, equity-only, smaller-size" sleeve. *User has not explicitly set; override if desired.*
2. **Broker** — recorded default **Interactive Brokers**; user did not name one. Confirm or substitute Tradier.
3. **Taxonomy seed** — 9 clusters ratified as a *seed*; the precise boundaries (e.g., where "datacenter power/cooling" ends and "industrials with DC exposure" begins) need first-pass operational rules. The system is expected to surface *emerging* clusters not in the seed.
4. **Drawdown-brake measurement** — peak-to-trough window and reference (portfolio high-water mark vs rolling).
5. **LLM subscription operationalization** — user asserts their terms permit programmatic use; the provider abstraction is the hedge if that ever changes.

---

## 11. Specific Implementation Recommendations (Rust / Python / Postgres / NATS / Hetzner)

- **Rust services:** ingestion adapters, event router, indicator/evaluator services, regime service, discovery scanner, risk overlay, decision/alert gateway, config/registry. Libraries: `async-nats` (JetStream), `sqlx` (Postgres), `axum` (HTTP/SSE), `rust_decimal` (money). Keep each service single-responsibility and replay-driven.
- **Python services:** context maintainer, thesis engine, reflection, research/notebooks, and the **ib_insync** broker bridge. Use **pydantic** for structured LLM outputs (force schema-valid theses/conditions); `polars`/`pandas` for research; the LLM SDK strictly behind the provider interface.
- **Postgres (vanilla):** typed core + **JSONB** for the flexible bands and condition expressions (serves the extensibility north star); **BRIN indexes** on time-series columns; everything append-only with ingestion timestamps. One logical DB is sufficient at solo scale. (TimescaleDB is a deferred drop-in if volume ever grows.)
- **NATS JetStream:** durable streams per subject family (`ingest.*`, `context.*`, `thesis.*`, `regime.*`). Durability + replay is what makes "reprocess history through improved prompts/indicators" — i.e. *better with data* — actually work.
- **LLM cost (if/when on API):** prompt caching (cache per-ticker context, pay only deltas), Batch API (50% off non-urgent synthesis), model tiering (Haiku triage → Sonnet routine → Opus deep). At this scale, API LLM cost is ~$20–60/mo; data is the real cost — engineer cost there, not on the LLM.
- **Deployment — Kubernetes (your cluster):** one Deployment + Service per service; Ingress in front of the decision/UI gateway. **NATS JetStream** via the official Helm chart with PVCs for stream storage. **Postgres** (vanilla PG17+) via **CloudNativePG** (clean operator) or a StatefulSet on a backed-up PV. (TimescaleDB deferred — adopt as an extension only if data volume grows.) Secrets via k8s `Secret` (sealed-secrets / external-secrets for GitOps). Config/registry seeded from ConfigMaps or the DB. Multi-stage builds: Rust → distroless/scratch-compatible runtime image, Python → slim. **Local dev: docker compose** (Postgres + NATS + services), or skaffold/Tilt against kind/k3d for cluster parity.
- **UI — Svelte SPA embedded in Rust (`rust-embed`):** the operator UI is a Svelte + TypeScript SPA built and embedded into the gateway binary. Live push via **SSE** (server→client alert/state stream off NATS), **REST** for confirm/decision actions; charts via TradingView **lightweight-charts** + **uPlot** for dense indicator series. The target UX is a chart-first workstation: selected symbol, chart, watchlist-driven navigation, context/theses/alerts/decisions side panel, and bottom workflow drawer. *(Python UI — Streamlit/marimo/NiceGUI — remains the right tool for disposable **research** dashboards beside the Python research code; not for the production operator UI.)*
- **Registry/config:** store indicator sets, signal sets, prompts, thresholds, and taxonomy as **versioned config in Postgres**; stamp every `market_state`, alert, and decision with the config version that produced it. This closes the learning loop.

---

*End of specification v1. Implementable as written; resolve §10 Open Questions during Phase 0–1.*
