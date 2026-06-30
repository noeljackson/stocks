<script lang="ts">
  import {
    fetchAutomationStatus,
    type AutomationPermission,
    type AutomationStatus,
  } from "./api";

  let { symbol }: { symbol: string | null } = $props();

  let status = $state<AutomationStatus | null>(null);
  let loading = $state(false);
  let error = $state<string | null>(null);
  let lastKey = "__init__";

  async function refresh() {
    loading = true;
    error = null;
    try {
      status = await fetchAutomationStatus({ symbol });
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
      status = null;
    } finally {
      loading = false;
    }
  }

  function titleize(value: string | null | undefined): string {
    return (value ?? "")
      .replace(/_/g, " ")
      .replace(/\b\w/g, (char) => char.toUpperCase());
  }

  function money(value: number | null | undefined): string {
    if (value === null || value === undefined || Number.isNaN(value)) return "-";
    return value.toLocaleString(undefined, { style: "currency", currency: "USD", maximumFractionDigits: 0 });
  }

  function pct(value: number | null | undefined): string {
    if (value === null || value === undefined || Number.isNaN(value)) return "-";
    return `${(value * 100).toFixed(1)}%`;
  }

  function shortTs(value: string | null | undefined): string {
    if (!value) return "-";
    return new Date(value).toLocaleString();
  }

  function shortHash(value: string | null | undefined): string {
    if (!value) return "-";
    return value.replace(/^sha256:/, "").slice(0, 10);
  }

  function reasons(row: AutomationPermission): string[] {
    return [
      ...((row.latest_proof?.blocked_reasons ?? []) as string[]),
      ...((row.reconciliation?.blocked_reasons ?? []) as string[]),
    ].filter(Boolean);
  }

  function proofRisk(row: AutomationPermission): string {
    const proof = row.latest_proof;
    if (!proof) return "missing";
    return proof.risk_result?.snapshot?.status
      ?? proof.risk_result?.status
      ?? (proof.risk_result?.veto ? "veto" : proof.risk_result?.warnings?.length ? "warning" : "pass");
  }

  function allocatorStatus(row: AutomationPermission): string {
    const reasons = row.latest_proof?.capital_allocation?.allocator_blocked_reasons ?? [];
    if (!row.latest_proof) return "missing";
    return reasons.length > 0 ? "blocked" : "ok";
  }

  function simOrderCount(row: AutomationPermission): number {
    return row.reconciliation?.order_plan?.orders?.length ?? 0;
  }

  function simFilledCount(row: AutomationPermission): number {
    const orders = row.reconciliation?.order_plan?.orders ?? [];
    return orders.filter((order) => order.status === "filled" || order.status === "partially_filled").length;
  }

  $effect(() => {
    const key = symbol ?? "";
    if (key === lastKey) return;
    lastKey = key;
    status = null;
    void refresh();
  });
</script>

