import datetime as dt
import json

import pytest

from stocks.research import SearchResult, _evidence_strength, _insert_result, build_queries


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

    inserted = await _insert_result(
        pool,
        symbol="AMD",
        query="AMD MI400 deployment",
        provider="bing_news_rss",
        result=result,
        tags=["AMD", "MI400"],
    )

    assert inserted is False
    assert len(pool.execute_calls) == 1
    _, args = pool.execute_calls[0]
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
