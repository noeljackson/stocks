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

---

## 1. Price + market data

| Data | Why | Vendor | Tier / cost | Endpoint | Status |
|---|---|---|---|---|---|
| Daily OHLCV bars | Discovery signals (volume_anomaly, base_breakout), evaluator (`SYMBOL.close`), consensus price_extension component | **Massive** | Stocks Starter $29/mo | `/v2/aggs/ticker/{sym}/range/1/day/{from}/{to}?adjusted=true` | wired — `src/ingest/massive.rs` (730d backfill) |
| Intraday bars (1m/5m) | Not needed yet | Massive | Same plan | `/v2/aggs/ticker/.../range/1/minute/...` | not wired |
| Options chains | LEAPS thesis instrument selection (#5 epic) | Massive Options Starter $29/mo extra | `/v3/snapshot/options/{underlying}` | not wired |
| Corporate actions (splits/dividends) | Implicit — Massive serves adjusted close | Massive (built into adjusted prices) | included | implicit |
| Realtime quotes / websocket | Not needed at v0 (forward-only validation per SPEC §9 uses end-of-day) | Massive websocket | Stocks Developer $99/mo | `/v3/...` | not wired |

## 2. Fundamentals

| Data | Why | Vendor | Tier / cost | Endpoint | Status |
|---|---|---|---|---|---|
| Company facts (XBRL) | Evaluator metrics (`SYMBOL.gross_margin_pct`, etc.), context maintainer fundamentals block | **SEC EDGAR** | free, public | `/api/xbrl/companyfacts/CIK<N>.json` | wired — `src/ingest/xbrl.rs` (1.9k facts/ticker) |
| CIK lookup | Resolving ticker → CIK before XBRL pull | SEC EDGAR | free | `/files/company_tickers.json` | wired — `src/ingest/sec.rs` |
| Earnings calendar | Goalpost dates, horizon_at validation | **Finnhub** (cheap), Massive Benzinga add-on (expensive) | Finnhub free tier OK | `/calendar/earnings?from=&to=` | not wired |
| Insider transactions (Form 4/144) | Cross-check LLM context narrative claims | SEC EDGAR | free | `/cgi-bin/browse-edgar?action=getcompany&CIK=...&type=4` | not wired |
| 13F holdings | Lagged institutional positioning | SEC EDGAR | free | `/cgi-bin/browse-edgar?...&type=13F-HR` | not wired |

## 3. Analyst estimates + ratings

This is the SPEC §4 "#1 leading signal for the edge" gap (#18).

| Data | Why | Vendor | Tier / cost | Endpoint | Status |
|---|---|---|---|---|---|
| Current consensus EPS/revenue | Baseline for revision detection | FMP, Finnhub | FMP Premium ~$50, Finnhub Estimates add-on ~$50 | FMP: `/stable/financial-estimates`. Finnhub: `/stock/eps-estimate` | not wired (#18) |
| **Estimate revision time-series** (the actual signal) | "Earlier than the crowd" detection — when consensus is being revised up/down before retail sees it | **Finnhub** has this natively; FMP does not (snapshot only) | Finnhub Estimate add-on ~$50/mo | `/stock/revision?symbol=` (counts of up/down revisions over 7/30/60/90d windows per fiscal period) | not wired (#18) |
| Per-firm rating changes (upgrades/downgrades) | Discrete catalyst events for discovery + thesis flags | Finnhub | Free tier exposes US stocks (capped); Premium removes cap | `/stock/upgrade-downgrade?symbol=` returns `{gradeTime, fromGrade, toGrade, company, action}` | not wired (#18) |
| Aggregate buy/hold/sell counts | Lower-resolution sanity check | Finnhub | free | `/stock/recommendation` | not wired |

**Vendor verdict:** Finnhub's revision time-series is the FMP-killer. FMP's estimates
endpoint returns a snapshot only — their own docs say "historical drift must be
captured locally over time." If you build the snapshot-and-diff layer yourself,
either vendor works; if you'd rather just query a revision feed, use Finnhub.

## 4. News + per-article sentiment

| Data | Why | Vendor | Tier / cost | Endpoint | Status |
|---|---|---|---|---|---|
| Articles with per-ticker sentiment | Discovery (catalyst signal), context maintainer narrative refresh, consensus mainstream_coverage component | **Massive** (already paid for) | included in Stocks Starter $29 | `/v2/reference/news?ticker=&order=desc` returns `insights[].sentiment` ∈ {positive, neutral, negative} + `sentiment_reasoning` | not wired (#19) |
| Ticker-level intraday news sentiment aggregation (paid uplift) | When Massive's hourly news isn't fast enough | **Marketaux Pro** | $25–99/mo | `/v1/news/all?entities=&sentiment_gte=&sentiment_lte=` | not wired — evaluate only after Massive's wired |

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
| Marketaux Pro (paid uplift) | Best signal-to-noise among paid sentiment aggregators in our price range; ticker-resolved + intraday | Marketaux | $25 Standard or $99 Pro | `/v1/news/all` with entity + sentiment filters | continuous | not wired — only consider after free stack is in |

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
| LLM completions | **z.ai** (Anthropic-compat, glm-5.1 / glm-4.6) | per-token, ~cents per thesis draft | `ANTHROPIC_API_KEY` (auto-detected as z.ai by base URL) | wired |
| (alt) Real Anthropic | Anthropic | per-token | same env var, different value | supported, not used |

---

## Currently configured keys (names only)

`ANTHROPIC_API_KEY`, `FRED_API_KEY`, `MASSIVE_API_KEY` — all in Infisical
`dev` env. Add new keys via `infisical secrets set NAME` — never to `.env`.

To check which keys are configured without revealing values:

```bash
grep -E "_API_KEY|_TOKEN" src/platform/config.rs py/src/stocks/config.py
```

That lists the env-var names the code reads. Match against what's in Infisical;
absent names = not yet configured.

---

## Decision log

**2026-05-31 — vendor consolidation analysis**
- Considered replacing Massive with FMP: rejected. FMP only offers current
  consensus snapshot, no revision timeline (the reason FMP was on the table).
- Considered replacing Massive with Finnhub: viable. Finnhub `/stock/revision`
  is a true revision time-series (the FMP-killer feature). One Finnhub
  Premium All-In-One (~$50/mo) covers OHLCV + news+sentiment + estimates +
  rating changes, replacing Massive Starter ($29).
- Gaps where Massive wins: options chains (LEAPS work), websocket realtime,
  tick-level data quality reputation. None of these matter for our daily-batch
  thesis workflow today.
- **Open question for operator:** stay with Massive ($29, defer estimates) or
  swap to Finnhub Premium ($50, includes estimates). Doc updated when decided.
