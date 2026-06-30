<script lang="ts">
  import {
    fetchAutomationStatus,
    fetchAutomationTimeline,
    type AutomationPermission,
    type AutomationStatus,
    type AutomationTimeline,
    type AutomationTimelineEvent,
  } from "./api";

  let {
    symbol = null,
    onOpenWorkspace = (_symbol: string) => {},
    onFilterSymbol = (_symbol: string | null) => {},
    onBack = () => {},
  } = $props<{
    symbol?: string | null;
    onOpenWorkspace?: (symbol: string) => void;
    onFilterSymbol?: (symbol: string | null) => void;
    onBack?: () => void;
  }>();

  type NextAction = "enter" | "exit" | "resize" | "hold" | "blocked" | "no_signal";

  let status = $state<AutomationStatus | null>(null);
  let timeline = $state<AutomationTimeline | null>(null);
  let loading = $state(false);
  let timelineLoading = $state(false);
  let error = $state<string | null>(null);
  let timelineError = $state<string | null>(null);
  let selectedPermissionId = $state<string | null>(null);
  let lastStatusKey = "__init__";
  let lastTimelineKey = "__init__";

  const selectedRow = $derived.by<AutomationPermission | null>(() => {
    if (!status?.permissions.length) return null;
    return status.permissions.find((row) => row.permission_id === selectedPermissionId) ?? status.permissions[0] ?? null;
  });

  const visiblePermissions = $derived.by<AutomationPermission[]>(() => status?.permissions ?? []);

  const summary = $derived.by(() => {
    const permissions = visiblePermissions.length;
    const approved = visiblePermissions.filter((row) => row.permission_status === "approved").length;
    const frozen = visiblePermissions.filter((row) => row.manual_freeze || row.derived_status === "frozen").length;
    const blocked = visiblePermissions.filter((row) => nextAction(row) === "blocked").length;
    const enters = visiblePermissions.filter((row) => nextAction(row) === "enter").length;
    const exits = visiblePermissions.filter((row) => nextAction(row) === "exit").length;
    const resizes = visiblePermissions.filter((row) => nextAction(row) === "resize").length;
    const paperOrders = visiblePermissions.reduce((sum, row) => sum + paperOrderTotal(row), 0);
    return { permissions, approved, frozen, blocked, enters, exits, resizes, paperOrders };
  });

  function selectedTimelineFilters() {
    const row = selectedRow;
    if (row) return { symbol: row.symbol, strategyId: row.strategy_id };
    return { symbol, strategyId: null };
  }

  async function refreshStatus() {
    loading = true;
    error = null;
    try {
      status = await fetchAutomationStatus({ symbol });
      if (!status.permissions.some((row) => row.permission_id === selectedPermissionId)) {
        selectedPermissionId = status.permissions[0]?.permission_id ?? null;
      }
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
      status = null;
      selectedPermissionId = null;
    } finally {
      loading = false;
    }
  }

  async function refreshTimeline(force = false) {
    const filters = selectedTimelineFilters();
    const key = `${filters.symbol ?? ""}:${filters.strategyId ?? ""}`;
    if (!force && key === lastTimelineKey) return;
    lastTimelineKey = key;
    timelineLoading = true;
    timelineError = null;
    try {
      timeline = await fetchAutomationTimeline({
        symbol: filters.symbol,
        strategyId: filters.strategyId,
        limit: 120,
      });
    } catch (e) {
      timelineError = e instanceof Error ? e.message : String(e);
      timeline = null;
    } finally {
      timelineLoading = false;
    }
  }

  async function refreshAll() {
    await refreshStatus();
    await refreshTimeline(true);
  }

  function titleize(value: string | null | undefined): string {
    const text = value ?? "";
    if (!text) return "-";
    return text
      .replace(/_/g, " ")
      .replace(/\b\w/g, (char) => char.toUpperCase());
  }

  function blockerLabel(value: string | null | undefined): string {
    const key = (value ?? "").trim().toLowerCase();
    if (key === "approval_missing") return "Stage Promotion Approval Needed";
    return titleize(value);
  }

  function readinessApprovalText(row: AutomationPermission): string {
    const target = row.readiness?.target_stage ? titleize(row.readiness.target_stage) : "Target Stage";
    if (row.readiness?.approval_valid) return `Valid for ${target} promotion`;
    if (row.readiness?.approval_required) return `Needed for ${target} promotion`;
    return "-";
  }

  function money(value: number | null | undefined): string {
    if (value === null || value === undefined || Number.isNaN(value)) return "-";
    return value.toLocaleString(undefined, { style: "currency", currency: "USD", maximumFractionDigits: 0 });
  }

  function numberText(value: number | null | undefined): string {
    if (value === null || value === undefined || Number.isNaN(value)) return "-";
    return value.toLocaleString(undefined, { maximumFractionDigits: 2 });
  }

  function pct(value: number | null | undefined): string {
    if (value === null || value === undefined || Number.isNaN(value)) return "-";
    return `${(value * 100).toFixed(1)}%`;
  }

  function signedPct(value: number | null | undefined): string {
    if (value === null || value === undefined || Number.isNaN(value)) return "-";
    const sign = value > 0 ? "+" : "";
    return `${sign}${(value * 100).toFixed(1)}%`;
  }

  function shortTs(value: string | null | undefined): string {
    if (!value) return "-";
    return new Date(value).toLocaleString();
  }

  function shortId(value: string | null | undefined): string {
    if (!value) return "-";
    return value.slice(0, 8);
  }

  function shortHash(value: string | null | undefined): string {
    if (!value) return "-";
    return value.replace(/^sha256:/, "").slice(0, 12);
  }

  function unique(values: Array<string | null | undefined>): string[] {
    return [...new Set(values.filter((value): value is string => Boolean(value)))];
  }

  function blockers(row: AutomationPermission): string[] {
    return unique([
      ...((row.latest_proof?.blocked_reasons ?? []) as string[]),
      ...((row.readiness?.blockers ?? []) as string[]),
      ...((row.reconciliation?.blocked_reasons ?? []) as string[]),
      ...((row.latest_proof?.data_freshness?.market_readiness?.blocked_reasons ?? []) as string[]),
    ]);
  }

  function paperOrderTotal(row: AutomationPermission): number {
    return row.paper_orders?.orders_total ?? 0;
  }

  function paperOpenCount(row: AutomationPermission): number {
    return (row.paper_orders?.submitted ?? 0) + (row.paper_orders?.partially_filled ?? 0);
  }

  function sameSide(row: AutomationPermission): boolean {
    const current = row.sleeve?.current_side ?? "flat";
    const target = row.desired_position?.target_side ?? "flat";
    return current === target;
  }

  function notionalDiff(row: AutomationPermission): number {
    const current = row.sleeve?.current_notional_usd ?? 0;
    const target = row.desired_position?.target_notional_usd ?? 0;
    return target - current;
  }

  function nextAction(row: AutomationPermission): NextAction {
    if (
      row.manual_freeze
      || ["frozen", "expired", "revoked", "blocked"].includes(row.derived_status)
      || row.latest_proof?.result === "blocked"
      || row.reconciliation?.status === "blocked"
      || blockers(row).length > 0
    ) return "blocked";
    const target = row.desired_position?.target_side ?? "flat";
    const current = row.sleeve?.current_side ?? "flat";
    if (!row.desired_position) return "no_signal";
    if (current === "flat" && target !== "flat") return "enter";
    if (current !== "flat" && target === "flat") return "exit";
    if (!sameSide(row)) return "resize";
    const currentNotional = row.sleeve?.current_notional_usd ?? 0;
    const delta = Math.abs(notionalDiff(row));
    if (delta > Math.max(1, Math.abs(currentNotional) * 0.01)) return "resize";
    return "hold";
  }

  function actionText(action: NextAction): string {
    if (action === "no_signal") return "No Signal";
    return titleize(action);
  }

  function actionDetail(row: AutomationPermission): string {
    const action = nextAction(row);
    if (action === "blocked") return blockers(row).map(blockerLabel).join(", ") || titleize(row.derived_status);
    if (action === "enter") return `Open ${titleize(row.desired_position?.target_side)} sleeve exposure.`;
    if (action === "exit") return "Flatten existing sleeve exposure.";
    if (action === "resize") return `Adjust by ${money(notionalDiff(row))}.`;
    if (action === "hold") return "Target and sleeve are already aligned.";
    return "Strategy has not emitted a desired position.";
  }

  function rowScore(row: AutomationPermission): number {
    const action = nextAction(row);
    const actionWeight: Record<NextAction, number> = {
      enter: 0,
      resize: 1,
      exit: 2,
      blocked: 3,
      no_signal: 4,
      hold: 5,
    };
    return actionWeight[action];
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

  function marketStatus(row: AutomationPermission): string {
    if (!row.latest_proof) return "missing";
    return row.latest_proof.data_freshness?.market_readiness_status
      ?? row.latest_proof.data_freshness?.market_readiness?.status
      ?? "missing";
  }

  function simOrderCount(row: AutomationPermission): number {
    return row.reconciliation?.order_plan?.orders?.length ?? 0;
  }

  function simFilledCount(row: AutomationPermission): number {
    const orders = row.reconciliation?.order_plan?.orders ?? [];
    return orders.filter((order) => order.status === "filled" || order.status === "partially_filled").length;
  }

  function jsonPreview(value: unknown): string {
    if (value === null || value === undefined) return "{}";
    return JSON.stringify(value, null, 2);
  }

  function hasObjectValue(value: unknown): boolean {
    return Boolean(value && typeof value === "object" && Object.keys(value as Record<string, unknown>).length > 0);
  }

  function eventIds(event: AutomationTimelineEvent): string[] {
    return unique([
      event.symbol ? `symbol ${event.symbol}` : null,
      event.strategy_id ? `strategy ${event.strategy_id}` : null,
      event.permission_id ? `permission ${shortId(event.permission_id)}` : null,
      event.desired_position_id ? `desired ${shortId(event.desired_position_id)}` : null,
      event.proof_id ? `proof ${shortId(event.proof_id)}` : null,
      event.reconciliation_id ? `recon ${shortId(event.reconciliation_id)}` : null,
    ]);
  }

  $effect(() => {
    const key = symbol ?? "";
    if (key === lastStatusKey) return;
    lastStatusKey = key;
    status = null;
    timeline = null;
    selectedPermissionId = null;
    lastTimelineKey = "__reset__";
    void refreshStatus();
  });

  $effect(() => {
    if (!status) return;
    void refreshTimeline();
  });
</script>

<section class="auto-page" data-testid="autonomous-cockpit">
  <header class="page-head">
    <div>
      <span class="eyebrow">Automation</span>
      <h1>Autonomous Trading</h1>
      <p>{symbol ? `${symbol} permissioned strategy control plane` : "Permissioned strategy control plane"}</p>
    </div>
    <div class="head-actions">
      {#if symbol}
        <button type="button" class="chip-button" onclick={() => onFilterSymbol(null)}>Clear {symbol}</button>
      {/if}
      {#if selectedRow}
        <button type="button" onclick={() => onOpenWorkspace(selectedRow.symbol)}>Workspace</button>
      {/if}
      <button type="button" onclick={refreshAll} disabled={loading || timelineLoading}>
        {loading || timelineLoading ? "Refreshing" : "Refresh"}
      </button>
      <button type="button" onclick={onBack}>Back</button>
    </div>
  </header>

  {#if error}
    <p class="error-text">{error}</p>
  {/if}

  {#if status}
    <section class="summary-strip" aria-label="Autonomous trading summary">
      <div><span>permissions</span><strong>{summary.permissions}</strong></div>
      <div><span>approved</span><strong>{summary.approved}</strong></div>
      <div><span>blocked</span><strong>{summary.blocked}</strong></div>
      <div><span>frozen</span><strong>{summary.frozen}</strong></div>
      <div><span>enter</span><strong>{summary.enters}</strong></div>
      <div><span>exit</span><strong>{summary.exits}</strong></div>
      <div><span>resize</span><strong>{summary.resizes}</strong></div>
      <div><span>paper orders</span><strong>{summary.paperOrders}</strong></div>
      <div><span>kill switch</span><strong>{status.kill_switch.enabled ? "on" : "off"}</strong></div>
      <div><span>adapter</span><strong>{status.paper_order_adapter?.enabled ? "paper on" : "paper off"}</strong></div>
    </section>

    <div class="page-grid">
      <section class="decision-board" aria-label="Strategy decisions">
        <div class="section-head">
          <div>
            <h2>Decision Board</h2>
            <p>What the bot would do next for every approved ticker/strategy pair.</p>
          </div>
          <span>{shortTs(status.as_of)}</span>
        </div>

        {#if visiblePermissions.length === 0}
          <div class="empty-state">
            <strong>No automation permissions</strong>
            <span>{symbol ? `No permissioned strategies exist for ${symbol}.` : "No tickers have been manually approved for strategy automation yet."}</span>
          </div>
        {:else}
          <div class="permission-table">
            {#each [...visiblePermissions].sort((a, b) => rowScore(a) - rowScore(b)) as row (row.permission_id)}
              {@const action = nextAction(row)}
              {@const rowBlockers = blockers(row)}
              <button
                type="button"
                class:active={selectedRow?.permission_id === row.permission_id}
                class="permission-row action-{action}"
                onclick={() => (selectedPermissionId = row.permission_id)}
              >
                <span class="ticker">
                  <strong>{row.symbol}</strong>
                  <small>{row.strategy_display_name}</small>
                </span>
                <span class="action-cell">
                  <span class="badge action">{actionText(action)}</span>
                  <small>{actionDetail(row)}</small>
                </span>
                <span class="position-cell">
                  <span>{titleize(row.sleeve?.current_side ?? "flat")} → {titleize(row.desired_position?.target_side ?? "none")}</span>
                  <small>{money(row.sleeve?.current_notional_usd)} → {money(row.desired_position?.target_notional_usd)}</small>
                </span>
                <span class="gate-cell">
                  <span class="badge proof-{row.latest_proof?.result ?? "missing"}">proof {titleize(row.latest_proof?.result ?? "missing")}</span>
                  <span class="badge rec-{row.reconciliation?.status ?? "missing"}">recon {titleize(row.reconciliation?.status ?? "missing")}</span>
                  <span class="badge readiness-{row.readiness?.status ?? "missing"}">readiness {titleize(row.readiness?.status ?? "missing")}</span>
                </span>
                <span class="reason-cell">
                  {#if row.desired_position?.rationale}
                    {row.desired_position.rationale}
                  {:else if rowBlockers.length > 0}
                    {rowBlockers.map(blockerLabel).join(", ")}
                  {:else}
                    {titleize(row.derived_status)}
                  {/if}
                </span>
              </button>
            {/each}
          </div>
        {/if}
      </section>

      <aside class="detail-column">
        {#if selectedRow}
          {@const row = selectedRow}
          {@const rowBlockers = blockers(row)}
          {@const action = nextAction(row)}
          <section class="selected-panel">
            <div class="selected-head">
              <div>
                <span class="eyebrow">{row.symbol}</span>
                <h2>{row.strategy_display_name}</h2>
                <p>{row.strategy_id}@{row.strategy_version} · {titleize(row.environment_scope)} · {titleize(row.instrument_scope)}</p>
              </div>
              <span class="badge action action-{action}">{actionText(action)}</span>
            </div>

            <div class="callout">
              <strong>{actionDetail(row)}</strong>
              <span>
                {#if row.desired_position?.rationale}
                  {row.desired_position.rationale}
                {:else}
                  Latest strategy signal has no rationale attached.
                {/if}
              </span>
            </div>

            {#if rowBlockers.length > 0}
              <div class="blocker-strip">
                {#each rowBlockers as reason}
                  <span>{blockerLabel(reason)}</span>
                {/each}
              </div>
            {/if}

            <div class="metric-grid">
              <section>
                <h3>Current Sleeve</h3>
                <dl>
                  <dt>status</dt><dd>{titleize(row.sleeve?.status ?? "missing")}</dd>
                  <dt>side</dt><dd>{titleize(row.sleeve?.current_side ?? "flat")}</dd>
                  <dt>quantity</dt><dd>{numberText(row.sleeve?.current_quantity)}</dd>
                  <dt>notional</dt><dd>{money(row.sleeve?.current_notional_usd)}</dd>
                  <dt>allocated</dt><dd>{money(row.sleeve?.allocated_notional_usd)}</dd>
                  <dt>uPnL</dt><dd>{money(row.sleeve?.unrealized_pnl)}</dd>
                </dl>
              </section>

              <section>
                <h3>Desired Target</h3>
                <dl>
                  <dt>side</dt><dd>{titleize(row.desired_position?.target_side ?? "none")}</dd>
                  <dt>quantity</dt><dd>{numberText(row.desired_position?.target_quantity)}</dd>
                  <dt>notional</dt><dd>{money(row.desired_position?.target_notional_usd)}</dd>
                  <dt>weight</dt><dd>{pct(row.desired_position?.target_weight_pct)}</dd>
                  <dt>delta</dt><dd>{money(notionalDiff(row))}</dd>
                  <dt>config</dt><dd>{shortHash(row.desired_position?.strategy_config_hash ?? row.latest_proof?.strategy_config_hash)}</dd>
                  <dt>emitted</dt><dd>{shortTs(row.desired_position?.emitted_at)}</dd>
                </dl>
              </section>

              <section>
                <h3>Proof Gates</h3>
                <dl>
                  <dt>result</dt><dd>{titleize(row.latest_proof?.result ?? "missing")}</dd>
                  <dt>data</dt><dd>{titleize(row.latest_proof?.data_freshness?.status ?? "missing")}</dd>
                  <dt>market</dt><dd>{titleize(marketStatus(row))}</dd>
                  <dt>session</dt><dd>{titleize(row.latest_proof?.session_state?.label ?? "missing")}</dd>
                  <dt>risk</dt><dd>{titleize(proofRisk(row))}</dd>
                  <dt>allocator</dt><dd>{titleize(allocatorStatus(row))}</dd>
                </dl>
              </section>

              <section>
                <h3>Execution</h3>
                <dl>
                  <dt>recon</dt><dd>{titleize(row.reconciliation?.status ?? "missing")}</dd>
                  <dt>sim orders</dt><dd>{simOrderCount(row)}</dd>
                  <dt>sim fills</dt><dd>{simFilledCount(row)}</dd>
                  <dt>paper total</dt><dd>{paperOrderTotal(row)}</dd>
                  <dt>paper open</dt><dd>{paperOpenCount(row)}</dd>
                  <dt>broker delta</dt><dd>{money(row.broker_position?.delta_notional)}</dd>
                </dl>
              </section>

              <section>
                <h3>Readiness</h3>
                <dl>
                  <dt>stage</dt><dd>{titleize(row.readiness?.lifecycle_stage ?? row.strategy_status)}</dd>
                  <dt>target</dt><dd>{titleize(row.readiness?.target_stage ?? "none")}</dd>
                  <dt>status</dt><dd>{titleize(row.readiness?.status ?? "missing")}</dd>
                  <dt>score</dt><dd>{pct(row.readiness?.readiness_score)}</dd>
                  <dt>stage approval</dt><dd>{readinessApprovalText(row)}</dd>
                  <dt>lookback</dt><dd>{row.readiness?.lookback_days ?? "-"}d</dd>
                </dl>
              </section>

              <section>
                <h3>Validation</h3>
                <dl>
                  <dt>observations</dt><dd>{row.readiness?.metrics?.observations_total ?? "-"}</dd>
                  <dt>outcomes</dt><dd>{row.readiness?.metrics?.outcomes_scored ?? "-"}</dd>
                  <dt>proof pass</dt><dd>{pct(row.readiness?.metrics?.proof_pass_rate)}</dd>
                  <dt>fill quality</dt><dd>{pct(row.readiness?.metrics?.paper_fill_quality_rate)}</dd>
                  <dt>excess return</dt><dd>{signedPct(row.readiness?.metrics?.baseline_excess_return_pct)}</dd>
                  <dt>incidents</dt><dd>{row.readiness?.metrics?.open_critical_incidents ?? 0} critical</dd>
                </dl>
              </section>
            </div>

            <div class="detail-list">
              <section>
                <h3>Limits And Approval</h3>
                <dl class="wide-dl">
                  <dt>permission</dt><dd>{shortId(row.permission_id)} · {titleize(row.permission_status)} · {titleize(row.derived_status)}</dd>
                  <dt>approved</dt><dd>{row.approved_by ?? "-"} · {shortTs(row.approved_at)}</dd>
                  <dt>expires</dt><dd>{shortTs(row.expires_at)}</dd>
                  <dt>max allocation</dt><dd>{pct(row.max_allocation_pct)} · {money(row.max_notional_usd)} · qty {numberText(row.max_quantity)}</dd>
                  <dt>freeze</dt><dd>{row.manual_freeze ? row.freeze_reason ?? "manual freeze" : "none"}</dd>
                </dl>
              </section>

              {#if row.desired_position?.reason_codes?.length}
                <section>
                  <h3>Reason Codes</h3>
                  <div class="pill-row">
                    {#each row.desired_position.reason_codes as reason}
                      <span>{titleize(reason)}</span>
                    {/each}
                  </div>
                </section>
              {/if}

              {#if row.paper_orders?.orders?.length}
                <section>
                  <h3>Paper Orders</h3>
                  <div class="order-list">
                    {#each row.paper_orders.orders as order (order.order_id)}
                      <div>
                        <span class="badge">{titleize(order.status)}</span>
                        <strong>{titleize(order.order_role)} {titleize(order.action)}</strong>
                        <span>{numberText(order.quantity)} {titleize(order.position_side)} · {titleize(order.order_type)}</span>
                      </div>
                    {/each}
                  </div>
                </section>
              {/if}

              {#if row.incidents && row.incidents.length > 0}
                <section>
                  <h3>Incidents</h3>
                  <div class="incident-list">
                    {#each row.incidents as incident (incident.incident_id)}
                      <div>
                        <span class="badge incident-{incident.severity}">{titleize(incident.severity)}</span>
                        <strong>{incident.title}</strong>
                        <span>{titleize(incident.kind)} · {shortTs(incident.created_at)}</span>
                      </div>
                    {/each}
                  </div>
                </section>
              {/if}

              {#if hasObjectValue(row.desired_position?.feature_snapshot) || hasObjectValue(row.desired_position?.signal_ref)}
                <section>
                  <h3>Signal Payload</h3>
                  {#if hasObjectValue(row.desired_position?.feature_snapshot)}
                    <details>
                      <summary>feature snapshot</summary>
                      <pre>{jsonPreview(row.desired_position?.feature_snapshot)}</pre>
                    </details>
                  {/if}
                  {#if hasObjectValue(row.desired_position?.signal_ref)}
                    <details>
                      <summary>signal reference</summary>
                      <pre>{jsonPreview(row.desired_position?.signal_ref)}</pre>
                    </details>
                  {/if}
                </section>
              {/if}
            </div>
          </section>
        {:else if loading}
          <div class="empty-state">
            <strong>Loading automation state</strong>
            <span>Reading permissions, desired positions, proof gates, and execution state.</span>
          </div>
        {:else}
          <div class="empty-state">
            <strong>No selected strategy</strong>
            <span>Select a strategy row to inspect the decision path.</span>
          </div>
        {/if}

        <section class="timeline-panel">
          <div class="section-head">
            <div>
              <h2>Lifecycle Timeline</h2>
              <p>Permission, desired target, proof, reconciliation, order, sleeve, and incident events.</p>
            </div>
            <button type="button" onclick={() => refreshTimeline(true)} disabled={timelineLoading}>
              {timelineLoading ? "Loading" : "Reload"}
            </button>
          </div>

          {#if timelineError}
            <p class="error-text">{timelineError}</p>
          {/if}

          {#if timeline?.events?.length}
            <ol class="timeline-list">
              {#each timeline.events as event (`${event.source_kind}:${event.source_id}`)}
                <li>
                  <div class="timeline-marker"></div>
                  <div class="timeline-item">
                    <div class="timeline-top">
                      <span class="badge">{titleize(event.source_kind)}</span>
                      {#if event.status}
                        <span class="badge status-{event.status}">{titleize(event.status)}</span>
                      {/if}
                      <time>{shortTs(event.occurred_at)}</time>
                    </div>
                    <strong>{event.title}</strong>
                    <p>{event.summary}</p>
                    <div class="id-row">
                      {#each eventIds(event) as id}
                        <span>{id}</span>
                      {/each}
                    </div>
                    <details>
                      <summary>payload</summary>
                      <pre>{jsonPreview(event.payload)}</pre>
                    </details>
                  </div>
                </li>
              {/each}
            </ol>
          {:else if timelineLoading}
            <p class="muted">Loading lifecycle timeline...</p>
          {:else}
            <p class="muted">No automation lifecycle events recorded for this filter.</p>
          {/if}
        </section>
      </aside>
    </div>
  {:else if loading}
    <div class="empty-state page-loading">
      <strong>Loading autonomous trading state</strong>
      <span>Reading the latest automation status.</span>
    </div>
  {:else}
    <div class="empty-state page-loading">
      <strong>Automation state unavailable</strong>
      <span>Use refresh to retry the read-only cockpit endpoints.</span>
    </div>
  {/if}
</section>

<style>
  .auto-page {
    min-height: 0;
    height: 100%;
    overflow: auto;
    background: #0b0e14;
    color: #cdd6f4;
    padding: 1rem;
    display: flex;
    flex-direction: column;
    gap: .85rem;
  }

  .page-head,
  .head-actions,
  .summary-strip,
  .section-head,
  .selected-head,
  .permission-row,
  .gate-cell,
  .pill-row,
  .order-list div,
  .incident-list div,
  .timeline-top,
  .id-row,
  .blocker-strip {
    display: flex;
    align-items: center;
    gap: .5rem;
    flex-wrap: wrap;
  }

  .page-head,
  .section-head,
  .selected-head {
    justify-content: space-between;
  }

  .page-head {
    border-bottom: 1px solid #1f2733;
    padding-bottom: .85rem;
  }

  h1,
  h2,
  h3,
  p {
    margin: 0;
  }

  h1 {
    font-size: 1.35rem;
    line-height: 1.2;
  }

  h2 {
    font-size: .95rem;
  }

  h3 {
    color: #cdd6f4;
    font-size: .74rem;
    text-transform: uppercase;
  }

  p,
  .muted,
  .section-head span,
  .section-head p,
  .page-head p,
  small,
  time {
    color: #7f8aa3;
    font-size: .78rem;
  }

  .eyebrow {
    color: #89b4fa;
    font-size: .7rem;
    font-weight: 700;
    text-transform: uppercase;
  }

  button {
    background: #111827;
    color: #cdd6f4;
    border: 1px solid #2a3548;
    border-radius: 4px;
    padding: .28rem .55rem;
    font: inherit;
    cursor: pointer;
  }

  button:hover {
    background: #162033;
  }

  button:disabled {
    cursor: default;
    opacity: .65;
  }

  .chip-button {
    color: #89b4fa;
    border-color: rgba(137,180,250,.45);
  }

  .summary-strip {
    align-items: stretch;
    background: #0a0d14;
    border: 1px solid #1f2733;
    border-radius: 4px;
    padding: .55rem;
  }

  .summary-strip div {
    min-width: 96px;
    display: flex;
    flex-direction: column;
    gap: .1rem;
    border-right: 1px solid #1f2733;
    padding-right: .65rem;
  }

  .summary-strip div:last-child {
    border-right: 0;
  }

  .summary-strip span {
    color: #7f8aa3;
    font-size: .68rem;
    text-transform: uppercase;
  }

  .summary-strip strong {
    color: #cdd6f4;
    font-size: .95rem;
  }

  .page-grid {
    display: grid;
    grid-template-columns: minmax(420px, .95fr) minmax(420px, 1.05fr);
    gap: .85rem;
    align-items: start;
  }

  .decision-board,
  .selected-panel,
  .timeline-panel {
    border: 1px solid #1f2733;
    border-radius: 4px;
    background: #0c1019;
    padding: .75rem;
  }

  .decision-board,
  .detail-column,
  .selected-panel,
  .timeline-panel {
    display: flex;
    flex-direction: column;
    gap: .7rem;
  }

  .permission-table {
    display: flex;
    flex-direction: column;
    gap: .35rem;
  }

  .permission-row {
    width: 100%;
    align-items: stretch;
    text-align: left;
    background: #0a0d14;
    border-color: #1f2733;
    border-left-width: 3px;
    padding: .55rem;
    display: grid;
    grid-template-columns: 140px minmax(150px, 1.1fr) minmax(120px, .8fr) minmax(180px, 1fr);
    gap: .55rem;
  }

  .permission-row.active {
    border-color: #89b4fa;
    background: #101724;
  }

  .permission-row.action-blocked {
    border-left-color: #f9e2af;
  }

  .permission-row.action-enter {
    border-left-color: #a6e3a1;
  }

  .permission-row.action-exit {
    border-left-color: #f38ba8;
  }

  .permission-row.action-resize {
    border-left-color: #89b4fa;
  }

  .ticker,
  .action-cell,
  .position-cell,
  .reason-cell {
    display: flex;
    flex-direction: column;
    gap: .16rem;
  }

  .ticker strong {
    color: #f5e0dc;
    font-size: .98rem;
  }

  .reason-cell {
    grid-column: 1 / -1;
    color: #bac2de;
    font-size: .8rem;
  }

  .gate-cell {
    align-content: flex-start;
  }

  .badge {
    border: 1px solid #2a3548;
    background: #111827;
    border-radius: 3px;
    color: #bac2de;
    font-size: .7rem;
    padding: .08rem .35rem;
    white-space: nowrap;
  }

  .badge.action,
  .action-blocked,
  .proof-blocked,
  .rec-blocked,
  .readiness-blocked,
  .status-blocked {
    border-color: rgba(249,226,175,.45);
    color: #f9e2af;
  }

  .action-enter,
  .proof-passed,
  .rec-reconciled,
  .rec-noop,
  .readiness-ready,
  .status-filled,
  .status-approved {
    border-color: rgba(166,227,161,.45);
    color: #a6e3a1;
  }

  .action-exit,
  .status-rejected,
  .status-revoked,
  .incident-critical {
    border-color: rgba(243,139,168,.45);
    color: #f38ba8;
  }

  .action-resize,
  .status-submitted,
  .status-partially_filled {
    border-color: rgba(137,180,250,.45);
    color: #89b4fa;
  }

  .proof-warning,
  .incident-warning {
    border-color: rgba(250,179,135,.45);
    color: #fab387;
  }

  .selected-panel {
    gap: .75rem;
  }

  .selected-head h2 {
    margin-top: .1rem;
  }

  .callout {
    border-left: 3px solid #89b4fa;
    background: #0a0d14;
    padding: .55rem .65rem;
    display: flex;
    flex-direction: column;
    gap: .2rem;
  }

  .callout span {
    color: #a6adc8;
    font-size: .82rem;
  }

  .blocker-strip span,
  .pill-row span,
  .id-row span {
    background: rgba(249,226,175,.08);
    border: 1px solid rgba(249,226,175,.28);
    border-radius: 3px;
    color: #f9e2af;
    font-size: .74rem;
    padding: .08rem .35rem;
  }

  .id-row span {
    background: #0a0d14;
    border-color: #1f2733;
    color: #7f8aa3;
  }

  .metric-grid {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: .55rem;
  }

  .metric-grid section,
  .detail-list section {
    background: #0a0d14;
    border: 1px solid #1f2733;
    border-radius: 4px;
    padding: .55rem;
  }

  dl {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: .2rem .55rem;
    margin: .35rem 0 0;
    font-size: .78rem;
  }

  .wide-dl {
    grid-template-columns: 100px 1fr;
  }

  dt {
    color: #7f8aa3;
  }

  dd {
    margin: 0;
    color: #cdd6f4;
  }

  .detail-list {
    display: flex;
    flex-direction: column;
    gap: .55rem;
  }

  .order-list,
  .incident-list {
    display: flex;
    flex-direction: column;
    gap: .35rem;
    margin-top: .45rem;
  }

  .order-list div,
  .incident-list div {
    background: #0c1019;
    border: 1px solid #1f2733;
    border-radius: 4px;
    padding: .4rem;
  }

  details {
    margin-top: .45rem;
  }

  summary {
    color: #89b4fa;
    cursor: pointer;
    font-size: .78rem;
  }

  pre {
    margin: .35rem 0 0;
    max-height: 260px;
    overflow: auto;
    white-space: pre-wrap;
    word-break: break-word;
    background: #05070b;
    border: 1px solid #1f2733;
    border-radius: 4px;
    color: #bac2de;
    padding: .5rem;
    font-size: .72rem;
  }

  .timeline-list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: .55rem;
  }

  .timeline-list li {
    display: grid;
    grid-template-columns: 14px minmax(0, 1fr);
    gap: .45rem;
  }

  .timeline-marker {
    width: 9px;
    height: 9px;
    border-radius: 50%;
    margin-top: .4rem;
    background: #89b4fa;
    box-shadow: 0 0 0 3px rgba(137,180,250,.12);
  }

  .timeline-item {
    background: #0a0d14;
    border: 1px solid #1f2733;
    border-radius: 4px;
    padding: .5rem;
  }

  .timeline-item strong {
    display: block;
    margin-top: .35rem;
  }

  .timeline-item p {
    margin-top: .18rem;
    color: #a6adc8;
  }

  .empty-state {
    border: 1px dashed #2a3548;
    border-radius: 4px;
    background: #0a0d14;
    color: #a6adc8;
    padding: 1rem;
    display: flex;
    flex-direction: column;
    gap: .25rem;
  }

  .page-loading {
    min-height: 180px;
    justify-content: center;
  }

  .error-text {
    color: #f38ba8;
    font-size: .82rem;
  }

  @media (max-width: 1050px) {
    .page-grid {
      grid-template-columns: 1fr;
    }

    .permission-row {
      grid-template-columns: 1fr;
    }

    .reason-cell {
      grid-column: auto;
    }
  }

  @media (max-width: 720px) {
    .auto-page {
      padding: .75rem;
    }

    .metric-grid {
      grid-template-columns: 1fr;
    }

    .summary-strip div {
      min-width: 84px;
      border-right: 0;
    }
  }
</style>
