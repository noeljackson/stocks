-- 0018 — drop news_article.symbol → ticker.symbol FK.
--
-- Why: news ingest is broad-radar (Tier 3 in the pool model — see docs/LIFECYCLE.md).
-- Pool members live in `discovery_pool`, not `ticker`. The FK was inherited from
-- the seed-only era and now blocks ~110 of 116 pool symbols from getting news
-- captured. Without news, the `news_sentiment_shift` signal is dark for the pool.
--
-- `news_article` is append-only event data keyed by symbol-as-string. We don't
-- need referential integrity here — `symbol` is the lookup key, not a join.

ALTER TABLE news_article DROP CONSTRAINT IF EXISTS news_article_symbol_fkey;
