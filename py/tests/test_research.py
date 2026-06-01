from stocks.research import build_queries


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
