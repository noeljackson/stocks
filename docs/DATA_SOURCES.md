# Data sources

The system pulls data from a small set of vendors. This doc is the **single
source of truth** for what each piece of data is, why we need it, which vendor
it comes from, what tier/cost that requires, and the current wiring status.

When you add a new data source: edit this doc *first*. When you find a gap:
file an issue and link it from the relevant row's "status" column.

**Status legend:**
- `wired` — running in production, has tests, has audit trail
- `key-only` — API key in Infisical but no adapter code yet
- `not wired` — vendor identified, no key, no code
- `gap` — known limitation, no fix planned

**Current vendor stack (as of 2026-05-31):**
- **FMP Starter** ($22/mo) — primary: price/OHLCV, analyst estimates, per-firm rating events, earnings calendar, company news (no sentiment), company profile
- **Massive Stocks Starter** ($29/mo) — kept for: news with per-article sentiment (where FMP has no equivalent)
- **SEC EDGAR** (free) — XBRL company facts, insider transactions, 13F holdings
- **FRED** (free) — macro economic series, credit spreads, VIX history
- **z.ai** (~cents/call) — LLM provider for cognition layer, plus the universal
  sentiment classifier (so any news source we add later — RSS, Twitter, future
  paid feeds — gets sentiment-scored without depending on a vendor's own scorer)

---

## 1. Price + market data

| Data | Why | Vendor | Tier / cost | Endpoint | Status |
|---|---|---|---|---|---|
| Daily OHLCV bars | Discovery signals (volume_anomaly, base_breakout), evaluator (`SYMBOL.close`), consensus price_extension component | **FMP** (primary) | Starter $22/mo | `/stable/historical-price-eod/full?symbol=&from=&to=` returns OHLCV + change + vwap. 5+ yrs adjusted history. | wired — `src/ingest/fmp_price.rs` |
| Intraday bars (1m/5m/15m/30m/1h/4h) | TradingView-style chart intervals; 3m/2h are aggregated from native bars | FMP | Same plan | `/stable/historical-chart/{interval}` | wired — `src/ingest/fmp_intraday.rs` |
| Company screener / discovery pool | Broad radar for tech infrastructure plus adjacent bottlenecks: semis, optics/networking, software infra, power/grid/cooling, copper/materials, data-center REITs | FMP | Starter | `/stable/company-screener` by sector/industry slice | wired — `src/ingest/fmp_screener.rs`; creates `pool_inspection` candidates for unreviewed pool names |
| Options chains | LEAPS thesis instrument selection (#5 epic) | Massive Options Starter $29/mo extra (or FMP has thin coverage — verify before swap) | `/v3/snapshot/options/{underlying}` (Massive) | not wired |
| Corporate actions (splits/dividends) | Implicit — FMP serves adjusted close | FMP (built into adjusted prices) | included | implicit |
| Realtime quotes / websocket | Not needed at v0 (forward-only validation per SPEC §9 uses end-of-day) | FMP websocket limited; Massive better | varies | not wired |

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

## 2. Fundamentals

| Data | Why | Vendor | Tier / cost | Endpoint | Status |
|---|---|---|---|---|---|
| Company facts (XBRL) | Evaluator metrics (`SYMBOL.gross_margin_pct`, etc.), context maintainer fundamentals block | **SEC EDGAR** | free, public | `/api/xbrl/companyfacts/CIK<N>.json` | wired — `src/ingest/xbrl.rs` (1.9k facts/ticker) |
| CIK lookup | Resolving ticker → CIK before XBRL pull | SEC EDGAR | free | `/files/company_tickers.json` | wired — `src/ingest/sec.rs` |
| Earnings calendar (upcoming + history) | Goalpost dates, horizon_at validation, beat/miss pattern | **FMP** | Starter | `/stable/earnings?symbol=` returns `epsActual/epsEstimated/revenueActual/revenueEstimated/lastUpdated`. `/stable/earnings-calendar?from=&to=` for global upcoming | not wired |
| Company profile | Cluster classification context, market cap | FMP | Starter | `/stable/profile?symbol=` | not wired |
| Insider transactions (Form 4/144) | Cross-check LLM context narrative claims | SEC EDGAR | free | `/cgi-bin/browse-edgar?action=getcompany&CIK=...&type=4` | not wired |
| 13F holdings | Lagged institutional positioning | SEC EDGAR | free | `/cgi-bin/browse-edgar?...&type=13F-HR` | not wired |

## 3. Analyst estimates + ratings

This is the SPEC §4 "#1 leading signal for the edge" gap (#18).

| Data | Why | Vendor | Tier / cost | Endpoint | Status |
|---|---|---|---|---|---|
| Current consensus EPS/revenue (per fiscal period, forward 5+ yrs) | Baseline for revision detection; `numAnalystsEps` tells us coverage depth | **FMP** | Starter | `/stable/analyst-estimates?symbol=&period=annual` returns revenueLow/High/Avg, ebitda/ebit, netIncome, epsAvg/High/Low, numAnalystsRevenue, numAnalystsEps | wired — `src/ingest/fmp_estimates.rs` |
| **Estimate revision time-series** (the actual signal) | "Earlier than the crowd" detection — when consensus is being revised up/down before retail sees it | FMP via daily snapshot + diff against prior snapshot (we build this layer) | Starter | snapshot `/stable/analyst-estimates` daily → `estimate_snapshot` table → diff → `estimate_revision` events | wired — `src/ingest/fmp_estimates_service.rs` |
| Per-firm rating events (upgrade/downgrade with `gradingCompany`, `newGrade`, `previousGrade`, `priceWhenPosted`) | Discrete catalyst events for discovery + thesis flags. The actual "revisions" data Bloomberg/Refinitiv charge 5-figures/yr for, here for free. | **FMP** | Starter | `/stable/grades-latest-news?limit=` returns global event feed; filter to our universe client-side | not wired (#18) |
| Aggregate buy/hold/sell counts (monthly buckets per symbol) | Lower-resolution drift sanity check | FMP | Starter | `/stable/grades-historical?symbol=` returns monthly StrongBuy/Buy/Hold/Sell counts | not wired |
| Analyst price target consensus | Consensus artifact: target high/low/median/consensus helps separate "outside consensus" from "already accepted" | FMP | Starter — verify live key | `/stable/price-target-consensus?symbol=` | not wired (#116) |
| Analyst recommendations / opinion mix | Buy/hold/sell mix for "what does sell-side already believe?" context | FMP | Starter — verify live key | recommendation endpoints / stock recommendations | not wired (#116) |

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

## 5. Crowd sentiment

Free-first strategy: stack as many free signals as possible, add one paid feed
only if it carries signal beyond the free set.

| Data | What signal | Source | Cost | URL | Cadence | Status |
|---|---|---|---|---|---|---|
| AAII bulls/bears/neutral | Retail individual-investor sentiment (the "late crowd" we're trying to lead per SPEC §0) | AAII | free | `https://www.aaii.com/files/surveys/sentiment.xls` | weekly Thu AM | not wired (#20) |
| AAII asset allocation | What retail is actually holding (stocks vs bonds vs cash) | AAII | free | `https://www.aaii.com/files/surveys/allocation.xls` | monthly | not wired |
| NAAIM Exposure Index | Active manager net long exposure | NAAIM via Nasdaq Data Link | free (free key required) | `https://data.nasdaq.com/data/NAAIM/NAAIM_EXPOSURE_INDEX` | weekly Thu | not wired |
| CBOE equity put/call | Options-derived crowd hedging | CBOE | free | `https://cdn.cboe.com/resources/options/volume_and_call_put_ratios/equitypc.csv` | daily | not wired (#20) |
| CBOE index put/call | Macro hedging vs single-name | CBOE | free | `https://cdn.cboe.com/resources/options/volume_and_call_put_ratios/indexpcarchive.csv` | daily | not wired |
| CNN Fear & Greed (all 7 components) | Composite + components (price strength, momentum, market volatility, junk bond demand, etc.) | CNN (undocumented JSON) | free (set browser UA to avoid blocks) | `https://production.dataviz.cnn.io/index/fearandgreed/graphdata/{YYYY-MM-DD}` | daily | not wired |
| VIX + term structure | Volatility regime / contango-backwardation | CBOE | free | `https://cdn.cboe.com/api/global/us_indices/daily_prices/VIX_History.csv` (plus `VIX9D_`, `VIX3M_`, `VIX6M_History.csv`) | daily | not wired |
| StockTwits message volume per ticker | Retail social-chatter spike detection (best-effort — no new API keys being issued) | StockTwits public API | free, ~200 req/hr no auth | `https://api.stocktwits.com/api/2/streams/symbol/{TICKER}.json` | continuous | not wired |
| Reddit / r/wallstreetbets ticker mentions | Retail meme/momentum signal | Reddit OAuth API (free PRAW) | free, 100 req/min | n/a (PRAW scrape of daily discussion threads) | continuous | not wired |
| ICI mutual fund / ETF flows | Slow-moving retail+institutional capital flows | ICI | free | `https://www.ici.org/research/stats/flows` + `/etf_flows` | weekly XLS, ~3-day lag | not wired |
| Marketaux Pro (paid uplift) | Best signal-to-noise among paid sentiment aggregators in our price range; ticker-resolved + intraday | Marketaux | $25 Standard or $99 Pro | `/v1/news/all` with entity + sentiment filters | continuous | not wired — only consider after free + FMP + Massive set isn't enough |

**Won't consider:** RavenPack (starts at five figures/yr), Sentdex (effectively abandoned).

## 6. Macro / market state

| Data | Why | Vendor | Tier / cost | Endpoint | Status |
|---|---|---|---|---|---|
| Selected FRED series | Regime classifier (yield curve, unemployment, etc.) | FRED | free (free key required) | `https://api.stlouisfed.org/fred/series/observations?series_id=` | wired — `src/ingest/` |
| HY/IG credit spreads | Risk-on/risk-off signal | FRED (same key) | free | series ids `BAMLH0A0HYM2` (HY OAS), `BAMLH0A1HYBB` (BB), `BAMLH0A2HYB` (B), `BAMLC0A0CM` (IG OAS) | not wired |
| VIX (historical) | Regime input | FRED `VIXCLS`, `VXVCLS` (3M) | free | (see above for FRED endpoint) | not wired |

## 7. Position / portfolio state

For v0 the operator sets the account size manually (#26 — already shipped).
Once #25 (IBKR bridge) lands, real position rows replace the manual entry.

| Data | Why | Vendor | Tier / cost | Status |
|---|---|---|---|---|
| Account size + high-water mark | Risk overlay portfolio frame (cash floor, single-name cap, drawdown brake) | Operator-set via `PUT /api/portfolio` | n/a | wired — `db/migrations/0012_portfolio_settings.sql` |
| Open positions + fills | Real-time portfolio state for risk overlay; replaces manual entry | **IBKR** via `ib_insync` against the gateway | IBKR Pro $10/mo activity min (waived with active trading); paper trading free | not wired (#25) |
| Realized PnL | Drawdown anchor in `risk::derive_portfolio` | Currently SUM of `position.realized_pnl WHERE closed_at IS NOT NULL`; IBKR will populate | n/a | wired — `Store::realized_pnl_total` |

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
  to our universe.
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
