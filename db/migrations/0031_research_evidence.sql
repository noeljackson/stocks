-- 0031_research_evidence.sql
--
-- Product/theme evidence that is not reliably symbol-tagged by market-data
-- news feeds. This supports retrieval for claims such as accelerator launches,
-- benchmarks, deployment reports, customer adoption, and competitive notes.

CREATE TABLE IF NOT EXISTS research_evidence (
    id              bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    symbol          text NOT NULL REFERENCES ticker(symbol),
    query           text NOT NULL,
    url             text NOT NULL,
    title           text NOT NULL,
    publisher       text,
    published_at    timestamptz,
    retrieved_at    timestamptz NOT NULL DEFAULT now(),
    provider        text NOT NULL,
    source_type     text NOT NULL DEFAULT 'web_search'
                    CHECK (source_type IN ('web_search', 'news_search', 'rss', 'manual')),
    credibility     text NOT NULL DEFAULT 'unknown'
                    CHECK (credibility IN ('primary', 'credible_media', 'industry', 'unknown')),
    summary         text,
    tags            text[] NOT NULL DEFAULT '{}',
    source_ref      jsonb NOT NULL DEFAULT '{}'::jsonb,
    content_hash    text NOT NULL,
    UNIQUE (symbol, url)
);

CREATE INDEX IF NOT EXISTS ix_research_evidence_symbol_at
    ON research_evidence(symbol, retrieved_at DESC);

CREATE INDEX IF NOT EXISTS ix_research_evidence_published_at
    ON research_evidence(symbol, published_at DESC);

CREATE INDEX IF NOT EXISTS ix_research_evidence_tags
    ON research_evidence USING gin(tags);

CREATE TABLE IF NOT EXISTS research_retrieval_run (
    id            bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    symbol        text NOT NULL REFERENCES ticker(symbol),
    provider      text NOT NULL,
    query         text NOT NULL,
    status        text NOT NULL CHECK (status IN ('ok', 'no_results', 'failed')),
    result_count  int NOT NULL DEFAULT 0,
    last_error    text,
    source_ref    jsonb NOT NULL DEFAULT '{}'::jsonb,
    started_at    timestamptz NOT NULL DEFAULT now(),
    finished_at   timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS ix_research_retrieval_run_symbol_at
    ON research_retrieval_run(symbol, finished_at DESC);
