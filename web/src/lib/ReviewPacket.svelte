<script lang="ts">
  import type {
    AttentionReviewPacket,
    ProposedList,
    ReviewPacketAction,
    ReviewPacketActionPayload,
    ReviewPacketSection,
  } from "./api";

  type Props = {
    packet?: AttentionReviewPacket | null;
    loading?: boolean;
    error?: string | null;
    busy?: boolean;
    status?: string | null;
    onAction?: (
      action: ReviewPacketAction,
      packet: AttentionReviewPacket,
      payload?: ReviewPacketActionPayload,
    ) => void;
  };

  let {
    packet = null as AttentionReviewPacket | null,
    loading = false,
    error = null as string | null,
    busy = false,
    status = null as string | null,
    onAction = (
      _action: ReviewPacketAction,
      _packet: AttentionReviewPacket,
      _payload?: ReviewPacketActionPayload,
    ) => {},
  }: Props = $props();

  let confirmOpen = $state(false);
  let automationConfirmOpen = $state(false);
  let disagreeOpen = $state(false);
  let automationMaxAllocationPct = $state("5");
  let automationMaxNotionalUsd = $state("");
  let selectedWatchlists = $state<Record<string, boolean>>({});
  let lastPacketId = $state<number | null>(null);

  $effect(() => {
    const nextId = packet?.attention.id ?? null;
    if (nextId !== lastPacketId) {
      lastPacketId = nextId;
      confirmOpen = false;
      automationConfirmOpen = false;
      disagreeOpen = false;
      automationMaxAllocationPct = "5";
      automationMaxNotionalUsd = "";
      selectedWatchlists = {};
    }
  });

  function label(value?: string | null): string {
    return value ? value.replace(/_/g, " ") : "unknown";
  }

  function primaryAction(packet: AttentionReviewPacket): ReviewPacketAction {
    return packet.decision?.primary_action ?? packet.allowed_actions[0] ?? {
      id: "open_symbol",
      label: "Open symbol",
      kind: "open_symbol",
      detail: "Inspect context and evidence.",
    };
  }

  function secondaryActions(packet: AttentionReviewPacket): ReviewPacketAction[] {
    const primary = primaryAction(packet);
    const actions = packet.decision?.secondary_actions?.length ? packet.decision.secondary_actions : packet.allowed_actions;
    return actions.filter((action) => action.kind !== primary.kind);
  }

  function proposedLists(packet: AttentionReviewPacket): ProposedList[] {
    return (packet.candidate?.proposed_lists ?? []).filter((item) => Boolean(item.watchlist_id));
  }

  function selectedListIds(): string[] {
    return Object.entries(selectedWatchlists)
      .filter(([, selected]) => selected)
      .map(([id]) => id);
  }

  function setList(id: string | null | undefined, checked: boolean) {
    if (!id) return;
    selectedWatchlists = { ...selectedWatchlists, [id]: checked };
  }

  function universeLine(packet: AttentionReviewPacket): string {
    const status = packet.universe_status;
    if (status?.in_universe) {
      const tier = status.tier ? `T${status.tier}` : "active";
      const theses = status.open_theses ?? 0;
      return `Universe ${tier} · ${theses} open thesis${theses === 1 ? "" : "es"}`;
    }
    if (packet.candidate) return `Not in Universe · proposed T${packet.candidate.proposed_tier ?? 2}`;
    return "Not in Universe";
  }

  function actionTone(action: ReviewPacketAction): string {
    if (action.kind === "candidate_confirm" || action.kind === "decision" || action.kind === "automation_approve") return "primary";
    if (action.kind === "candidate_reject" || action.kind === "attention_dismiss" || action.kind === "automation_disagree") return "danger";
    return "secondary";
  }

  function sectionItems(packet: AttentionReviewPacket, section: ReviewPacketSection): string[] {
    const items = [...(section.items ?? [])];
    if (section.key === "recorded_artifacts") {
      for (const consequence of packet.decision?.consequences ?? []) {
        if (!items.includes(consequence)) items.push(consequence);
      }
    }
    return items;
  }

  function runPrimary(packet: AttentionReviewPacket) {
    const action = primaryAction(packet);
    if (action.kind === "candidate_confirm") {
      if (!confirmOpen) {
        confirmOpen = true;
        return;
      }
      onAction(action, packet, { watchlistIds: selectedListIds() });
      return;
    }
    if (action.kind === "automation_approve") {
      if (!automationConfirmOpen) {
        automationConfirmOpen = true;
        return;
      }
      const allocation = Number(automationMaxAllocationPct);
      const notional = Number(automationMaxNotionalUsd);
      onAction(action, packet, {
        maxAllocationPct: Number.isFinite(allocation) && allocation > 0
          ? allocation > 1 ? allocation / 100 : allocation
          : 0.05,
        maxNotionalUsd: Number.isFinite(notional) && notional > 0 ? notional : null,
      });
      return;
    }
    onAction(action, packet);
  }

  const disagreementReasons = [
    { value: "signal_too_weak", label: "Signal too weak" },
    { value: "valuation_priced", label: "Valuation priced" },
    { value: "data_stale", label: "Data stale" },
    { value: "llm_overreached", label: "LLM overreached" },
    { value: "risk_too_high", label: "Risk too high" },
    { value: "not_my_edge", label: "Not my edge" },
  ];

  function runSecondary(action: ReviewPacketAction, packet: AttentionReviewPacket) {
    if (action.kind === "automation_disagree") {
      disagreeOpen = !disagreeOpen;
      automationConfirmOpen = false;
      return;
    }
    onAction(action, packet);
  }

  function submitDisagreement(action: ReviewPacketAction, packet: AttentionReviewPacket, reason: string) {
    onAction(action, packet, { disagreementReason: reason });
  }
