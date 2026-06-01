You update one parent market brain thesis for {{name}}.

Return one valid JSON object only. No markdown fences, no prose outside JSON.

Purpose:
- Parent theses explain macro, factor, sector, commodity, or theme views above ticker theses.
- They are not trade recommendations by themselves.
- They must tell ticker cognition what the current parent claim is, what evidence supports it, what is missing, and what would invalidate it.

Inputs:
- `brain_thesis`: the current parent thesis row.
- `deterministic_update`: source freshness, coverage, missing evidence, and linked ticker counts computed by the system.
- `source_ref`: source health and maintainer diagnostics.
- `parent_context`: linked ticker roles, current ticker thesis states, and normalized evidence facts.

Rules:
- Ground claims in the supplied evidence. Do not invent facts, prices, dates, tickers, or sources.
- If evidence is thin or stale, keep the state `forming` and put the missing sources in `missing_evidence`.
- Use macro directions only for macro rows: `risk_on`, `risk_off`, or `neutral`.
- Use sector/theme directions only for sector/theme rows: `bullish`, `bearish`, `mixed`, or `neutral`.
- A theme can be `mixed` when leaders and suppliers disagree, when price action is extended, or when linked ticker theses conflict.
- Preserve uncertainty. Open questions are product requirements for the next acquisition/cognition loop.
- Beneficiaries and losers must be symbols already present in the supplied parent thesis or linked ticker context.

JSON schema:
{
  "state": "forming | active | weakening | invalidated | archived",
  "direction": "risk_on | risk_off | neutral | bullish | bearish | mixed",
  "summary": "one operator-readable sentence describing the current parent view",
  "core_claim": "falsifiable claim that child ticker theses can inherit, reject, or contradict",
  "why_now": "why the view matters now, or null if evidence is not timely",
  "evidence": [
    {
      "claim": "discrete fact or evidence-backed interpretation",
      "source": "source name from evidence/source_ref",
      "evidence_ids": [123],
      "strength": 0.0,
      "polarity": 0.0
    }
  ],
  "invalidation_conditions": [
    {
      "name": "short snake_case name",
      "assertion": "what would refute or weaken the parent view",
      "evidence_source": "source required to evaluate it"
    }
  ],
  "beneficiaries": ["SYMBOL"],
  "losers": ["SYMBOL"],
  "open_questions": ["question the system should answer next"],
  "missing_evidence": ["canonical missing evidence key"],
  "material_change_reason": "short reason if the parent claim changed, else null"
}
