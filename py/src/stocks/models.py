"""Pydantic models for structured LLM output (theses, context updates).

Mirror the db schema (SPEC §5); what the provider must return as valid JSON.
"""

from __future__ import annotations

from typing import Any, Literal

from pydantic import BaseModel, Field


class Forecast(BaseModel):
    """Validation instrument only — does NOT drive exits (SPEC §5.3)."""

    direction: Literal["up", "down"]
    magnitude_rough: str
    horizon: str


class Condition(BaseModel):
    type: Literal["quantitative", "narrative"]
    expr: str | None = None  # quantitative: expression over indicators
    assertion: str | None = None  # narrative: LLM-evaluated assertion


class ThesisDraft(BaseModel):
    symbol: str
    cluster_id: str | None = None
    cluster_thesis: str | None = None
    bull_case: str
    bear_case: str
    edge_rationale: str = Field(..., min_length=1)  # required: the diffusion gap
    forecast: Forecast | None = None
    conviction_conditions: list[Condition] = Field(default_factory=list)
    trigger_conditions: list[Condition] = Field(default_factory=list)
    invalidation_conditions: list[Condition] = Field(default_factory=list)
    fulfillment_conditions: list[Condition] = Field(default_factory=list)
    conviction_tier: Literal["high", "medium", "low"] | None = None
    system_confidence: Literal["low", "medium", "high", "very_high"] | None = None
    system_confidence_components: dict[str, Any] = Field(default_factory=dict)
    instrument: Literal["equity", "leaps"] | None = None


class ContextBandUpdate(BaseModel):
    """An LLM-proposed update to one freshness band of a ticker context."""

    band: Literal["structural", "narrative", "market"]
    fields: dict[str, Any]
    as_of: str  # ISO-8601
    significant_shift: bool = False  # → emits context.shift alert (SPEC §3 FR7)
    shift_reason: str | None = None