<section class="automation" data-testid="automation-cockpit">
  <header class="auto-top">
    <div>
      <h4>Automation</h4>
      <span>{symbol ? `${symbol} strategy permissions` : "all strategy permissions"}</span>
    </div>
    <div class="top-actions">
      <button type="button" onclick={refresh} disabled={loading}>{loading ? "refreshing" : "refresh"}</button>
      <button type="button" class="danger" disabled title={status?.kill_switch.reason ?? "write endpoint not wired"}>
        kill switch
      </button>
    </div>
  </header>

  {#if error}
    <p class="error-text">{error}</p>
  {/if}

  {#if status}
    <section class="summary-strip" aria-label="Automation summary">
      <span><strong>{status.summary.permissions_total}</strong> permissions</span>
      <span><strong>{status.summary.approved}</strong> approved</span>
      <span><strong>{status.summary.pending}</strong> pending</span>
      <span><strong>{status.summary.frozen}</strong> frozen</span>
      <span><strong>{status.summary.paper_only}</strong> paper</span>
      <span><strong>{status.summary.live_capable}</strong> live-capable</span>
      <span><strong>{status.summary.blocked_strategies}</strong> blocked</span>
      <span><strong>{status.summary.incidents_open}</strong> incidents</span>
    </section>

    <section class="approval-readonly">
      <div>
        <strong>Approval Draft</strong>
        <span class="muted">read-only until permission mutation endpoints exist</span>
      </div>
      <div class="approval-grid">
        <label>
          Symbol
          <input value={symbol ?? ""} placeholder="select symbol" disabled />
        </label>
        <label>
          Strategy
          <select disabled>
            <option>ticker + strategy approval</option>
          </select>
        </label>
        <label>
          TTL
          <input value="90 days" disabled />
        </label>
        <label>
          Allocation
          <input value="operator-set cap" disabled />
        </label>
        <button type="button" disabled>approve</button>
      </div>
    </section>

    {#if status.permissions.length === 0}
      <p class="muted">No automation permissions recorded{symbol ? ` for ${symbol}` : ""}.</p>
    {:else}
      <div class="permission-list">
        {#each status.permissions as row (row.permission_id)}
          {@const blocked = reasons(row)}
          <article class="permission-row status-{row.derived_status}">
            <div class="permission-head">
              <div>
                <button type="button" class="symbol-chip" disabled>{row.symbol}</button>
                <strong>{row.strategy_display_name}</strong>
                <span class="muted">{row.strategy_id}@{row.strategy_version}</span>
              </div>
              <div class="badges">
                <span class="badge status">{titleize(row.derived_status)}</span>
                <span class="badge">{titleize(row.environment_scope)}</span>
                <span class="badge">{titleize(row.instrument_scope)}</span>
              </div>
            </div>

            <div class="permission-actions">
              <button type="button" disabled title="permission mutation endpoint not wired">freeze</button>
              <button type="button" disabled title="permission mutation endpoint not wired">unfreeze</button>
              <span class="muted">
                approved {row.approved_at ? shortTs(row.approved_at) : "-"}
                {#if row.expires_at} · expires {shortTs(row.expires_at)}{/if}
              </span>
            </div>

            {#if row.manual_freeze || row.freeze_reason}
              <p class="freeze-note">
                frozen{row.freeze_reason ? `: ${row.freeze_reason}` : ""}
              </p>
            {/if}

            <div class="state-grid">
              <section>
                <h5>Limits</h5>
                <dl>
                  <dt>allocation</dt><dd>{pct(row.max_allocation_pct)}</dd>
                  <dt>notional</dt><dd>{money(row.max_notional_usd)}</dd>
                  <dt>quantity</dt><dd>{row.max_quantity ?? "-"}</dd>
                </dl>
              </section>

              <section>
                <h5>Sleeve</h5>
                <dl>
                  <dt>status</dt><dd>{titleize(row.sleeve?.status ?? "missing")}</dd>
                  <dt>side</dt><dd>{titleize(row.sleeve?.current_side ?? "flat")}</dd>
                  <dt>notional</dt><dd>{money(row.sleeve?.current_notional_usd)}</dd>
                  <dt>allocated</dt><dd>{money(row.sleeve?.allocated_notional_usd)}</dd>
                  <dt>uPnL</dt><dd>{money(row.sleeve?.unrealized_pnl)}</dd>
                </dl>
              </section>

              <section>
                <h5>Desired</h5>
                <dl>
                  <dt>target</dt><dd>{titleize(row.desired_position?.target_side ?? "none")}</dd>
                  <dt>notional</dt><dd>{money(row.desired_position?.target_notional_usd)}</dd>
                  <dt>weight</dt><dd>{pct(row.desired_position?.target_weight_pct)}</dd>
                  <dt>config</dt><dd>{shortHash(row.desired_position?.strategy_config_hash ?? row.latest_proof?.strategy_config_hash)}</dd>
                </dl>
              </section>

              <section>
                <h5>Broker Net</h5>
                <dl>
                  <dt>positions</dt><dd>{row.broker_position?.open_positions ?? 0}</dd>
                  <dt>broker</dt><dd>{row.broker_position?.broker_positions ?? 0}</dd>
                  <dt>delta</dt><dd>{money(row.broker_position?.delta_notional)}</dd>
                </dl>
              </section>

              <section>
                <h5>Simulator</h5>
                <dl>
                  <dt>status</dt><dd>{titleize(row.reconciliation?.status ?? "missing")}</dd>
                  <dt>orders</dt><dd>{simOrderCount(row)}</dd>
                  <dt>fills</dt><dd>{simFilledCount(row)}</dd>
                  <dt>delta</dt><dd>{money(row.reconciliation?.delta_snapshot?.notional_delta_usd)}</dd>
                  <dt>rPnL</dt><dd>{money(row.reconciliation?.delta_snapshot?.realized_pnl_delta)}</dd>
                </dl>
              </section>

              <section>
                <h5>Proof</h5>
                <dl>
                  <dt>result</dt><dd>{titleize(row.latest_proof?.result ?? "missing")}</dd>
                  <dt>data</dt><dd>{titleize(row.latest_proof?.data_freshness?.status ?? "missing")}</dd>
                  <dt>session</dt><dd>{titleize(row.latest_proof?.session_state?.label ?? "missing")}</dd>
                  <dt>risk</dt><dd>{titleize(proofRisk(row))}</dd>
                  <dt>allocator</dt><dd>{titleize(allocatorStatus(row))}</dd>
                  <dt>target</dt><dd>{pct(row.latest_proof?.capital_allocation?.target_weight_pct)}</dd>
                </dl>
              </section>
            </div>

            <div class="proof-line">
              <span class="badge proof-{row.latest_proof?.result ?? "missing"}">
                proof {titleize(row.latest_proof?.result ?? "missing")}
              </span>
              <span class="badge rec-{row.reconciliation?.status ?? "missing"}">
                reconciliation {titleize(row.reconciliation?.status ?? "missing")}
              </span>
              {#if row.desired_position?.rationale}
                <span class="muted">{row.desired_position.rationale}</span>
              {/if}
            </div>

            {#if blocked.length > 0}
              <div class="blocked-reasons">
                {#each [...new Set(blocked)] as reason}
                  <span>{reason}</span>
                {/each}
              </div>
            {/if}

            {#if row.incidents && row.incidents.length > 0}
              <ul class="incident-list">
                {#each row.incidents as incident (incident.incident_id)}
                  <li>
                    <span class="badge incident-{incident.severity}">{incident.severity}</span>
                    <strong>{incident.title}</strong>
                    <span class="muted">{titleize(incident.kind)} · {shortTs(incident.created_at)}</span>
                  </li>
                {/each}
              </ul>
            {/if}
          </article>
        {/each}
      </div>
    {/if}
  {:else if loading}
    <p class="muted">Loading automation state...</p>
  {:else}
    <p class="muted">Automation state unavailable.</p>
  {/if}
</section>

<style>
  .automation {
    display: flex;
    flex-direction: column;
    gap: .65rem;
  }

  .auto-top,
  .top-actions,
  .summary-strip,
  .approval-grid,
  .permission-head,
  .permission-head > div,
  .permission-actions,
  .badges,
  .proof-line,
  .blocked-reasons,
  .incident-list li {
    display: flex;
    align-items: center;
    gap: .4rem;
    flex-wrap: wrap;
  }

  .auto-top,
  .permission-head {
    justify-content: space-between;
  }

  h4,
  h5,
  p {
    margin: 0;
  }

  h4 {
    font-size: .95rem;
  }

  h5 {
    color: #cdd6f4;
    font-size: .75rem;
    margin-bottom: .25rem;
    text-transform: uppercase;
  }

  .auto-top span,
  .muted {
    color: #7f8aa3;
    font-size: .76rem;
  }

  button,
  input,
  select {
    background: #1b2230;
    color: #cdd6f4;
    border: 1px solid #2a3548;
    border-radius: 4px;
    padding: .18rem .5rem;
    font: inherit;
  }

  button {
    cursor: pointer;
  }

  button:disabled,
  input:disabled,
  select:disabled {
    opacity: .65;
    cursor: default;
  }

  .danger {
    border-color: rgba(243,139,168,.45);
    color: #f38ba8;
  }

  .summary-strip {
    background: #0a0d14;
    border: 1px solid #1f2733;
    border-radius: 4px;
    padding: .45rem .55rem;
  }

  .summary-strip span {
    color: #a6adc8;
    font-size: .76rem;
  }

  .summary-strip strong {
    color: #cdd6f4;
  }

  .approval-readonly,
  .permission-row {
    border: 1px solid #1f2733;
    background: #0c1019;
    border-radius: 4px;
    padding: .55rem .65rem;
  }

  .approval-readonly {
    display: flex;
    flex-direction: column;
    gap: .45rem;
  }

  .approval-grid label {
    display: flex;
    align-items: center;
    gap: .3rem;
    color: #7f8aa3;
    font-size: .75rem;
  }

  .permission-list {
    display: flex;
    flex-direction: column;
    gap: .55rem;
  }

  .permission-row {
    display: flex;
    flex-direction: column;
    gap: .45rem;
    border-left: 3px solid #6c7693;
  }

  .permission-row.status-frozen,
  .permission-row.status-expired {
    border-left-color: #f9e2af;
  }

  .permission-row.status-approved {
    border-left-color: #a6e3a1;
  }

  .symbol-chip {
    font-weight: 700;
  }

  .badge {
    border: 1px solid #2a3548;
    background: #111827;
    color: #bac2de;
    border-radius: 3px;
    padding: .08rem .35rem;
    font-size: .72rem;
  }

  .badge.status,
  .proof-blocked,
  .rec-blocked {
    border-color: rgba(249,226,175,.45);
    color: #f9e2af;
  }

  .proof-warning {
    border-color: rgba(250,179,135,.45);
    color: #fab387;
  }

  .proof-passed,
  .rec-reconciled,
  .rec-noop {
    border-color: rgba(166,227,161,.45);
    color: #a6e3a1;
  }

  .freeze-note {
    color: #f9e2af;
    font-size: .78rem;
  }

  .state-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(150px, 1fr));
    gap: .45rem;
  }

  .state-grid section {
    background: #0a0d14;
    border: 1px solid #1f2733;
    border-radius: 4px;
    padding: .45rem .5rem;
  }

  dl {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: .2rem .45rem;
    margin: 0;
    font-size: .76rem;
  }

  dt {
    color: #7f8aa3;
  }

  dd {
    margin: 0;
    color: #cdd6f4;
  }

  .blocked-reasons span {
    background: rgba(249,226,175,.08);
    border: 1px solid rgba(249,226,175,.28);
    border-radius: 3px;
    color: #f9e2af;
    font-size: .74rem;
    padding: .08rem .35rem;
  }

  .incident-list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: .2rem;
  }

  .incident-warning {
    border-color: rgba(249,226,175,.45);
    color: #f9e2af;
  }

  .incident-critical {
    border-color: rgba(243,139,168,.45);
    color: #f38ba8;
  }

  .error-text {
    color: #f38ba8;
    font-size: .8rem;
  }
</style>
