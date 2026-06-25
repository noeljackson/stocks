import datetime as dt
import json

import httpx
import pytest

from stocks.research import (
    FirecrawlProvider,
    SearchResult,
    _evidence_strength,
    _insert_result,
    _recent_run_exists,
    build_queries,
    load_research_evidence,
    research_relevance,
)


class FakeResearchPool:
    def __init__(self) -> None:
        self.fetchrow_calls: list[tuple[str, tuple[object, ...]]] = []
        self.execute_calls: list[tuple[str, tuple[object, ...]]] = []

    async def fetchrow(self, sql: str, *args: object) -> dict[str, object]:
        self.fetchrow_calls.append((sql, args))
        assert "RETURNING id" in sql
        return {"id": 42, "inserted": False}

    async def execute(self, sql: str, *args: object) -> str:
        self.execute_calls.append((sql, args))
        assert "INSERT INTO evidence_item" in sql
        return "INSERT 0 1"


class FakeLoadResearchPool:
    def __init__(self) -> None:
        self.fetch_args: tuple[object, ...] | None = None

    async def fetchrow(self, _sql: str, symbol: str) -> dict[str, object]:
        assert symbol == "ADI"
        return {
            "company_name": "Analog Devices, Inc.",
            "industry": "Semiconductors",
            "sector": "Technology",
            "cluster_id": "semis",
        }

    async def fetch(self, _sql: str, *args: object) -> list[dict[str, object]]:
        self.fetch_args = args
        now = dt.datetime(2026, 6, 1, 12, tzinfo=dt.UTC)
        return [
            {
                "id": 1,
                "query": "ADI semiconductor demand",
                "url": "https://example.com/crus",
                "title": "Cirrus Logic (CRUS) Q4 2026 Earnings Transcript",
                "publisher": "Example",
                "published_at": now,
                "retrieved_at": now,
                "provider": "bing_news_rss",
                "source_type": "news_search",
                "credibility": "unknown",
                "summary": None,
                "tags": ["ADI"],
            },
            {
                "id": 2,
                "query": "ADI semiconductor demand",
                "url": "https://example.com/adi",
                "title": "Analog Devices (ADI) margin update",
                "publisher": "Example",
                "published_at": now,
                "retrieved_at": now,
                "provider": "bing_news_rss",
                "source_type": "news_search",
                "credibility": "unknown",
                "summary": None,
                "tags": ["ADI"],
            },
        ]


class FakeRecentRunPool:
    def __init__(self, providers: list[str]) -> None:
        self.providers = providers
        self.fetch_calls: list[tuple[str, tuple[object, ...]]] = []

    async def fetch(self, sql: str, *args: object) -> list[dict[str, str]]:
        self.fetch_calls.append((sql, args))
        return [{"provider": provider} for provider in self.providers]


def _result(title: str, url: str = "https://example.com/story") -> SearchResult:
    return SearchResult(
        title=title,
        url=url,
        publisher="Example",
        published_at=None,
        summary=None,
        source_type="news_search",
        credibility="unknown",
        source_ref={},
    )


def test_build_queries_uses_static_product_terms_for_amd() -> None:
    queries = build_queries(
        "AMD",
        {"company_name": "Advanced Micro Devices, Inc.", "industry": "Semiconductors"},
        None,
        max_queries=4,
    )

    joined = " ".join(queries)
    assert "MI325X" in joined
    assert "MI355X" in joined
    assert "Advanced Micro Devices" in queries[0]


def test_build_queries_extracts_product_tokens_from_context() -> None:
    queries = build_queries(
        "XYZ",
        {"company_name": "Example Compute", "industry": "Software - Infrastructure"},
        {
            "narrative": {
                "themes": ["ROCm deployments and GB200 supply are the watch items"],
            },
        },
        max_queries=4,
    )

    joined = " ".join(queries)
    assert "ROCm" in joined
    assert "GB200" in joined


def test_build_queries_prioritizes_llm_research_questions() -> None:
    queries = build_queries(
        "AVGO",
        {"company_name": "Broadcom Inc.", "industry": "Semiconductors"},
        None,
        max_queries=2,
        extra_queries=[
            "custom silicon hyperscaler socket wins 2026",
            "AVGO Tomahawk switch deployment momentum",
        ],
    )

    assert queries == [
        "Broadcom custom silicon hyperscaler socket wins 2026",
        "AVGO Tomahawk switch deployment momentum",
    ]


