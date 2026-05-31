"""Tests for the Python prompt registry (#6/#7)."""

from __future__ import annotations

from pathlib import Path

import pytest

from stocks.prompts import Prompt, load


def write(dir_path: Path, files: dict[str, str]) -> None:
    for name, body in files.items():
        (dir_path / name).write_text(body, encoding="utf-8")


def test_load_indexes_by_filename_stem(tmp_path: Path):
    write(tmp_path, {
        "synthesize-context.md": "do context",
        "draft-thesis.md": "draft for {{symbol}}",
        "README.txt": "not a prompt",  # non-.md ignored
    })
    reg = load(tmp_path)
    assert len(reg) == 2
    assert reg.get("synthesize-context") is not None
    assert reg.get("draft-thesis") is not None
    assert reg.get("README") is None


def test_hash_is_stable_and_content_addressed(tmp_path: Path):
    write(tmp_path, {"p.md": "abc"})
    h1 = load(tmp_path).get("p").hash
    h2 = load(tmp_path).get("p").hash
    assert h1 == h2


def test_hash_changes_when_content_changes(tmp_path: Path):
    write(tmp_path, {"p.md": "abc"})
    h1 = load(tmp_path).get("p").hash
    (tmp_path / "p.md").write_text("abcd", encoding="utf-8")
    h2 = load(tmp_path).get("p").hash
    assert h1 != h2


def test_render_substitutes():
    p = Prompt(name="p", hash="x", template="hi {{name}}, you are a {{role}}")
    assert p.render({"name": "noel", "role": "trader"}) == "hi noel, you are a trader"


def test_render_passes_unknown_placeholders_through():
    p = Prompt(name="p", hash="x", template="{{a}} and {{b}}")
    # Unknown stays visible so prompt-writers can spot it.
    assert p.render({"a": "ok"}) == "ok and {{b}}"


def test_load_missing_dir_raises():
    with pytest.raises(FileNotFoundError):
        load("/does/not/exist")


def test_python_and_rust_hashes_match():
    """The Rust loader uses the same sha256(file_bytes) scheme. A shared
    `prompts/echo.md` should produce the same hash from both sides — this
    test pins the Python side; the Rust llmsmoke output shows the same."""
    import hashlib
    repo_root = Path(__file__).resolve().parents[2]
    p = repo_root / "prompts" / "echo.md"
    assert p.exists(), f"expected shared prompt at {p}"
    expected = hashlib.sha256(p.read_bytes()).hexdigest()
    loaded = load(repo_root / "prompts").get("echo")
    assert loaded is not None
    assert loaded.hash == expected


# ---------- _extract_json + invoke_typed (#28) ----------


def test_extract_json_passthrough_clean():
    from stocks.prompts import _extract_json

    assert _extract_json('{"a":1}') == '{"a":1}'


def test_extract_json_strips_fences():
    from stocks.prompts import _extract_json

    assert _extract_json('```json\n{"a":1}\n```') == '{"a":1}'
    assert _extract_json('```\n{"a":1}\n```') == '{"a":1}'


def test_extract_json_finds_object_in_prose():
    from stocks.prompts import _extract_json

    s = 'Sure: {"a":1,"b":2} done.'
    assert _extract_json(s) == '{"a":1,"b":2}'


def test_extract_json_handles_arrays():
    from stocks.prompts import _extract_json

    assert _extract_json('```json\n[1,2,3]\n```') == '[1,2,3]'


class _ScriptedProvider:
    """Test double — returns scripted responses in order."""

    def __init__(self, responses: list[str]) -> None:
        self.responses = list(responses)

    async def complete(self, _req):  # noqa: ANN001
        from stocks.llm import Response

        next_body = self.responses.pop(0)
        return Response(content=next_body, model="scripted")


@pytest.mark.asyncio
async def test_invoke_typed_succeeds_first_try():
    import pydantic

    from stocks.prompts import Prompt, invoke_typed

    class Demo(pydantic.BaseModel):
        n: int
        s: str

    p = Prompt(name="demo", hash="h", template="demo")
    out = await invoke_typed(
        provider=_ScriptedProvider(['{"n":42,"s":"ok"}']),
        recorder=None,
        prompt=p,
        vars={},
        user_message="go",
        provider_name="scripted",
        model_cls=Demo,
    )
    assert out == Demo(n=42, s="ok")


@pytest.mark.asyncio
async def test_invoke_typed_retries_then_succeeds():
    import pydantic

    from stocks.prompts import Prompt, invoke_typed

    class Demo(pydantic.BaseModel):
        n: int

    p = Prompt(name="demo", hash="h", template="demo")
    out = await invoke_typed(
        provider=_ScriptedProvider(["not json", '{"n":7}']),
        recorder=None,
        prompt=p,
        vars={},
        user_message="go",
        provider_name="scripted",
        model_cls=Demo,
    )
    assert out.n == 7


@pytest.mark.asyncio
async def test_invoke_typed_gives_up_after_max_retries():
    import pydantic

    from stocks.prompts import Prompt, invoke_typed

    class Demo(pydantic.BaseModel):
        n: int

    p = Prompt(name="demo", hash="h", template="demo")
    with pytest.raises(RuntimeError, match="schema parse failed"):
        await invoke_typed(
            provider=_ScriptedProvider(["x", "y", "z"]),
            recorder=None,
            prompt=p,
            vars={},
            user_message="go",
            provider_name="scripted",
            model_cls=Demo,
            max_retries=2,  # → 3 total attempts → all fail
        )
