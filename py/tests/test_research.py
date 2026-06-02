import datetime as dt
import json

import pytest

from stocks.research import (
    SearchResult,
    _evidence_strength,
    _insert_result,
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


def test_evidence_strength_maps_credibility_to_confidence() -> None:
    assert _evidence_strength("primary") == 0.9
    assert _evidence_strength("credible_media") == 0.75
    assert _evidence_strength("industry") == 0.6
    assert _evidence_strength("unknown") == 0.4
    assert _evidence_strength("unrecognized") == 0.4


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
