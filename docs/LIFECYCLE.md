# Lifecycle Map

This is the canonical map for how a ticker moves from broad radar to context,
thesis, decision, position, and outcome. Update this document when adding a new
attention kind, state transition, event source, or operator decision surface.

## Main Flow

```text
discovery_pool                              active ticker/watchlist universe
  broad radar                                  focused radar
  price/news/estimates                         richer context + thesis loop
       │                                                   │
       └──────────────┬────────────────────────────────────┘
                      ▼
              cheap raw detectors
              volume, breakout, news, estimates
                      │
                      ▼
              composed interpretation
              raw facts + extension + thesis/watchlist state
                      │
          ┌───────────┴───────────┐
          │                       │
          ▼                       ▼
 discovery_candidate        thesis/risk/consensus review
 candidate_review           existing thesis trigger,
 attention                  consensus arrival, exhaustion
          │
          ▼
 operator confirm/reject
          │
          ├── reject -> candidate closed, feedback retained
          │
          └── confirm -> watchlist membership / ticker promotion
                                │
                                ▼
                         discovery.confirmed
                                │
                                ▼
                      cognition pipeline
                      context_maintainer
                      thesis_engine
                      sharpen
                      challenge
                                │
            ┌───────────────────┴───────────────────┐
            │                                       │
            ▼                                       ▼
   ticker_context version                 honest no-edge decline
   structural/narrative/market            thesis_incomplete attention
            │
            ▼
        thesis state machine
        forming -> building_conviction -> armed -> actionable
            │
            ▼
      thesis_actionable attention
            │
            ▼
       operator decision
       enter/exit/skip/resize + side/instrument
            │
            ▼
         position
            │
            ▼
         outcome
            │
            ▼
      reflection/calibration
      lead time, Brier, prompt/signal learning
```

## Attention Contract

Attention is not an event log. Attention means "the operator needs to judge
something now." Background services may append many events without creating
attention.

| Kind | Source | Meaning | Resolver |
| --- | --- | --- | --- |
| `candidate_review` | discovery | A composed discovery interpretation deserves confirm/reject. | confirm or reject candidate |
| `thesis_incomplete` | cognition | Context was refreshed but thesis engine declined to invent an edge. | dismiss |
| `thesis_actionable` | thesis transition | A thesis reached actionable and needs a human decision. | record decision |
| `risk_review` | risk | Proposed/recorded intent hit risk warnings or vetoes. | acknowledge / adjust |
| `context_stale` | staler | A thesis depends on stale context. | refresh context |
| `invalidation_hit` | evaluator/staler | Evidence may refute a thesis condition. | review transition/decision |
| `outcome_ready` | reflection | A forecast horizon is ready to score. | score outcome |

## Discovery Composition

Raw detector firings are facts, not operator interpretations. The scanner first
collects raw facts:

```text
volume_anomaly
base_breakout
estimate_revision_velocity
news_sentiment_shift
```

Then it composes them with price extension and symbol state:

```text
raw hits
+ distance from available-window high
+ distance from SMA
+ RSI
+ open/actionable thesis state
+ watchlist membership
-> interpretation
```

Current deterministic interpretations:

| Interpretation | Meaning |
| --- | --- |
| `early_accumulation` | Evidence suggests a name may be moving before broad consensus. |
| `breakout_confirmation` | A base breakout is confirmed by supporting activity. |
| `extended_momentum` | A volume event is happening after the move is already stretched. |
| `consensus_arrival` | Price extension plus positive news/estimate activity suggests crowd arrival. |
| `possible_exhaustion` | Negative evidence is appearing while the name is extended. |
| `existing_thesis_trigger` | Signal belongs to an already actionable thesis and routes to `thesis_actionable`, not generic discovery. |

This avoids treating "volume spike at/near highs" as the same thing as "early
discovery." It is still deterministic and auditable: raw signals are preserved
in `attention_item.source_ref.raw_signals`.

## Context And Thesis

Confirming a discovery candidate publishes `discovery.confirmed`. The cognition
consumer handles that message:

```text
discovery.confirmed
  -> refresh ticker_context
  -> draft thesis
  -> sharpen thesis
  -> challenge thesis
```

The thesis engine may decline. A decline is not failure when there is no
measurable edge. Example: a mega-cap can have fresh context and still get no
thesis if the facts are already consensus and there is no undiffused edge.

## State Ownership

Use these ownership boundaries when changing behavior:

```text
raw detectors       -> src/discovery/signals.rs
interpretation      -> src/discovery/composer.rs
candidate storage   -> discovery_candidate
attention storage   -> attention_item
context memory      -> ticker_context
thesis lifecycle    -> thesis + thesis_state_history
decision log        -> decision
position state      -> position
outcome scoring     -> outcome
```

Append-only tables remain append-only. State changes go through the canonical
service/API path for that object.
