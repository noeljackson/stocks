from stocks.source_tasks import (
    _research_questions_from_ref,
    is_rate_limit_error,
    retry_delay_minutes,
)


def test_retry_delay_minutes_is_bounded_exponential() -> None:
    assert retry_delay_minutes(0) == 15
    assert retry_delay_minutes(1) == 15
    assert retry_delay_minutes(2) == 30
    assert retry_delay_minutes(3) == 60
    assert retry_delay_minutes(20) == 360


def test_is_rate_limit_error_matches_common_vendor_messages() -> None:
    assert is_rate_limit_error("HTTP 429 Too Many Requests")
    assert is_rate_limit_error("rate limit exceeded")
    assert not is_rate_limit_error("connection reset")


def test_research_questions_from_ref_extracts_llm_questions() -> None:
    questions = _research_questions_from_ref({
        "research_requests": [
            {"question": "AVGO Broadcom custom silicon socket wins 2026"},
            {"question": "AVGO Broadcom custom silicon socket wins 2026"},
            {"question": "Broadcom Tomahawk switching deployment momentum"},
        ],
        "llm_research_request": {
            "question": "Broadcom VMware cross-sell evidence after acquisition",
        },
    })

    assert questions == [
        "Broadcom VMware cross-sell evidence after acquisition",
        "AVGO Broadcom custom silicon socket wins 2026",
        "Broadcom Tomahawk switching deployment momentum",
    ]