@pytest.mark.asyncio
async def test_recent_run_does_not_skip_missing_provider() -> None:
    pool = FakeRecentRunPool(["bing_news_rss"])

    recent = await _recent_run_exists(
        pool,
        "AVGO",
        max_age_hours=24,
        providers=["bing_news_rss", "firecrawl"],
    )

    assert recent is False
    _sql, args = pool.fetch_calls[0]
    assert args == ("AVGO", "24", ["bing_news_rss", "firecrawl"])


@pytest.mark.asyncio
async def test_recent_run_skips_when_all_requested_providers_ran() -> None:
    pool = FakeRecentRunPool(["gdelt_doc", "bing_news_rss", "firecrawl"])

    recent = await _recent_run_exists(
        pool,
        "AVGO",
        max_age_hours=24,
        providers=["gdelt_doc", "bing_news_rss", "firecrawl"],
    )

    assert recent is True


def test_evidence_strength_maps_credibility_to_confidence() -> None:
    assert _evidence_strength("primary") == 0.9
    assert _evidence_strength("credible_media") == 0.75
    assert _evidence_strength("industry") == 0.6
    assert _evidence_strength("unknown") == 0.4
    assert _evidence_strength("unrecognized") == 0.4


@pytest.mark.asyncio
async def test_firecrawl_provider_parses_search_results() -> None:
    async def handler(request: httpx.Request) -> httpx.Response:
        assert request.url == "http://firecrawl.local/v2/search"
        payload = json.loads(request.content)
        assert payload["query"] == "AMD MI400 deployment"
        assert payload["sources"] == ["web"]
        return httpx.Response(
            200,
            json={
                "id": "fc-job-1",
                "creditsUsed": 2,
                "data": {
                    "web": [
                        {
                            "title": "Advanced Micro Devices MI400 deployment update",
                            "url": "https://www.amd.com/en/newsroom/mi400",
                            "description": "AMD customer deployment note",
                            "metadata": {
                                "siteName": "AMD",
                                "publishedTime": "2026-06-20T12:00:00Z",
                            },
                        },
                        {
                            "title": "No URL row",
                            "description": "ignored",
                        },
                    ],
                    "news": [
                        {
                            "title": "AMD MI400 customer adoption expands",
                            "url": "https://www.reuters.com/technology/amd-mi400",
                            "date": "Fri, 19 Jun 2026 10:00:00 GMT",
                            "snippet": "Reuters report",
                        },
                    ],
                },
            },
        )

    provider = FirecrawlProvider(
        base_url="http://firecrawl.local",
        transport=httpx.MockTransport(handler),
    )
    try:
        results = await provider.search("AMD MI400 deployment", max_results=5)
    finally:
        await provider.close()

    assert [row.title for row in results] == [
        "Advanced Micro Devices MI400 deployment update",
        "AMD MI400 customer adoption expands",
    ]
    assert results[0].publisher == "AMD"
    assert results[0].source_type == "web_search"
    assert results[0].credibility == "primary"
    assert results[0].published_at == dt.datetime(2026, 6, 20, 12, tzinfo=dt.UTC)
    assert results[0].source_ref["firecrawl_job_id"] == "fc-job-1"
    assert results[0].source_ref["firecrawl_credits_used"] == 2
    assert results[1].source_type == "news_search"
    assert results[1].credibility == "credible_media"


def test_research_relevance_accepts_company_and_specific_product_match() -> None:
    relevance = research_relevance(
        "AMD",
        {"company_name": "Advanced Micro Devices, Inc."},
        "AMD MI400 deployment",
        _result("Advanced Micro Devices MI400 customer deployment update"),
        ["AMD", "MI400"],
    )

    assert relevance.accepted
    assert relevance.score == 0.85
    assert "company_alias" in relevance.reasons
    assert "specific_product_term" in relevance.reasons
    assert "mi400" in {term.lower() for term in relevance.matched_terms}


def test_research_relevance_accepts_numeric_exchange_ticker_by_company_alias() -> None:
    relevance = research_relevance(
        "2454.TW",
        {"company_name": "MediaTek Inc."},
        "2454.TW Dimensity customer adoption",
        _result("MediaTek says Dimensity design wins are expanding"),
        ["2454.TW"],
    )

    assert relevance.accepted
    assert "company_alias" in relevance.reasons
    assert "mediatek" in relevance.matched_terms


