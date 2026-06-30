# Data sources

The system pulls data from a small set of vendors. This doc is the **single
source of truth** for what each piece of data is, why we need it, which vendor
it comes from, what tier/cost that requires, and the current wiring status.

When you add a new data source: edit this doc *first*. When you find a gap:
file an issue and link it from the relevant row's "status" column.

**Status legend:**
- `wired` — running in production, has tests, has audit trail
- `partial` — a useful first slice is wired, but the row still has named gaps
- `key-only` — API key in Infisical but no adapter code yet
- `not wired` — vendor identified, no key, no code
- `gap` — known limitation, no fix planned

**Current vendor stack (as of 2026-06-03):**
- **FMP Starter** ($22/mo) — primary: daily/intraday price bars, screener
  discovery, analyst estimate snapshots/revisions, price-target consensus,
  recommendation mix, price-target events, global grade-change events,
  company profile, earnings calendar, and company news without upstream
  sentiment
- **Massive Stocks Starter** ($29/mo) — kept for: stock news with per-article
  sentiment where FMP has no equivalent
- **SEC EDGAR** (free) — CIK lookup, filing metadata, and XBRL company facts;
  insider transactions and 13F holdings are still gaps
- **FRED** (free) — selected macro series (`DGS10`, `DGS3MO`,
  `BAMLH0A0HYM2`, `VIXCLS`), not a full macro/breadth library yet
- **CBOE** (free, no key) — equity put/call ratio and VIX close for
  crowd-sentiment/regime inputs
- **TWSE** (free, no key) — official daily OHLCV fallback for numeric Taiwan
  listings such as `2454.TW` when FMP search resolves the symbol but EOD
  history is entitlement-gated
- **GDELT Doc 2.0** (free, no key) + **Bing News RSS** (free, no key) —
  fallback web/news search for product and theme evidence that vendor
  symbol-news misses
