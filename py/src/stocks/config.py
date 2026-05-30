"""Runtime configuration from the environment (mirrors the Go config)."""

from __future__ import annotations

import os
from dataclasses import dataclass


@dataclass(frozen=True)
class Config:
    database_url: str
    nats_url: str
    llm_provider: str
    model_deep: str
    model_routine: str
    model_triage: str


def load() -> Config:
    return Config(
        database_url=os.getenv(
            "DATABASE_URL",
            "postgres://stocks:stocks_dev_only@localhost:5432/stocks",
        ),
        nats_url=os.getenv("NATS_URL", "nats://localhost:4222"),
        llm_provider=os.getenv("LLM_PROVIDER", "mock"),
        model_deep=os.getenv("LLM_MODEL_DEEP", "claude-opus-4-8"),
        model_routine=os.getenv("LLM_MODEL_ROUTINE", "claude-sonnet-4-6"),
        model_triage=os.getenv("LLM_MODEL_TRIAGE", "claude-haiku-4-5"),
    )