</script>

{#if loading}
  <section class="review-packet">
    <p class="muted">Loading review packet...</p>
  </section>
{:else if error}
  <section class="review-packet error">
    <p>{error}</p>
  </section>
{:else if packet}
  {@const action = primaryAction(packet)}
  {@const lists = proposedLists(packet)}
  <section class={`review-packet intent-${packet.decision?.intent ?? "inspect_symbol"}`} data-testid="review-packet">
    <div class="packet-head">
      <div>
        <span class="kicker">review packet</span>
        <strong>{packet.decision?.headline ?? `${packet.attention.symbol ?? "System"} · ${label(packet.attention.kind)}`}</strong>
      </div>
      <div class="packet-status">
        <span class="badge state-{packet.attention.fsm_state ?? 'ready_for_review'}">{label(packet.attention.fsm_state)}</span>
        <span class="badge">{universeLine(packet)}</span>
      </div>
    </div>

    {#if status}
      <p class="packet-result">{status}</p>
    {/if}

    <div class="decision-panel">
      <div>
        <span class="kicker">what can the human do</span>
        <h3>{action.label}</h3>
        <p>{action.detail}</p>
      </div>
      <button
        type="button"
        class={`primary-action tone-${actionTone(action)}`}
        disabled={busy || Boolean(status)}
        onclick={() => runPrimary(packet)}
      >
        {busy ? "Working..." : action.label}
      </button>
    </div>

    {#if packet.decision?.blockers?.length}
      <div class="blockers">
        {#each packet.decision.blockers as blocker, i (`${blocker}-${i}`)}
          <span>{blocker}</span>
        {/each}
      </div>
    {/if}

    {#if action.kind === "candidate_confirm" && confirmOpen}
      <section class="confirm-panel" data-testid="review-packet-confirm">
        <div>
          <span class="kicker">confirm research kickoff</span>
          <strong>Universe is always included.</strong>
          <p class="muted">Optional watchlists can be added now; leaving all unchecked still starts research for the symbol.</p>
        </div>
        {#if packet.candidate}
          <dl class="candidate-meta">
            <dt>signal</dt><dd>{label(packet.candidate.signal_name)}</dd>
            <dt>rank</dt><dd>{Math.round(packet.candidate.rank_score ?? 0)} · {packet.candidate.rank_bucket ?? "unranked"}</dd>
            <dt>tier</dt><dd>T{packet.candidate.proposed_tier ?? 2}</dd>
          </dl>
          {#if packet.candidate.rank_reasons?.length}
            <ul class="compact-list">
              {#each packet.candidate.rank_reasons.slice(0, 3) as reason, i (`${reason}-${i}`)}
                <li>{reason}</li>
              {/each}
            </ul>
          {/if}
        {/if}
        {#if lists.length}
          <div class="watchlist-picks">
            {#each lists as list (list.watchlist_id)}
              <label>
                <input
                  type="checkbox"
                  disabled={busy || Boolean(status)}
                  checked={Boolean(selectedWatchlists[list.watchlist_id ?? ""])}
                  onchange={(event) => setList(list.watchlist_id, (event.currentTarget as HTMLInputElement).checked)}
                />
                <span>
                  {list.watchlist_name}
                  <small>{list.confidence} · {list.rationale}</small>
                </span>
              </label>
            {/each}
          </div>
        {/if}
        <div class="confirm-actions">
          <button type="button" class="primary-action tone-primary" disabled={busy || Boolean(status)} onclick={() => runPrimary(packet)}>
            {busy ? "Starting..." : "Start research"}
          </button>
          <button type="button" class="secondary-action" disabled={busy || Boolean(status)} onclick={() => (confirmOpen = false)}>Cancel</button>
        </div>
      </section>
    {/if}

    {#if action.kind === "automation_approve" && automationConfirmOpen}
      <section class="confirm-panel automation-confirm" data-testid="review-packet-automation-confirm">
        <div>
          <span class="kicker">confirm bot approval</span>
          <strong>Approve shadow bot-managed trading.</strong>
          <p class="muted">The bot may manage entries and exits for this strategy sleeve after proof and risk gates pass. No live broker order is placed by this approval.</p>
        </div>
        <dl class="candidate-meta">
          <dt>symbol</dt><dd>{packet.attention.symbol ?? "unknown"}</dd>
          <dt>strategy</dt><dd>{action.strategy_id ?? "thesis_timing"}@{action.strategy_version ?? "0.1.0"}</dd>
          <dt>mode</dt><dd>{action.environment_scope ?? "shadow"}</dd>
          <dt>ttl</dt><dd>90 days</dd>
        </dl>
        <div class="automation-fields">
          <label>
            Max allocation
            <span>
              <input bind:value={automationMaxAllocationPct} inputmode="decimal" />
              %
            </span>
          </label>
          <label>
            Max notional
            <input bind:value={automationMaxNotionalUsd} inputmode="decimal" placeholder="optional" />
          </label>
        </div>
        <div class="confirm-actions">
          <button type="button" class="primary-action tone-primary" disabled={busy || Boolean(status)} onclick={() => runPrimary(packet)}>
            {busy ? "Approving..." : "Approve bot trading"}
          </button>
          <button type="button" class="secondary-action" disabled={busy || Boolean(status)} onclick={() => (automationConfirmOpen = false)}>Cancel</button>
        </div>
      </section>
    {/if}

    {#if disagreeOpen}
      {@const disagreeAction = secondaryActions(packet).find((item) => item.kind === "automation_disagree")}
      {#if disagreeAction}
        <section class="confirm-panel disagree-panel" data-testid="review-packet-disagree">
          <div>
            <span class="kicker">reject automation thesis</span>
            <strong>Why disagree?</strong>
            <p class="muted">Records a rejected skip decision and resolves this review item.</p>
          </div>
          <div class="reason-chips">
            {#each disagreementReasons as reason (reason.value)}
              <button
                type="button"
                class="secondary-action tone-danger"
                disabled={busy || Boolean(status)}
                onclick={() => submitDisagreement(disagreeAction, packet, reason.value)}
              >{reason.label}</button>
            {/each}
          </div>
        </section>
      {/if}
    {/if}

    {#if packet.sections.length}
      <div class="packet-grid" data-testid="review-packet-sections">
        {#each packet.sections as section, i (`${section.key}-${i}`)}
          {@const items = sectionItems(packet, section)}
          <article class="packet-section">
            <span>{section.title}</span>
            {#if section.body}
              <p>{section.body}</p>
            {/if}
            {#if items.length}
              <ul>
                {#each items as item, j (`${item}-${j}`)}
                  <li>{item}</li>
                {/each}
              </ul>
            {:else if !section.body}
              <p class="muted">No source-backed details attached.</p>
            {/if}
          </article>
        {/each}
      </div>
    {/if}

    <div class="packet-actions">
      {#each secondaryActions(packet) as action (`${action.id}-${action.kind}`)}
        <button type="button" class={`tone-${actionTone(action)}`} disabled={busy || Boolean(status)} onclick={() => runSecondary(action, packet)}>
          {action.label}
          <small>{action.detail}</small>
        </button>
      {/each}
    </div>
  </section>
{/if}

<style>
  .review-packet {
    border: 1px solid #273246;
    border-left: 3px solid #89b4fa;
    background: #0a0f18;
    border-radius: 4px;
    padding: .65rem;
    display: grid;
    gap: .65rem;
  }
  .review-packet.error {
    border-left-color: #f38ba8;
    color: #f38ba8;
  }
  .intent-promote_to_universe {
    border-left-color: #a6e3a1;
  }
  .intent-resolve_evidence_blocker {
    border-left-color: #f9e2af;
  }
  .intent-review_thesis_change {
    border-left-color: #89b4fa;
  }
  .intent-record_trade_decision {
    border-left-color: #fab387;
  }
  .packet-head,
  .packet-status,
  .decision-panel,
  .confirm-actions,
  .packet-actions {
    display: flex;
    gap: .5rem;
    align-items: start;
    flex-wrap: wrap;
  }
  .packet-head,
  .decision-panel {
    justify-content: space-between;
  }
  .packet-status {
    justify-content: end;
  }
  .kicker,
  .packet-section span {
    display: block;
    color: #89b4fa;
    font-size: .7rem;
    text-transform: uppercase;
    letter-spacing: 0;
  }
  h3 {
    margin: .1rem 0;
    font-size: 1rem;
    line-height: 1.25;
  }
  p,
  ul,
  dl {
    margin: 0;
    color: #a6adc8;
    font-size: .78rem;
    line-height: 1.35;
  }
  ul {
    padding-left: 1rem;
  }
  li {
    margin: .12rem 0;
  }
  .decision-panel,
  .confirm-panel {
    border: 1px solid #1f2733;
    background: #080c13;
    border-radius: 4px;
    padding: .55rem .6rem;
  }
  .confirm-panel {
    display: grid;
    gap: .5rem;
  }
  .candidate-meta {
    display: grid;
    grid-template-columns: max-content 1fr;
    gap: .18rem .55rem;
  }
  .candidate-meta dt {
    color: #7f849c;
  }
  .candidate-meta dd {
    margin: 0;
  }
  .compact-list {
    display: grid;
    gap: .1rem;
  }
  .watchlist-picks {
    display: grid;
    gap: .35rem;
  }
  .watchlist-picks label {
    display: flex;
    gap: .4rem;
    align-items: start;
    color: #cdd6f4;
    font-size: .78rem;
  }
  .watchlist-picks small {
    display: block;
    color: #7f849c;
    line-height: 1.25;
  }
  .automation-confirm {
    border-color: rgba(166, 227, 161, .36);
    background: #09110d;
  }
  .disagree-panel {
    border-color: rgba(243, 139, 168, .36);
    background: #14090d;
  }
  .reason-chips {
    display: flex;
    flex-wrap: wrap;
    gap: .4rem;
  }
  .reason-chips .secondary-action {
    max-width: none;
    min-width: 8.5rem;
    justify-content: center;
    text-align: center;
  }
  .automation-fields {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: .45rem;
  }
  .automation-fields label {
    display: grid;
    gap: .16rem;
    color: #9aa3b8;
    font-size: .72rem;
    text-transform: uppercase;
  }
  .automation-fields label span {
    display: flex;
    align-items: center;
    gap: .25rem;
    color: #7f849c;
    text-transform: none;
  }
  .automation-fields input {
    min-width: 0;
    width: 100%;
    background: #0a0d14;
    color: #cdd6f4;
    border: 1px solid #2a3548;
    border-radius: 4px;
    padding: .25rem .4rem;
    font: inherit;
    text-transform: none;
  }
  .packet-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(210px, 1fr));
    gap: .45rem;
  }
  .packet-section {
    display: grid;
    gap: .25rem;
    border: 1px solid #1f2733;
    background: #0a0f18;
    border-radius: 4px;
    padding: .45rem .5rem;
    min-width: 0;
  }
  .blockers {
    display: flex;
    flex-wrap: wrap;
    gap: .35rem;
  }
  .blockers span,
  .badge {
    border: 1px solid #2a3447;
    border-radius: 999px;
    padding: .12rem .42rem;
    color: #bac2de;
    font-size: .7rem;
    white-space: nowrap;
  }
  .blockers span {
    color: #f9e2af;
    border-color: #5d4d26;
    background: #171205;
  }
  .packet-result {
    border: 1px solid #2f5d3a;
    background: #071409;
    color: #a6e3a1;
    border-radius: 4px;
    padding: .45rem .55rem;
  }
  .primary-action,
  .secondary-action,
  .packet-actions button {
    border: 1px solid #2a3447;
    background: #111827;
    color: #cdd6f4;
    border-radius: 4px;
    padding: .4rem .55rem;
    font: inherit;
    cursor: pointer;
    display: grid;
    gap: .08rem;
    text-align: left;
    max-width: 16rem;
  }
  .primary-action {
    align-self: center;
    min-width: 10rem;
    text-align: center;
    justify-content: center;
  }
  .tone-primary {
    border-color: #3a6b45;
    background: #102416;
    color: #d8f6d5;
  }
  .tone-danger {
    border-color: #6d3042;
    background: #241018;
    color: #ffd6df;
  }
  .primary-action:hover:not(:disabled),
  .secondary-action:hover:not(:disabled),
  .packet-actions button:hover:not(:disabled) {
    border-color: #45567a;
    background: #162033;
  }
  .tone-primary:hover:not(:disabled) {
    border-color: #5a9c66;
    background: #17331e;
  }
  .tone-danger:hover:not(:disabled) {
    border-color: #a6425e;
    background: #321522;
  }
  button:disabled {
    opacity: .55;
    cursor: not-allowed;
  }
  .packet-actions small {
    color: #7f849c;
    font-size: .68rem;
    line-height: 1.2;
  }
  .muted {
    color: #7f849c;
  }
</style>
