"""Tests for the LLM transport layer (mirrors the Go tests)."""

from __future__ import annotations

import json
from typing import Any

import httpx
import pytest

from stocks.llm import (
    AnthropicProvider,
    Message,
    MockProvider,
    OpenAICompatProvider,
    Request,
    TransportConfig,
    new_provider,
)

ANTHROPIC_HAPPY = {
    "id": "msg_01",
    "type": "message",
    "role": "assistant",
    "content": [{"type": "text", "text": "hello world"}],
    "model": "glm-5.1",
    "stop_reason": "end_turn",
    "usage": {"input_tokens": 12, "output_tokens": 3},
}

OPENAI_HAPPY = {
    "id": "cmpl_01",
    "choices": [
        {
            "message": {"role": "assistant", "content": "hi back"},
            "finish_reason": "stop",
            "index": 0,
        }
    ],
    "model": "deepseek-chat",
    "usage": {"prompt_tokens": 7, "completion_tokens": 2, "total_tokens": 9},
}


def _mock_transport(
    capture: list[httpx.Request], status: int = 200, body: dict[str, Any] | None = None
) -> httpx.MockTransport:
    body = body if body is not None else ANTHROPIC_HAPPY

    def handler(request: httpx.Request) -> httpx.Response:
        capture.append(request)
        return httpx.Response(status, json=body)

    return httpx.MockTransport(handler)


# ---------- anthropic ----------


@pytest.mark.asyncio
async def test_anthropic_happy_path():
    captured: list[httpx.Request] = []
    client = httpx.AsyncClient(transport=_mock_transport(captured, body=ANTHROPIC_HAPPY))
    p = AnthropicProvider("https://x", "test-token", model="glm-5.1", client=client)

    r = await p.complete(Request(system="be precise", messages=[Message("user", "say hi")]))
    assert r.content == "hello world"
    assert r.usage.input_tokens == 12
    assert r.usage.output_tokens == 3

    sent = json.loads(captured[0].content)
    assert sent["model"] == "glm-5.1"
    assert sent["system"] == "be precise"
    assert "max_tokens" in sent, "anthropic requires max_tokens"


@pytest.mark.asyncio
async def test_anthropic_sends_api_key_and_version():
    captured: list[httpx.Request] = []
    client = httpx.AsyncClient(transport=_mock_transport(captured))
    p = AnthropicProvider("https://x", "tok", client=client)
    await p.complete(Request(messages=[Message("user", "x")]))
    assert captured[0].headers["x-api-key"] == "tok"
    assert captured[0].headers["anthropic-version"]


@pytest.mark.asyncio
async def test_anthropic_http_error_propagates():
    transport = httpx.MockTransport(
        lambda _r: httpx.Response(401, json={"error": {"type": "auth", "message": "bad key"}})
    )
    client = httpx.AsyncClient(transport=transport)
    p = AnthropicProvider("https://x", "tok", client=client)
    with pytest.raises(RuntimeError, match="401"):
        await p.complete(Request(messages=[Message("user", "x")]))


@pytest.mark.asyncio
async def test_anthropic_json_schema_appends_to_system():
    captured: list[httpx.Request] = []
    client = httpx.AsyncClient(transport=_mock_transport(captured))
    p = AnthropicProvider("https://x", "tok", client=client)
    schema = {"type": "object", "properties": {"x": {"type": "number"}}}
    await p.complete(
        Request(system="be helpful", messages=[Message("user", "x")], json_schema=schema)
    )
    sys = json.loads(captured[0].content)["system"]
    assert "be helpful" in sys
    assert "JSON" in sys and "schema" in sys


# ---------- openai-compat ----------


@pytest.mark.asyncio
async def test_openai_happy_path():
    captured: list[httpx.Request] = []
    client = httpx.AsyncClient(transport=_mock_transport(captured, body=OPENAI_HAPPY))
    p = OpenAICompatProvider("https://x", "sk-test", model="deepseek-chat", client=client)
    r = await p.complete(Request(system="be terse", messages=[Message("user", "hi")]))
    assert r.content == "hi back"
    assert r.usage.input_tokens == 7
    assert r.usage.output_tokens == 2

    sent = json.loads(captured[0].content)
    assert sent["messages"][0] == {"role": "system", "content": "be terse"}
    assert sent["messages"][1] == {"role": "user", "content": "hi"}


@pytest.mark.asyncio
async def test_openai_sends_bearer():
    captured: list[httpx.Request] = []
    client = httpx.AsyncClient(transport=_mock_transport(captured, body=OPENAI_HAPPY))
    p = OpenAICompatProvider("https://x", "sk-test", client=client)
    await p.complete(Request(messages=[Message("user", "x")]))
    assert captured[0].headers["Authorization"] == "Bearer sk-test"


@pytest.mark.asyncio
async def test_openai_strips_v1_suffix():
    captured: list[httpx.Request] = []
    client = httpx.AsyncClient(transport=_mock_transport(captured, body=OPENAI_HAPPY))
    # User accidentally appends /v1; we should still land on /v1/chat/completions exactly once.
    p = OpenAICompatProvider("https://x/v1", "sk", client=client)
    await p.complete(Request(messages=[Message("user", "x")]))
    assert captured[0].url.path == "/v1/chat/completions"


@pytest.mark.asyncio
async def test_openai_http_error_propagates():
    transport = httpx.MockTransport(
        lambda _r: httpx.Response(429, json={"error": {"message": "rate limited"}})
    )
    client = httpx.AsyncClient(transport=transport)
    p = OpenAICompatProvider("https://x", "k", client=client)
    with pytest.raises(RuntimeError, match="429"):
        await p.complete(Request(messages=[Message("user", "x")]))


# ---------- factory ----------


def test_factory_unknown_provider_returns_mock():
    assert isinstance(new_provider(TransportConfig(provider="???")), MockProvider)


def test_factory_anthropic_without_key_returns_mock():
    assert isinstance(new_provider(TransportConfig(provider="anthropic")), MockProvider)


def test_factory_openai_without_base_returns_mock():
    assert isinstance(
        new_provider(TransportConfig(provider="openai_compat", openai_api_key="k")),
        MockProvider,
    )


def test_factory_openai_without_key_returns_mock():
    assert isinstance(
        new_provider(TransportConfig(provider="openai_compat", openai_base_url="https://x")),
        MockProvider,
    )


# ---------- auto-detect ----------


def test_detect_anthropic_when_key_present():
    from stocks.llm import detect

    assert detect(TransportConfig(anthropic_api_key="k")) == "anthropic"


def test_detect_openai_when_both_present():
    from stocks.llm import detect

    cfg = TransportConfig(openai_base_url="https://x", openai_api_key="k")
    assert detect(cfg) == "openai_compat"


def test_detect_anthropic_wins_over_openai():
    from stocks.llm import detect

    cfg = TransportConfig(
        anthropic_api_key="ak",
        openai_base_url="https://x",
        openai_api_key="ok",
    )
    assert detect(cfg) == "anthropic"


def test_detect_falls_back_to_mock():
    from stocks.llm import detect

    assert detect(TransportConfig()) == "mock"


def test_factory_zero_config_uses_auto():
    # Empty provider + anthropic key → AnthropicProvider (not mock).
    from stocks.llm import AnthropicProvider

    p = new_provider(TransportConfig(anthropic_api_key="k"))
    assert isinstance(p, AnthropicProvider)