- **z.ai** (~cents/call) — LLM provider for cognition layer, plus the universal
  sentiment classifier (so any news source we add later — RSS, Twitter, future
  paid feeds — gets sentiment-scored without depending on a vendor's own scorer)

---

## 1. Price + market data

| Data | Why | Vendor | Tier / cost | Endpoint | Status |
|---|---|---|---|---|---|
| Daily OHLCV bars | Discovery signals (volume_anomaly, base_breakout), evaluator (`SYMBOL.close`), consensus price_extension component | **FMP** primary; **TWSE** fallback for numeric `.TW` symbols | FMP Starter $22/mo; TWSE free/no key | FMP `/stable/historical-price-eod/full?symbol=&from=&to=`; TWSE `/exchangeReport/STOCK_DAY?response=json&date=&stockNo=` monthly bars | wired — `src/ingest/fmp.rs` plus `src/ingest/twse.rs`; tiered deep universe plus benchmarks; `.TW` price history uses TWSE so symbols like `2454.TW` can chart and enter technical analysis |
| Intraday bars (1m/5m/15m/30m/1h/4h) | TradingView-style chart intervals; 3m/2h are aggregated from native bars | FMP | Same plan | `/stable/historical-chart/{interval}` | wired — `src/ingest/fmp_intraday.rs` |
| Company screener / discovery pool | Broad radar for liquid equities across technology, financials, staples/agriculture, energy, materials/metals, healthcare, industrials, real estate, utilities, consumer, and communications | FMP | Starter | `/stable/company-screener` by sector slice | wired — `src/ingest/fmp_screener.rs`; creates `research_nomination` candidates with explicit sector/theme/business/evidence reasons for unreviewed pool names |
| Commodity/factor proxy bars | First slice for copper/wheat/factor parent theses via tradable proxies such as CPER, WEAT, and XME | FMP | Starter | same EOD/intraday endpoints as equities/ETFs | wired as price bars when proxies are active tickers; direct futures/inventory/USDA/weather remain separate gaps |
| Options chains | LEAPS thesis instrument selection (#5 epic) | Massive Options Starter $29/mo extra (or FMP has thin coverage — verify before swap) | `/v3/snapshot/options/{underlying}` (Massive) | not wired |
| Corporate actions (splits/dividends) | Implicit — FMP serves adjusted close | FMP (built into adjusted prices) | included | implicit |
| Live aggregate bars / websocket | TradingView-style in-progress candle updates while the operator watches a symbol | FMP delayed polling now; future upgrade path is Massive/Polygon-style aggregates or IBKR market data with entitlement caveats | FMP Starter included; true websocket provider varies by entitlement | FMP `/stable/historical-chart/1min` polled by `fmp_live_bars` | partial - ingest publishes delayed `market.bar.<interval>.<symbol>` events for active symbols, gateway fans them out over `/api/stream`, and `ChartPanel` patches/appends the visible candle for matching symbol+interval; true websocket adapter tracked in #278 |
| Technical analog package | Multi-timeframe RSI/SMA state, time-in-zone, prior 200-day SMA cross behavior, forward paths after similar regimes | Derived from price bars | n/a | local computation over `price_bar` and `price_bar_intraday` | not wired as first-class API/table; chart displays SMA ribbon and RSI, thesis prompt receives daily `price_snapshot` |
| Automation market readiness | Proof gates for missing/stale/suspicious market data, session state, halt/suspension flags, adjusted corporate-action handling, and no-trade windows | FMP adjusted EOD bars plus local US equity calendar and strategy config; halt/suspension provider still future | n/a | `strategy-runner` reads latest `price_bar`, derived technical state, `market_calendar`, and strategy config (`max_bar_age_days`, `max_bar_gap_pct`, `corporate_actions_adjusted`, `halt_state`, `no_trade_windows_utc`) | wired - market readiness is persisted inside `automation_proof.data_freshness.market_readiness` and hard-blocks desired-state emissions; halt/suspension defaults to `not_halted` until a provider is wired |
| Shadow strategy features | Automation desired-state signals, reason codes, config-hashed feature snapshots, proof snapshots, allocator snapshots, simulator reconciliation, churn/forward validation anchors | Derived from thesis state + daily OHLCV technical state plus existing risk/portfolio/open-position/sleeve state | n/a | `strategy-runner` reads `automation_trade_permission`, `automation_strategy_definition`, `thesis`, `price_bar`, risk config, portfolio settings, allocation policy, sleeves, and broker-position aggregates | wired - shadow-only `technical_timing@0.1.0` and `thesis_timing@0.1.0` run deterministic proof policy first; blocked preflights append `automation_proof` only, while passed/warning evaluations emit append-only `desired_strategy_position`, `automation_proof`, sleeve allocation updates, simulator `automation_execution_reconciliation`, simulated `position_fill`, sleeve attribution, and `automation_strategy_signal_observation`; no paper/live broker orders |

**Note:** Massive was the original price source (`src/ingest/massive.rs`); kept in
the repo for the news+sentiment endpoint but no longer the OHLCV source.

### Provider pacing and 429 handling

FMP and FRED calls go through process-wide provider limiters in
`src/ingest/rate_limit.rs`. Per-loop sleeps are not enough because price,
intraday chart, news, estimates, and screener adapters can run concurrently
against the same vendor. The limiter serializes requests per provider and, on
HTTP 429, respects `Retry-After` when present or applies an automatic backoff.

Default dev pacing:

| Provider | Min request spacing | 429 backoff | Max backoff |
|---|---:|---:|---:|
| FMP | 750ms | 60s | 15m |
| FRED | 2s | 5m | 60m |

Override with `FMP_MIN_REQUEST_INTERVAL_MS`, `FMP_RATE_LIMIT_BACKOFF_MS`,
`FMP_MAX_RATE_LIMIT_BACKOFF_MS`, `FRED_MIN_REQUEST_INTERVAL_MS`,
`FRED_RATE_LIMIT_BACKOFF_MS`, and `FRED_MAX_RATE_LIMIT_BACKOFF_MS`.

Expensive per-symbol loops do not scan the entire screener pool every pass.
They use `Store::priority_scan_symbols()`: active tickers first, then Tier 1/2
proposed discovery candidates, capped per provider loop. Dev defaults are
`FMP_PRICE_MAX_SYMBOLS_PER_PASS=125`, `FMP_ESTIMATES_MAX_SYMBOLS_PER_PASS=100`,
`FMP_OPINION_MAX_SYMBOLS_PER_PASS=75`, `FMP_LIVE_BAR_MAX_SYMBOLS_PER_PASS=25`,
`FMP_LIVE_BAR_INTERVAL_SECS=60`, `FMP_LIVE_BAR_ENTITLEMENT_BACKOFF_SECS=3600`,
`NEWS_MAX_SYMBOLS_PER_PASS=100`,
`EDGAR_MAX_SYMBOLS_PER_PASS=100`, and `XBRL_MAX_SYMBOLS_PER_PASS=100`.

## 2. Fundamentals

| Data | Why | Vendor | Tier / cost | Endpoint | Status |
|---|---|---|---|---|---|
| Company facts (XBRL) | Evaluator metrics (`SYMBOL.gross_margin_pct`, etc.), context maintainer fundamentals block | **SEC EDGAR** | free, public | `/api/xbrl/companyfacts/CIK<N>.json` | wired — `src/ingest/xbrl.rs`; tiered deep universe so active tickers and top candidates acquire facts without starving the freshness loop |
| CIK lookup | Resolving ticker → CIK before XBRL pull | SEC EDGAR | free | `/files/company_tickers.json` | wired — `src/ingest/sec.rs`; fetched dynamically with seeded fallback for known ADRs |
| Filing metadata | 8-K/10-Q/10-K submission watch between slower XBRL fact refreshes | SEC EDGAR | free | `/submissions/CIK<N>.json` | wired — `src/ingest/edgar.rs`; owns `sec_edgar_submissions` source tasks on a 30-minute freshness loop |
| Earnings calendar (upcoming + recent history) | Goalpost dates, horizon_at validation, beat/miss pattern, pending catalyst timing | **FMP** | Starter | `/stable/earnings?symbol=&limit=` returns upcoming + recent rows with `epsActual/epsEstimated/revenueActual/revenueEstimated/lastUpdated`; global `/stable/earnings-calendar?from=&to=` exists but large windows cap at 4,000 rows | wired — `src/ingest/fmp_profile_calendar.rs` + `src/ingest/fmp_profile_calendar_service.rs`, persisted to `earnings_calendar_event` and normalized as `earnings_calendar` evidence |
| Company profile | Cluster classification context, market cap, sector/industry, exchange/country, issuer flags | FMP | Starter | `/stable/profile?symbol=` returns profile rows for US and supported international tickers such as `2454.TW` | wired — `src/ingest/fmp_profile_calendar.rs` + `src/ingest/fmp_profile_calendar_service.rs`, persisted to `company_profile` |
| Insider transactions (Form 4/144) | Cross-check LLM context narrative claims | SEC EDGAR | free | `/cgi-bin/browse-edgar?action=getcompany&CIK=...&type=4` | not wired — tracked by #2 |
| 13F holdings | Lagged institutional positioning | SEC EDGAR | free | `/cgi-bin/browse-edgar?...&type=13F-HR` | not wired — tracked by #2 |

## 3. Analyst estimates + ratings

This is the SPEC §4 leading signal surface. The estimate snapshot/revision loop
is wired; the remaining high-value gap is FMP's global per-firm grade-change
feed, which should create discrete rating-catalyst evidence without needing to
poll every symbol.

| Data | Why | Vendor | Tier / cost | Endpoint | Status |
|---|---|---|---|---|---|
| Current consensus EPS/revenue (per fiscal period, forward 5+ yrs) | Baseline for revision detection; `numAnalystsEps` tells us coverage depth | **FMP** | Starter | `/stable/analyst-estimates?symbol=&period=annual` returns revenueLow/High/Avg, ebitda/ebit, netIncome, epsAvg/High/Low, numAnalystsRevenue, numAnalystsEps | wired — `src/ingest/fmp_estimates.rs` |
| **Estimate revision time-series** (the actual signal) | "Earlier than the crowd" detection — when consensus is being revised up/down before retail sees it | FMP via daily snapshot + diff against prior snapshot (we build this layer) | Starter | snapshot `/stable/analyst-estimates` daily → `estimate_snapshot` table → diff → `estimate_revision` events | wired — `src/ingest/fmp_estimates_service.rs` |
| Per-firm rating events (upgrade/downgrade with `gradingCompany`, `newGrade`, `previousGrade`, `priceWhenPosted`) | Discrete catalyst events for discovery + thesis flags. The actual "revisions" data Bloomberg/Refinitiv charge 5-figures/yr for, here for free. | **FMP** | Starter | `/stable/grades-latest-news?limit=` returns global event feed; filter to our universe client-side | wired — `src/ingest/fmp_opinion.rs` + `src/ingest/fmp_opinion_service.rs`, persisted to `analyst_rating_event` and normalized as `rating_change` evidence |
| Full recommendation history/backfill | Lower-resolution drift sanity check over time | FMP | Starter | `/stable/grades-historical?symbol=` returns monthly StrongBuy/Buy/Hold/Sell counts | partial — latest bucket wired via opinion mix below; full monthly backfill not wired |
| Analyst price target consensus | Consensus artifact: target high/low/median/consensus helps separate "outside consensus" from "already accepted" | FMP | Starter | `/stable/price-target-consensus?symbol=` | wired — `src/ingest/fmp_opinion.rs`, active tickers + Tier 1/2 proposed candidates, persisted to `analyst_price_target_snapshot` |
| Analyst recommendations / opinion mix | Latest buy/hold/sell mix for "what does sell-side already believe?" context | FMP | Starter | `/stable/grades-historical?symbol=&limit=1` | wired — `src/ingest/fmp_opinion.rs`, active tickers + Tier 1/2 proposed candidates, persisted to `analyst_recommendation_snapshot` |
| Analyst price target events | Recent firm-level target changes for catalysts and consensus drift narrative | FMP | Starter | `/stable/price-target-news?symbol=&limit=10` | wired — `src/ingest/fmp_opinion.rs`, active tickers + Tier 1/2 proposed candidates, persisted to `analyst_price_target_event` |

The normalized fact layer is `evidence_item`. News articles, estimate
revisions, analyst price-target events, SEC filing metadata, discovery
price-action signals, macro-regime changes, CBOE crowd-sentiment observations,
product/theme web research, and context shifts are written there as discrete
facts with source, source row pointer, strength, polarity, timestamp, and URL
where available.
`thesis_evidence` links the
current symbol thesis to the normalized facts seen during draft/reconciliation.
Raw vendor tables, `ingest_event` rows, `market_state` rows, and
`ticker_context` versions remain the audit source; `evidence_item` is the
operator/cognition-facing fact stream.

`evidence_item.observed_at` is when the underlying market/company fact happened.
`created_at` is when the fact first entered the normalized stream. `updated_at`
advances when an existing fact is refreshed, rescored, or merged with new
source details; the cognition sweep uses that as the evidence-delta clock.

## 4. News + per-article sentiment

Two-source strategy: ingest from both vendors, dedupe by URL/title, sentiment
populated from upstream when present else scored by our LLM classifier. New
sources (RSS, Twitter, future paid feeds) plug into the same pipeline.

| Data | Why | Vendor | Tier / cost | Endpoint | Status |
|---|---|---|---|---|---|
| Articles with per-ticker sentiment (pre-scored) | Discovery (catalyst signal), context maintainer narrative refresh, consensus mainstream_coverage component | **Massive** | Stocks Starter $29 (already paid) | `/v2/reference/news?ticker=&order=desc` returns `insights[].sentiment` ∈ {positive, neutral, negative} + `sentiment_reasoning` | wired — `src/ingest/massive_news.rs` |
| Additional articles (no upstream sentiment) | Wider coverage; FMP often surfaces articles Massive doesn't (Motley Fool, niche IR sites) | **FMP** | Starter | `/stable/news/stock?symbols=&limit=` returns title/text/publisher/url/publishedDate | wired — `src/ingest/fmp_news.rs` |
| **Universal sentiment classifier** | Scores any article without an upstream sentiment score; lets future news sources plug in without re-engineering | **z.ai** (Anthropic-compat) via `src/sentiment/` module + `prompts/score-sentiment.md` | per-token (~$0.001/article) | n/a — our own module | wired — `src/sentiment/` |
| Ticker-level intraday news sentiment aggregation (paid uplift) | When Massive's hourly news isn't fast enough | Marketaux Pro | $25–99/mo | `/v1/news/all?entities=&sentiment_gte=&sentiment_lte=` | not wired — evaluate only if free+FMP+Massive set isn't enough |

## 4.5 Product / Theme Web Research

This fills the gap between symbol-tagged vendor news and the operator's actual
questions, e.g. "what public material exists on AMD MI325X/MI355X/MI400
deployments, benchmarks, and adoption?"

| Data | Why | Vendor | Tier / cost | Endpoint | Status |
|---|---|---|---|---|---|
| Product/theme web articles | Public evidence for roadmaps, benchmarks, deployment reports, customer adoption, and competitive claims that may not be tagged to a ticker by FMP/Massive | **GDELT Doc 2.0** + Bing News RSS fallback | free, no key | GDELT `/api/v2/doc/doc?query=&mode=ArtList&format=json`; Bing `/news/search?q=&format=rss` | wired — `py/src/stocks/research.py`, persisted to `research_evidence`, source health `web_research` |
| Semantic web retrieval uplift | Better recall for niche engineering blogs, benchmark posts, product docs, and other public pages that RSS/news-only search misses | **Firecrawl** via hosted API or optional local compose sidecar | local self-host or hosted credits | Firecrawl `/v2/search` with `sources=["web"]`, optional self-host at `http://firecrawl-api:3002` | wired behind `RESEARCH_PROVIDER=...,firecrawl`; local sidecar in `deploy/local/docker-compose.firecrawl.yml` |

Provider search results are relevance-gated before they become symbol-level
research evidence. A result must match the requested ticker, a company alias
such as `MediaTek` for `2454.TW`, or a specific configured product term such as
`MI400`, `GB200`, `ROCm`, `CoWoS`, or `PowerEdge`. Generic theme terms alone do
not promote a row, and unrelated ticker collisions are counted in
`research_retrieval_run.source_ref` rather than inserted as evidence.

## 5. Crowd sentiment

Free-first strategy: stack as many free signals as possible, add one paid feed
only if it carries signal beyond the free set.

| Data | What signal | Source | Cost | URL | Cadence | Status |
|---|---|---|---|---|---|---|
| AAII bulls/bears/neutral | Retail individual-investor sentiment (the "late crowd" we're trying to lead per SPEC §0) | AAII | free | `https://www.aaii.com/files/surveys/sentiment.xls` | weekly Thu AM | not wired (#20) |
| AAII asset allocation | What retail is actually holding (stocks vs bonds vs cash) | AAII | free | `https://www.aaii.com/files/surveys/allocation.xls` | monthly | not wired |
| NAAIM Exposure Index | Active manager net long exposure | NAAIM via Nasdaq Data Link | free (free key required) | `https://data.nasdaq.com/data/NAAIM/NAAIM_EXPOSURE_INDEX` | weekly Thu | not wired |
| CBOE equity put/call | Options-derived crowd hedging | CBOE | free | `https://cdn.cboe.com/resources/options/volume_and_call_put_ratios/equitypc.csv` | daily | wired — `src/ingest/cboe.rs` + `src/ingest/crowd_sentiment_service.rs`; writes `crowd_sentiment`, `evidence_item`, and source-task health |
| CBOE index put/call | Macro hedging vs single-name | CBOE | free | `https://cdn.cboe.com/resources/options/volume_and_call_put_ratios/indexpcarchive.csv` | daily | not wired |
| CNN Fear & Greed (all 7 components) | Composite + components (price strength, momentum, market volatility, junk bond demand, etc.) | CNN (undocumented JSON) | free (set browser UA to avoid blocks) | `https://production.dataviz.cnn.io/index/fearandgreed/graphdata/{YYYY-MM-DD}` | daily | not wired |
| VIX + term structure | Volatility regime / contango-backwardation | CBOE | free | `https://cdn.cboe.com/api/global/us_indices/daily_prices/VIX_History.csv` (plus `VIX9D_`, `VIX3M_`, `VIX6M_History.csv`) | daily | partial — VIX close wired via CBOE and FRED `VIXCLS`; VIX9D/VIX3M/VIX6M term structure not wired |
| StockTwits message volume per ticker | Retail social-chatter spike detection (best-effort — no new API keys being issued) | StockTwits public API | free, ~200 req/hr no auth | `https://api.stocktwits.com/api/2/streams/symbol/{TICKER}.json` | continuous | not wired |
| Reddit / r/wallstreetbets ticker mentions | Retail meme/momentum signal | Reddit OAuth API (free PRAW) | free, 100 req/min | n/a (PRAW scrape of daily discussion threads) | continuous | not wired |
| ICI mutual fund / ETF flows | Slow-moving retail+institutional capital flows | ICI | free | `https://www.ici.org/research/stats/flows` + `/etf_flows` | weekly XLS, ~3-day lag | not wired |
| Marketaux Pro (paid uplift) | Best signal-to-noise among paid sentiment aggregators in our price range; ticker-resolved + intraday | Marketaux | $25 Standard or $99 Pro | `/v1/news/all` with entity + sentiment filters | continuous | not wired — only consider after free + FMP + Massive set isn't enough |

**Won't consider:** RavenPack (starts at five figures/yr), Sentdex (effectively abandoned).

## 6. Macro / market state

| Data | Why | Vendor | Tier / cost | Endpoint | Status |
|---|---|---|---|---|---|
| Selected FRED series | Regime classifier (yield curve, risk spreads, volatility) | FRED | free (free key required) | `https://api.stlouisfed.org/fred/series/observations?series_id=` | wired — `src/ingest/fred.rs`; currently `DGS10`, `DGS3MO`, `BAMLH0A0HYM2`, `VIXCLS` |
| HY/IG credit spreads | Risk-on/risk-off signal | FRED (same key) | free | series ids `BAMLH0A0HYM2` (HY OAS), `BAMLH0A1HYBB` (BB), `BAMLH0A2HYB` (B), `BAMLC0A0CM` (IG OAS) | partial — HY OAS is wired; BB/B/IG buckets are not wired |
| VIX (historical) | Regime input | FRED `VIXCLS`, `VXVCLS` (3M) | free | (see above for FRED endpoint) | partial — `VIXCLS` wired via FRED and CBOE VIX close; `VXVCLS`/3M volatility is not wired |

## 7. Position / portfolio state

For v0 the operator sets the account size manually (#26 — already shipped).
Once #25 (IBKR bridge) is live against the operator's local TWS/Gateway or the
optional managed IB Gateway compose profile, broker position rows replace manual
portfolio entry for risk sizing.

| Data | Why | Vendor | Tier / cost | Status |
|---|---|---|---|---|
| Account size + high-water mark | Risk overlay portfolio frame (cash floor, single-name cap, drawdown brake) | Operator-set via `PUT /api/portfolio` | n/a | wired — `db/migrations/0012_portfolio_settings.sql` |
| Open positions + fills | Real-time portfolio state for risk overlay; replaces manual entry | **IBKR** via `ib_insync` against TWS/IB Gateway | IBKR Pro $10/mo activity min (waived with active trading); paper trading free | partial — `py/src/stocks/ibkr_sync.py` connects read-only, upserts broker-tagged `position` rows, appends deduped broker fills, updates `portfolio_settings` from NetLiquidation, and publishes `position.updated`; use an existing local TWS/Gateway with `make sync-ibkr` one-shot or `make ibkr-up` loop, or run the managed compose gateway with `make ibkr-stack-up` (paper API defaults to `ibkr-gateway:4004`, host port `4002`; live host port `4001`). IBKR credentials must be stored in Infisical dev (`TWS_USERID`/`TWS_PASSWORD`, or paper variants when `TRADING_MODE=both`). Order placement remains explicitly not wired |
| Realized PnL | Drawdown anchor in `risk::derive_portfolio` | Currently SUM of `position.realized_pnl WHERE closed_at IS NOT NULL`; broker-imported account equity now updates the portfolio frame | n/a | wired — `Store::realized_pnl_total` plus IBKR NetLiquidation high-water update |

## 8. Auth / LLM

Not market data but worth listing because the smoketest needs it.

| Data | Vendor | Tier / cost | Env var | Status |
|---|---|---|---|---|
| LLM completions (cognition + universal sentiment scoring) | **z.ai** (Anthropic-compat, glm-5.1 / glm-4.6) | per-token, ~cents per thesis draft, sub-cent per news article scored | `ANTHROPIC_API_KEY` (auto-detected as z.ai by base URL) | wired |
| (alt) Real Anthropic | Anthropic | per-token | same env var, different value | supported, not used |

---

## Currently configured keys (names only)

`ANTHROPIC_API_KEY`, `FRED_API_KEY`, `MASSIVE_API_KEY`, `FMP_API_KEY` — all in
Infisical `dev` env. Add new keys via `infisical secrets set NAME` — never to `.env`.

To check which keys are configured without revealing values:

```bash
grep -E "_API_KEY|_TOKEN" src/platform/config.rs py/src/stocks/config.py
```

That lists the env-var names the code reads. Match against what's in Infisical;
absent names = not yet configured.

---

## Decision log

**2026-05-31 — vendor consolidation analysis (initial)**
- Considered replacing Massive with FMP only: rejected. FMP's earnings endpoint
  is snapshot only, no native revision timeline.
- Considered replacing Massive with Finnhub: viable but Finnhub's estimate
  add-on alone is $75/mo. Total cost worse than alternatives.

**2026-05-31 — FMP probe with live key**
- FMP Starter unlocks per-symbol queries across our full universe (NVDA, MU,
  AMD, AMAT, TSM, ANET, VRT, CDNS all returned 200 on `historical-price-eod/full`
  and `analyst-estimates`).
- **`grades-latest-news` is the breakthrough** — a global event feed with
  `newGrade` / `previousGrade` / `gradingCompany` / `priceWhenPosted` per event.
  This is per-firm rating *revisions* (the rich version Bloomberg/Refinitiv
  sells for 5-figures/yr), free on FMP Starter, just needs client-side filter
  to our universe. FMP Starter caps this feed's `limit` parameter at 100; the
  ingest code clamps operator overrides to stay inside that plan limit.
- FMP news lacks per-article sentiment (Massive's has it via `insights[]`).

**2026-05-31 — chosen architecture**
- FMP Starter primary for price, estimates, grades, earnings calendar
- Massive kept for news+sentiment specifically
- Build a generic LLM sentiment classifier (`src/sentiment/`) so any future
  news source (FMP news, RSS feeds, Twitter, etc.) gets scored through the
  same path with full audit trail via `llm_invocation`
- For estimates: build snapshot+diff layer ourselves (daily snapshot of
  `analyst-estimates` → `estimate_snapshot` table → diff against prior →
  emit `estimate_revision` events). Cheaper than Finnhub's pre-computed
  revision feed and we own the audit trail.

**2026-05-31 — Massive Developer 25ms websocket considered, deferred**
- Massive Stocks Developer ($99/mo) includes a 25ms-latency websocket feed.
  Considered upgrading from Starter ($29) → Developer ($99) = +$70/mo net
  after dropping the FMP price overlap.
- Rejected for now. SPEC §0 says we operate on day-to-week cadence with
  forward-only end-of-day validation. None of our current signals (discovery
  volume_anomaly / base_breakout, evaluator XBRL/close, consensus drift,
  risk overlay) require sub-second data. The analyst-revision flow operates
  on daily snapshot drift.
- Revisit when any of these become real priorities:
  (a) intraday alerting goal ("alert within 1s of a 3x volume spike"),
  (b) LEAPS execution via #25 IBKR (live bid/ask vs paying the spread),
  (c) tape-reading signals (unusual options activity, dark pool prints).

**2026-06-03 — data-source audit after Brain/workflow expansion**
- GitHub issue #2 was stale: it still said only EDGAR and FRED were ingesting.
  The actual stack now includes FMP daily/intraday price bars, screener
  discovery, estimate snapshots/revisions, analyst opinion, company profile,
  earnings calendar, FMP news, Massive news+sentiment, SEC submissions, XBRL
  company facts, TWSE Taiwan fallback, CBOE equity put/call + VIX close,
  selected FRED series, and GDELT/Bing product research.
- Remaining high-leverage gaps are narrower and should be tracked explicitly:
  options chains,
  insider/Form 4 and 13F positioning, broader macro/breadth inputs, direct
  commodity/weather/inventory data, and transcript/video ingestion (#245).
