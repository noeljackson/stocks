"""Runtime configuration from the environment (mirrors the Rust config)."""

from __future__ import annotations

import os
from dataclasses import dataclass


@dataclass(frozen=True)
class Config:
    database_url: str
    nats_url: str
    llm_provider: str  # "" → auto-detect; else "anthropic" | "openai_compat" | "mock"
    model_deep: str
    model_routine: str
    model_triage: str
    # LLM transport
    anthropic_base_url: str
    anthropic_api_key: str
    anthropic_version: str
    openai_base_url: str
    openai_api_key: str


def load() -> Config:
    return Config(
        database_url=os.getenv(
            "DATABASE_URL",
            "postgres://stocks:stocks_dev_only@localhost:5432/stocks",
        ),
        nats_url=os.getenv("NATS_URL", "nats://localhost:4222"),
        llm_provider=os.getenv("LLM_PROVIDER", ""),
        model_deep=os.getenv("LLM_MODEL_DEEP", "glm-5.1"),
        model_routine=os.getenv("LLM_MODEL_ROUTINE", "glm-5.1"),
        model_triage=os.getenv("LLM_MODEL_TRIAGE", "glm-5-turbo"),
        anthropic_base_url=os.getenv(
            "ANTHROPIC_BASE_URL", "https://api.z.ai/api/anthropic"
        ),
        anthropic_api_key=os.getenv("ANTHROPIC_API_KEY", ""),
        anthropic_version=os.getenv("ANTHROPIC_VERSION", "2023-06-01"),
        openai_base_url=os.getenv("OPENAI_BASE_URL", ""),
        openai_api_key=os.getenv("OPENAI_API_KEY", ""),
    )
