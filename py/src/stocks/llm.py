"""Swappable LLM provider abstraction (SPEC §3 invariant), mirrors the Go iface.

v1 uses a subscription transport (user's decision); the interface keeps that
reversible to the API without touching call sites.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, Protocol


@dataclass
class Message:
    role: str  # "user" | "assistant"
    content: str


@dataclass
class Request:
    model: str
    system: str = ""
    messages: list[Message] = field(default_factory=list)
    # json_schema, when set, instructs the provider to return schema-valid JSON.
    json_schema: dict[str, Any] | None = None


@dataclass
class Response:
    content: str


class Provider(Protocol):
    async def complete(self, req: Request) -> Response: ...


class MockProvider:
    async def complete(self, req: Request) -> Response:  # noqa: ARG002
        return Response(content='{"mock": true}')


def new_provider(name: str) -> Provider:
    """Return a provider by name. Real transports go behind this interface."""
    if name in ("anthropic", "subscription"):
        # TODO: real transport (prompt caching + batch + model tiering for API).
        return MockProvider()
    return MockProvider()
