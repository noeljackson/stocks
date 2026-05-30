"""Swappable LLM provider abstraction (SPEC §3 invariant), mirrors the Go iface.

Two real transports plus a mock; selection via env (see config.Config):

- ``anthropic``       Anthropic Messages shape; works with Anthropic direct,
                      z.ai (ANTHROPIC_BASE_URL=https://api.z.ai/api/anthropic),
                      Bedrock/Vertex proxies.
- ``openai_compat``   /v1/chat/completions shape; works with DeepSeek,
                      Together, OpenRouter, vLLM, Groq.
- ``mock``            Returns a fixed JSON payload; default for tests.
"""

from __future__ import annotations

import logging
from dataclasses import dataclass, field
from typing import Any, Protocol

import httpx

logger = logging.getLogger(__name__)


@dataclass
class Message:
    role: str  # "user" | "assistant"
    content: str


@dataclass
class Request:
    model: str = ""
    system: str = ""
    messages: list[Message] = field(default_factory=list)
    max_tokens: int = 0
    # json_schema, when set, is appended to the system prompt asking the model
    # to emit JSON matching the schema. Same approach as the Go transport.
    json_schema: dict[str, Any] | None = None


@dataclass
class Usage:
    input_tokens: int = 0
    output_tokens: int = 0


@dataclass
class Response:
    content: str
    usage: Usage = field(default_factory=Usage)
    model: str = ""


@dataclass
class TransportConfig:
    provider: str = "mock"
    model: str = ""
    anthropic_base_url: str = "https://api.anthropic.com"
    anthropic_auth_token: str = ""
    anthropic_version: str = "2023-06-01"
    openai_base_url: str = ""
    openai_api_key: str = ""


class Provider(Protocol):
    async def complete(self, req: Request) -> Response: ...


# ---------- mock ----------


class MockProvider:
    async def complete(self, req: Request) -> Response:  # noqa: ARG002
        return Response(content='{"mock": true}', model="mock")


# ---------- common helpers ----------


def _append_schema(system: str, schema: dict[str, Any] | None) -> str:
    if not schema:
        return system
    import json

    suffix = (
        "\n\nRespond ONLY with JSON matching this schema "
        "(no prose, no markdown fences):\n" + json.dumps(schema)
    )
    return (system or "Respond with JSON only.") + suffix


def _truncate(s: str, n: int = 512) -> str:
    return s if len(s) <= n else s[:n] + "…"


# ---------- anthropic ----------


class AnthropicProvider:
    def __init__(
        self,
        base_url: str,
        token: str,
        *,
        version: str = "2023-06-01",
        model: str = "",
        client: httpx.AsyncClient | None = None,
    ) -> None:
        self._base = base_url.rstrip("/")
        self._token = token
        self._version = version
        self._model = model
        self._client = client or httpx.AsyncClient(timeout=120.0)

    async def complete(self, req: Request) -> Response:
        body: dict[str, Any] = {
            "model": req.model or self._model,
            "messages": [{"role": m.role, "content": m.content} for m in req.messages],
            "max_tokens": req.max_tokens or 4096,
        }
        system = _append_schema(req.system, req.json_schema)
        if system:
            body["system"] = system
        r = await self._client.post(
            f"{self._base}/v1/messages",
            json=body,
            headers={
                "x-api-key": self._token,
                "anthropic-version": self._version,
                "content-type": "application/json",
            },
        )
        if r.status_code != 200:
            raise RuntimeError(f"anthropic {r.status_code}: {_truncate(r.text)}")
        parsed = r.json()
        if "error" in parsed and parsed["error"]:
            err = parsed["error"]
            raise RuntimeError(f"anthropic {err.get('type')}: {err.get('message')}")
        content = "".join(
            c.get("text", "") for c in parsed.get("content", []) if c.get("type") == "text"
        )
        usage = parsed.get("usage", {})
        return Response(
            content=content,
            model=parsed.get("model", ""),
            usage=Usage(
                input_tokens=usage.get("input_tokens", 0),
                output_tokens=usage.get("output_tokens", 0),
            ),
        )


# ---------- openai-compat ----------


class OpenAICompatProvider:
    def __init__(
        self,
        base_url: str,
        api_key: str,
        *,
        model: str = "",
        client: httpx.AsyncClient | None = None,
    ) -> None:
        # Strip a trailing /v1 if the user supplied it; we always append /v1/chat/completions.
        base = base_url.rstrip("/")
        if base.endswith("/v1"):
            base = base[: -len("/v1")]
        self._base = base
        self._key = api_key
        self._model = model
        self._client = client or httpx.AsyncClient(timeout=120.0)

    async def complete(self, req: Request) -> Response:
        system = _append_schema(req.system, req.json_schema)
        msgs: list[dict[str, str]] = []
        if system:
            msgs.append({"role": "system", "content": system})
        msgs.extend({"role": m.role, "content": m.content} for m in req.messages)

        body: dict[str, Any] = {
            "model": req.model or self._model,
            "messages": msgs,
            "max_tokens": req.max_tokens or 4096,
        }
        r = await self._client.post(
            f"{self._base}/v1/chat/completions",
            json=body,
            headers={
                "Authorization": f"Bearer {self._key}",
                "content-type": "application/json",
            },
        )
        if r.status_code != 200:
            raise RuntimeError(f"openai {r.status_code}: {_truncate(r.text)}")
        parsed = r.json()
        if "error" in parsed and parsed["error"]:
            err = parsed["error"]
            raise RuntimeError(f"openai {err.get('type')}: {err.get('message')}")
        choices = parsed.get("choices") or []
        content = choices[0]["message"]["content"] if choices else ""
        usage = parsed.get("usage", {})
        return Response(
            content=content,
            model=parsed.get("model", ""),
            usage=Usage(
                input_tokens=usage.get("prompt_tokens", 0),
                output_tokens=usage.get("completion_tokens", 0),
            ),
        )


# ---------- factory ----------


def new_provider(cfg: TransportConfig) -> Provider:
    """Return a provider configured from cfg. Falls back to mock for missing
    required config (never raises at construction time)."""
    if cfg.provider == "anthropic":
        if not cfg.anthropic_auth_token:
            logger.warning("llm anthropic: missing ANTHROPIC_AUTH_TOKEN, using mock")
            return MockProvider()
        return AnthropicProvider(
            cfg.anthropic_base_url,
            cfg.anthropic_auth_token,
            version=cfg.anthropic_version or "2023-06-01",
            model=cfg.model,
        )
    if cfg.provider in ("openai_compat", "openai"):
        if not (cfg.openai_base_url and cfg.openai_api_key):
            logger.warning(
                "llm openai_compat: missing OPENAI_BASE_URL or OPENAI_API_KEY, using mock"
            )
            return MockProvider()
        return OpenAICompatProvider(
            cfg.openai_base_url,
            cfg.openai_api_key,
            model=cfg.model,
        )
    return MockProvider()
