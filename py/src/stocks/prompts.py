"""Python mirror of `src/llm/prompts.rs` (issue #6/#7).

Shared `prompts/*.md` directory across Rust + Python; same sha256 hash so
audit rows from either side are comparable. Same `llm_invocation` table.
"""

from __future__ import annotations

import asyncio
import hashlib
import json as _json
import logging
import time
from collections.abc import Mapping
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Protocol, TypeVar

import pydantic

from .llm import Message, Provider, Request, Response

logger = logging.getLogger(__name__)


@dataclass
class Prompt:
    """Loaded prompt: filename stem = name, sha256 of content = hash."""

    name: str
    hash: str
    template: str

    def render(self, vars: Mapping[str, str]) -> str:
        """Substitute `{{key}}` placeholders. Unknown ones pass through."""
        out = self.template
        for k, v in vars.items():
            out = out.replace("{{" + k + "}}", v)
        return out


@dataclass
class Registry:
    by_name: dict[str, Prompt]

    def get(self, name: str) -> Prompt | None:
        return self.by_name.get(name)

    def names(self) -> list[str]:
        return sorted(self.by_name.keys())

    def __len__(self) -> int:
        return len(self.by_name)


def load(dir_path: str | Path) -> Registry:
    """Load every `*.md` in `dir_path` into a Registry."""
    d = Path(dir_path)
    if not d.is_dir():
        raise FileNotFoundError(f"prompts dir not found: {d}")
    by_name: dict[str, Prompt] = {}
    for path in d.iterdir():
        if path.suffix != ".md":
            continue
        text = path.read_text(encoding="utf-8")
        h = hashlib.sha256(text.encode("utf-8")).hexdigest()
        by_name[path.stem] = Prompt(name=path.stem, hash=h, template=text)
    return Registry(by_name=by_name)


class InvocationRecorder(Protocol):
    """Sink for audit rows; tests can pass a no-op."""

    async def record(
        self,
        *,
        prompt_name: str,
        prompt_hash: str,
        provider: str,
        model: str,
        input_tokens: int,
        output_tokens: int,
        latency_ms: int,
        request_summary: str,
        response_summary: str,
    ) -> None: ...


def _summary(s: str, n: int = 200) -> str:
    return s if len(s) <= n else s[:n] + "…"


async def invoke(
    provider: Provider,
    recorder: InvocationRecorder | None,
    prompt: Prompt,
    vars: Mapping[str, str],
    user_message: str,
    provider_name: str,
    *,
    model: str = "",
    max_tokens: int = 0,
) -> Response:
    """Call provider with a rendered prompt, optionally record to audit table."""
    system = prompt.render(vars)
    started = time.monotonic()
    resp = await provider.complete(
        Request(
            model=model,
            system=system,
            messages=[Message(role="user", content=user_message)],
            max_tokens=max_tokens,
        )
    )
    elapsed_ms = int((time.monotonic() - started) * 1000)

    if recorder is not None:
        try:
            await recorder.record(
                prompt_name=prompt.name,
                prompt_hash=prompt.hash,
                provider=provider_name,
                model=resp.model,
                input_tokens=resp.usage.input_tokens,
                output_tokens=resp.usage.output_tokens,
                latency_ms=elapsed_ms,
                request_summary=_summary(system),
                response_summary=_summary(resp.content),
            )
        except Exception:  # noqa: BLE001  audit failure must not break the call
            logger.exception("llm_invocation record failed (non-fatal)")
    return resp


# ---------- asyncpg-backed recorder (used by services) ----------


class AsyncpgRecorder:
    """Records `llm_invocation` rows via an asyncpg pool."""

    def __init__(self, pool: Any) -> None:  # asyncpg.Pool — kept loose for tests
        self._pool = pool

    async def record(
        self,
        *,
        prompt_name: str,
        prompt_hash: str,
        provider: str,
        model: str,
        input_tokens: int,
        output_tokens: int,
        latency_ms: int,
        request_summary: str,
        response_summary: str,
    ) -> None:
        await self._pool.execute(
            """INSERT INTO llm_invocation
                 (prompt_name, prompt_hash, provider, model,
                  input_tokens, output_tokens, latency_ms,
                  request_summary, response_summary)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)""",
            prompt_name,
            prompt_hash,
            provider,
            model,
            input_tokens,
            output_tokens,
            latency_ms,
            request_summary,
            response_summary,
        )


# ---------- typed invocation with auto-retry (#28) ----------

T = TypeVar("T", bound=pydantic.BaseModel)


def _extract_json(content: str) -> str:
    """Strip markdown fences + grab first {...} or [...] block.
    LLMs ignore "no fences" instructions sometimes."""
    s = content.strip()
    for fence in ("```json", "```"):
        if s.startswith(fence):
            s = s[len(fence):].lstrip()
            break
    if s.endswith("```"):
        s = s[:-3].rstrip()
    try:
        _json.loads(s)
        return s
    except _json.JSONDecodeError:
        pass
    # Fallback: grab first balanced {...} or [...].
    for open_c, close_c in [("{", "}"), ("[", "]")]:
        start = s.find(open_c)
        end = s.rfind(close_c)
        if 0 <= start < end:
            return s[start:end + 1]
    return s


async def invoke_typed(  # noqa: UP047 (TypeVar form supports 3.10+; PEP-695 syntax is 3.12+)
    provider: Provider,
    recorder: InvocationRecorder | None,
    prompt: Prompt,
    vars: Mapping[str, str],
    user_message: str,
    provider_name: str,
    model_cls: type[T],
    *,
    model: str = "",
    max_tokens: int = 0,
    max_retries: int = 2,
) -> T:
    """Like ``invoke``, but parses into ``model_cls`` (a pydantic BaseModel)
    and retries on validation failure. Mirrors the Rust ``complete_typed``."""
    current_user = user_message
    last_err = ""
    for attempt in range(max_retries + 1):
        resp = await invoke(
            provider, recorder, prompt, vars, current_user,
            provider_name, model=model, max_tokens=max_tokens,
        )
        raw = _extract_json(resp.content)
        try:
            return model_cls.model_validate_json(raw)
        except pydantic.ValidationError as e:
            last_err = str(e)
            if attempt == max_retries:
                raise RuntimeError(
                    f"invoke_typed: schema parse failed after {max_retries} retries: "
                    f"{last_err} (raw: {raw[:200]!r})"
                ) from e
            logger.warning("invoke_typed parse failed (attempt %d): %s", attempt, last_err)
            current_user = (
                f"{user_message}\n\n"
                f"[Previous attempt failed JSON-schema validation with error: "
                f'"{last_err}". Reply ONLY with valid JSON matching the '
                f"schema; no prose, no markdown fences.]"
            )
    # Unreachable.
    raise RuntimeError(f"invoke_typed: unreachable retry loop exit ({last_err})")


# small helper so tests can run async funcs without pytest-asyncio gymnastics
def run(coro: Any) -> Any:  # pragma: no cover
    return asyncio.run(coro)