def test_research_relevance_rejects_unrelated_ticker_collision() -> None:
    relevance = research_relevance(
        "ADI",
        {"company_name": "Analog Devices, Inc."},
        "ADI semiconductor latest demand pricing margins customers",
        _result("Cirrus Logic (CRUS) Q4 2026 Earnings Transcript"),
        ["ADI"],
    )

    assert not relevance.accepted
    assert relevance.rejected_reason == "dominant_unrelated_ticker"
    assert relevance.unrelated_tickers == ("CRUS",)


def test_research_relevance_rejects_generic_theme_without_symbol_or_company() -> None:
    relevance = research_relevance(
        "DELL",
        {"company_name": "Dell Technologies Inc."},
        "Dell enterprise AI customer deployment benchmark",
        _result("Enterprise AI spending remains strong across server buyers"),
        ["DELL", "enterprise AI"],
    )

    assert not relevance.accepted
    assert relevance.rejected_reason == "generic_theme_without_symbol_or_company"
    assert "generic_theme_term" in relevance.reasons


def test_research_relevance_accepts_specific_product_without_company_name() -> None:
    relevance = research_relevance(
        "AMD",
        {"company_name": "Advanced Micro Devices, Inc."},
        "AMD MI400 deployment",
        _result("MI400 benchmark results point to production deployment progress"),
        ["AMD", "MI400"],
    )

    assert relevance.accepted
    assert relevance.score == 0.75
    assert relevance.reasons == ("specific_product_term",)


@pytest.mark.asyncio
async def test_load_research_evidence_filters_unvetted_rows() -> None:
    pool = FakeLoadResearchPool()

    rows = await load_research_evidence(pool, "ADI", limit=2)

    assert pool.fetch_args == ("ADI", 8)
    assert [row["title"] for row in rows] == ["Analog Devices (ADI) margin update"]
    assert rows[0]["relevance"]["accepted"] is True


@pytest.mark.asyncio
async def test_insert_result_upserts_product_research_evidence_item_on_refresh() -> None:
    pool = FakeResearchPool()
    published_at = dt.datetime(2026, 6, 1, 12, tzinfo=dt.UTC)
    result = SearchResult(
        title="AMD MI400 customer deployment update",
        url="https://example.com/amd-mi400",
        publisher="Reuters",
        published_at=published_at,
        summary="Deployment note",
        source_type="news_search",
        credibility="credible_media",
        source_ref={"provider_payload_id": "abc"},
    )
    relevance = research_relevance(
        "AMD",
        {"company_name": "Advanced Micro Devices, Inc."},
        "AMD MI400 deployment",
        result,
        ["AMD", "MI400"],
    )

    inserted = await _insert_result(
        pool,
        symbol="AMD",
        query="AMD MI400 deployment",
        provider="bing_news_rss",
        result=result,
        tags=["AMD", "MI400"],
        relevance=relevance,
    )

    assert inserted is False
    _, fetch_args = pool.fetchrow_calls[0]
    insert_source_ref = json.loads(fetch_args[11])
    assert insert_source_ref["relevance"]["accepted"] is True
    assert insert_source_ref["relevance"]["score"] == 0.9
    assert len(pool.execute_calls) == 1
    sql, args = pool.execute_calls[0]
    assert "updated_at = now()" in sql
    source_ref = json.loads(args[4])
    assert args[0] == "AMD"
    assert args[1] == published_at
    assert args[2] == "web_research"
    assert args[3] == "research_evidence:42"
    assert args[5] == "AMD MI400 customer deployment update"
    assert args[6] == 0.75
    assert args[7] == "https://example.com/amd-mi400"
    assert source_ref["table"] == "research_evidence"
    assert source_ref["id"] == 42
    assert source_ref["provider"] == "bing_news_rss"
    assert source_ref["query"] == "AMD MI400 deployment"
    assert source_ref["publisher"] == "Reuters"
    assert source_ref["credibility"] == "credible_media"
    assert source_ref["provider_payload_id"] == "abc"
    assert source_ref["relevance"]["accepted"] is True
    assert source_ref["relevance"]["score"] == 0.9
