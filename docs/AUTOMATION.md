# Automation V2

Automation is a permissioned extension of the manual decision loop. It is not a
separate trading brain and it is not live order placement by default.

The operator grants permission per `symbol + strategy + version`. After that,
the approved strategy may express desired exposure for its own sleeve, but it
never creates broker orders directly.

```text
strategy definition
  -> ticker+strategy permission
  -> automation_proof
  -> desired_strategy_position, when proof passes
  -> automation_execution_reconciliation
  -> broker adapter later
  -> fills attributed back to sleeves
```

## State Objects

`automation_strategy_definition` is the versioned registry of strategies. The
first intended families are thesis timing and technical timing. A strategy
version includes a config hash so every desired-position output can be tied to
the exact code/config that produced it.

`automation_trade_permission` is the operator approval. It records symbol,
strategy id/version, status, instrument scope, environment scope, approval
actor/time, expiry, allocation caps, and manual freeze state.

`desired_strategy_position` is append-only strategy intent. It says what
exposure a strategy wants for its sleeve: flat, long, or short, with target
size plus rationale and feature snapshots. It is not an order request.

`automation_strategy_sleeve` separates ownership. Manual exposure uses a
manual sleeve. Each automated strategy permission gets its own strategy sleeve
so attribution, allocation, and manual freeze behavior stay clear even when the
broker reports only one net position.

`automation_proof` freezes the deterministic gate result for a strategy
evaluation: permission, data freshness, session state, risk, capital
allocation, and broker reconciliation inputs. Blocked preflight evaluations are
recorded even when no desired-position row is written.

`automation_strategy_signal_observation` is the forward-only validation anchor
for shadow strategy output. Every emitted desired position gets an observation
with target side, reason codes, feature snapshot, config hash, churn flag, and
future evaluation due date. Later validation fills outcome fields after the
market has moved; the runner never backfills a signal into the past.

`automation_execution_reconciliation` records how a passing desired position
would reconcile against actual broker state. In shadow mode this can stop at
`noop`, `needs_order`, `blocked`, or `reconciled`; paper/live adapters are later
work.

`automation_incident` is the operational safety log for stale broker state,
irreconcilable sleeves, duplicate submission risk, repeated rejects, or any
other condition that should freeze or block automation.

## Invariants

Strategies write desired state only. They do not place orders, mutate broker
state, or bypass risk.

Proof is required before desired exposure can be treated as executable input to
reconciliation. Missing, stale, failed, or under-scored inputs block the path
and must produce concrete blocked reasons.

The existing risk overlay remains an independent hard gate. Automation proof
may include risk output, but it does not replace the risk module.

Manual freeze and the global kill switch override every strategy. A frozen
permission or sleeve may be observed, but it must not create new desired
exposure or executable reconciliation.

Model outputs and LLM outputs are evidence inputs only. Future Kronos-style
forecast signals may feed a strategy feature snapshot after validation, but
they cannot create desired positions or orders directly.

Broker order placement is out of scope for the schema slice. The first broker
write path must be paper-only and explicitly gated by later issues.

## Shadow Strategy Runner

`strategy-runner` is the first automation producer. It seeds deterministic
built-in strategy definitions when missing, then evaluates approved shadow
permissions and writes append-only desired positions only when the target side
or target weight changes.

The initial families are:

- `technical_timing@0.1.0`: pure technical timing from derived chart state.
- `thesis_timing@0.1.0`: bullish actionable thesis plus acceptable chart
  timing.

The runner blocks before changing desired state when permission is missing,
pending, expired, frozen, non-shadow, stale, or technically invalid. Every
emission records the strategy version and exact config hash in the desired
position, proof, feature snapshot, and validation observation.

The runner now evaluates the proof policy before writing desired state. The
policy records permission, kill-switch, data freshness, regular-session, risk,
capital-cap, sleeve, and broker aggregate snapshots. If proof blocks, the
runner writes only an `automation_proof` row with blocked reasons. If proof
passes or warns, the runner may write a desired position and attaches the exact
proof snapshots to that emission.

The runner does not import broker adapters and does not create reconciliation
orders. Broker reconciliation remains read-only aggregate state until the later
paper/live simulator and adapter issues are implemented.
