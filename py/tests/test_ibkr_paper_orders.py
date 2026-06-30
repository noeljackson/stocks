import datetime as dt

from stocks.ibkr_paper_orders import (
    BrokerFault,
    BrokerSubmission,
    FakePaperBroker,
    PaperOrderConfig,
    PaperOrderContext,
    plan_paper_order_submission,
    submit_with_broker,
)


def _ctx(**overrides):
    now = dt.datetime(2026, 6, 30, 14, 30, tzinfo=dt.UTC)
    base = PaperOrderContext(
        reconciliation_id="11111111-1111-1111-1111-111111111111",
        desired_position_id="22222222-2222-2222-2222-222222222222",
        proof_id="33333333-3333-3333-3333-333333333333",
        sleeve_id="44444444-4444-4444-4444-444444444444",
        symbol="NVDA",
        environment_scope="paper",
        reconciliation_status="needs_order",
        proof_result="passed",
        blocked_reasons=[],
        existing_order_count=0,
        position_snapshot_at=now,
        now=now,
        order_plan={
            "broker": "simulator",
            "account": "digital",
            "orders": [
                {
                    "client_order_id": "22222222-2222-2222-2222-222222222222:0",
                    "type": "market",
                    "action": "buy",
                    "position_side": "long",
                    "leg_kind": "enter",
                    "quantity": 10,
                    "price": 100,
                    "bracket": {
                        "stop_price": 92,
                        "take_profit_price": 116,
                    },
                }
            ],
        },
    )
    return base.replace(**overrides)


def _enabled_config(**overrides):
    config = PaperOrderConfig(
        db_enabled=True,
        env_enabled=True,
        broker_account="DU1234567",
    )
    return config.replace(**overrides)


def test_default_disabled_config_blocks_without_calling_broker():
    broker = FakePaperBroker()

    outcome = submit_with_broker(_ctx(), PaperOrderConfig(), broker)

    assert outcome.status == "blocked"
    assert "paper_order_adapter_disabled" in outcome.blocked_reasons
    assert broker.calls == 0
    assert outcome.orders == []


def test_live_scopes_and_non_paper_accounts_are_impossible():
    live_scope = plan_paper_order_submission(
        _ctx(environment_scope="canary_live"),
        _enabled_config(),
    )
    real_account = plan_paper_order_submission(
        _ctx(),
        _enabled_config(broker_account="U1234567"),
    )

    assert live_scope.status == "blocked"
    assert "environment_not_paper" in live_scope.blocked_reasons
    assert real_account.status == "blocked"
    assert "paper_account_required" in real_account.blocked_reasons


def test_passing_reconciliation_maps_parent_and_bracket_children():
    planned = plan_paper_order_submission(_ctx(), _enabled_config())

    assert planned.status == "ready"
    assert [order.order_role for order in planned.orders] == [
        "parent",
        "take_profit",
        "stop_loss",
    ]
    assert planned.orders[0].parent_client_order_id is None
    assert planned.orders[1].parent_client_order_id == planned.orders[0].client_order_id
    assert planned.orders[2].parent_client_order_id == planned.orders[0].client_order_id
    assert planned.orders[1].limit_price == 116
    assert planned.orders[2].stop_price == 92
    assert planned.orders[2].transmit is True


def test_duplicate_and_stale_snapshot_block_before_broker_submit():
    old_snapshot = _ctx().now - dt.timedelta(minutes=10)
    duplicate_broker = FakePaperBroker()
    stale_broker = FakePaperBroker()

    duplicate = submit_with_broker(
        _ctx(existing_order_count=1),
        _enabled_config(),
        duplicate_broker,
    )
    stale = submit_with_broker(
        _ctx(position_snapshot_at=old_snapshot),
        _enabled_config(max_position_snapshot_age_seconds=120),
        stale_broker,
    )

    assert duplicate.status == "blocked"
    assert "duplicate_order_attempt" in duplicate.blocked_reasons
    assert duplicate_broker.calls == 0
    assert stale.status == "blocked"
    assert stale.incident is not None
    assert stale.incident.kind == "stale_broker_position_snapshot"
    assert stale_broker.calls == 0


def test_broker_disconnect_and_reject_create_incidents():
    disconnected = submit_with_broker(
        _ctx(),
        _enabled_config(),
        FakePaperBroker(fault=BrokerFault.DISCONNECT),
    )
    rejected = submit_with_broker(
        _ctx(),
        _enabled_config(),
        FakePaperBroker(fault=BrokerFault.REJECT),
    )

    assert disconnected.status == "incident"
    assert disconnected.incident is not None
    assert disconnected.incident.kind == "paper_broker_disconnect"
    assert rejected.status == "incident"
    assert rejected.incident is not None
    assert rejected.incident.kind == "paper_order_rejected"
    assert any(event.event_kind == "rejected" for event in rejected.events)


def test_partial_fill_stays_submitted_and_records_fill_event():
    outcome = submit_with_broker(
        _ctx(),
        _enabled_config(),
        FakePaperBroker(submission=BrokerSubmission.PARTIAL_FILL),
    )

    assert outcome.status == "submitted"
    assert outcome.incident is not None
    assert outcome.incident.kind == "paper_order_partial_fill"
    assert any(event.event_kind == "partial_fill" for event in outcome.events)
