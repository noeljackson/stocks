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
    # LLM transport
    anthropic_base_url: str
    anthropic_auth_token: str
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
        llm_provider=os.getenv("LLM_PROVIDER", "mock"),
        model_deep=os.getenv("LLM_MODEL_DEEP", "claude-opus-4-8"),
        model_routine=os.getenv("LLM_MODEL_ROUTINE", "glm-4.6"),
        model_triage=os.getenv("LLM_MODEL_TRIAGE", "glm-4.5-air"),
        anthropic_base_url=os.getenv(
            "ANTHROPIC_BASE_URL", "https://api.anthropic.com"
        ),
        anthropic_auth_token=os.getenv("ANTHROPIC_AUTH_TOKEN", ""),
        anthropic_version=os.getenv("ANTHROPIC_VERSION", "2023-06-01"),
        openai_base_url=os.getenv("OPENAI_BASE_URL", ""),
        openai_api_key=os.getenv("OPENAI_API_KEY", ""),
    )
