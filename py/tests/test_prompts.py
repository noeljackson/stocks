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
