<script lang="ts">
  import type { Condition, ThesisDetail } from "./api";

  let { thesis }: { thesis: ThesisDetail } = $props();

  function stateColor(s: string): string {
    switch (s) {
      case "actionable":          return "rgb(166, 227, 161)";
      case "armed":               return "rgb(249, 226, 175)";
      case "position_open":       return "rgb(137, 220, 235)";
      case "building_conviction": return "rgb(180, 190, 254)";
      case "forming":             return "rgb(124, 124, 124)";
      case "exiting":             return "rgb(245, 194, 231)";
      case "closed":              return "rgb(108, 112, 134)";
      case "disqualified":        return "rgb(243, 139, 168)";
      default:                    return "rgb(180, 190, 254)";
    }
  }

  // The "WHY" comparator: figure out which conditions were dropped from the
  // immutable_original — these are goalpost-movement signals the user should
  // see at a glance, even before they click into version history.
  function diffNames(orig: Condition[] | undefined, curr: Condition[]) {
    const o = new Set((orig ?? []).map((c) => c.name));
    const c = new Set(curr.map((x) => x.name));
    return {
      dropped: [...o].filter((n) => !c.has(n)),
      added: [...c].filter((n) => !o.has(n)),
    };
  }

  let originalInv = $derived(thesis.immutable_original?.invalidation_conditions ?? []);
  let invDiff = $derived(diffNames(originalInv, thesis.invalidation_conditions));
  let edgeChanged = $derived(
    thesis.immutable_original?.edge_rationale !== undefined &&
    thesis.immutable_original.edge_rationale !== thesis.edge_rationale
  );

  // Substance checklist (#10): which structural slots are filled.
  let sub = $derived(thesis.substance);
  let slotState = $derived.by(() => {
    const wf = sub?.well_formed ?? { conviction: 0, trigger: 0, invalidation: 0, fulfillment: 0 };
    const missing = new Set(sub?.missing ?? []);
    return [
      { key: "edge_rationale",          label: "Edge rationale",   present: !!thesis.edge_rationale,          count: undefined },
      { key: "forecast",                label: "Forecast",          present: !missing.has("forecast"),         count: undefined },
      { key: "conviction_conditions",   label: "Conviction",        present: !missing.has("conviction_conditions"),   count: `${wf.conviction}/${thesis.conviction_conditions?.length ?? 0}` },
      { key: "trigger_conditions",      label: "Trigger",           present: !missing.has("trigger_conditions"),      count: `${wf.trigger}/${thesis.trigger_conditions?.length ?? 0}` },
      { key: "invalidation_conditions", label: "Invalidation",      present: !missing.has("invalidation_conditions"), count: `${wf.invalidation}/${thesis.invalidation_conditions?.length ?? 0}` },
      { key: "intended_size",           label: "Intended size",     present: !missing.has("intended_size"),    count: undefined },
      { key: "fulfillment_conditions",  label: "Fulfillment",       present: !missing.has("fulfillment_conditions"),  count: `${wf.fulfillment}/${thesis.fulfillment_conditions?.length ?? 0}` },
    ];
  });

  function fmtCondition(c: Condition): string {
    if (c.type === "quantitative") return c.expr ?? "(no expr)";
    return c.assertion ?? "(no assertion)";
  }

  function shortTs(s: string): string {
    return new Date(s).toLocaleString();
  }
</script>

<div class="thesis">
  <div class="hdr">
    <span class="state-badge" style="background:{stateColor(thesis.state)}">
      {thesis.state.replace(/_/g, " ")}
    </span>
    {#if thesis.conviction_tier}
      <span class="meta">conviction: <strong>{thesis.conviction_tier}</strong></span>
    {/if}
    {#if thesis.instrument}
      <span class="meta">instrument: <strong>{thesis.instrument}</strong></span>
    {/if}
    <span class="meta">v{thesis.version}</span>
    <span class="meta muted">updated {shortTs(thesis.updated_at)}</span>
  </div>

  {#if sub}
    <div class="substance" class:skeleton={sub.blocked_at !== null}>
      <div class="sub-hdr">
        <strong>Substance:</strong>
        <span class="score">{sub.score}/{sub.max_score}</span>
        {#if sub.blocked_at}
          <span class="badge danger">SKELETON — can't enter <code>{sub.blocked_at}</code></span>
        {:else}
          <span class="badge ok">complete — all gates pass</span>
        {/if}
      </div>
      <ul class="slots">
        {#each slotState as s (s.key)}
          <li class:on={s.present} class:off={!s.present}>
            {s.present ? "✓" : "✗"} {s.label}
            {#if s.count !== undefined}<span class="muted">{s.count} well-formed</span>{/if}
          </li>
        {/each}
      </ul>
    </div>
  {/if}

  <h4>Edge rationale</h4>
  <p class="rationale">{thesis.edge_rationale}</p>
  {#if edgeChanged}
    <p class="warn">⚠ Edge rationale has changed from v1.
      Original: <em>"{thesis.immutable_original.edge_rationale}"</em>
    </p>
  {/if}

  <div class="two-col">
    {#if thesis.bull_case}
      <div>
        <h4>Bull case</h4>
        <p>{thesis.bull_case}</p>
      </div>
    {/if}
    {#if thesis.bear_case}
      <div>
        <h4>Bear case</h4>
        <p>{thesis.bear_case}</p>
      </div>
    {/if}
  </div>

  <h4>
    Invalidation conditions
    {#if invDiff.dropped.length > 0}
      <span class="badge danger">⚠ {invDiff.dropped.length} dropped from v1</span>
    {/if}
  </h4>
  {#if thesis.invalidation_conditions.length === 0}
    <p class="muted">No invalidation conditions set — thesis is not falsifiable. ⚠</p>
  {/if}
  <ul class="cond-list">
    {#each thesis.invalidation_conditions as c (c.name)}
      <li>
        <span class="cond-type">{c.type}</span>
        <strong>{c.name}</strong>
        <code>{fmtCondition(c)}</code>
      </li>
    {/each}
    {#each invDiff.dropped as name (name)}
      {@const orig = (originalInv as Condition[]).find((c) => c.name === name)}
      <li class="dropped">
        <span class="cond-type">{orig?.type ?? "?"}</span>
        <strong>{name}</strong>
        <code>{orig ? fmtCondition(orig) : ""}</code>
        <span class="badge danger">DROPPED FROM v1</span>
      </li>
    {/each}
  </ul>

  {#if thesis.history.length > 0}
    <h4>Version history</h4>
    <ul class="hist">
      {#each thesis.history as h (`${h.version}-${h.at}`)}
        <li>
          <span class="meta">v{h.version}</span>
          {#if h.weakens_invalidation}
            <span class="badge danger">WEAKENED</span>
          {:else}
            <span class="badge ok">clean</span>
          {/if}
          {#if h.rationale}
            <span class="muted">— "{h.rationale}"</span>
          {/if}
          <span class="meta muted">{shortTs(h.at)}</span>
        </li>
      {/each}
    </ul>
  {/if}

  <details class="raw">
    <summary>Raw JSON</summary>
    <pre>{JSON.stringify({
      thesis_id: thesis.thesis_id,
      cluster_id: thesis.cluster_id,
      forecast: thesis.forecast,
      conviction_conditions: thesis.conviction_conditions,
      trigger_conditions: thesis.trigger_conditions,
      fulfillment_conditions: thesis.fulfillment_conditions,
      intended_size: thesis.intended_size,
    }, null, 2)}</pre>
  </details>
</div>

<style>
  .thesis {
    background: #0c1019; border: 1px solid #1f2733; border-radius: 6px;
    padding: 1rem; margin: 0.5rem 0;
  }
  .hdr { display: flex; gap: 0.6rem; align-items: baseline; flex-wrap: wrap; margin-bottom: 0.75rem; }
  .state-badge {
    color: #0a0d14; padding: 0.1rem 0.5rem; border-radius: 4px;
    font-size: 0.7rem; text-transform: uppercase; letter-spacing: 0.05em; font-weight: 600;
  }
  .meta { font-size: 0.8rem; color: #bac2de; }
  .muted { color: #6c7086; }
  h4 { font-size: 0.85rem; color: #bac2de; margin: 0.75rem 0 0.25rem 0; }
  p { margin: 0.25rem 0; line-height: 1.45; color: #cdd6f4; }
  .rationale { background: rgba(137, 180, 250, 0.08); padding: 0.5rem 0.75rem; border-left: 3px solid #89b4fa; border-radius: 4px; }
  .warn { background: rgba(249, 226, 175, 0.1); padding: 0.5rem 0.75rem; border-left: 3px solid #f9e2af; border-radius: 4px; color: #f9e2af; font-size: 0.85rem; }

  .two-col { display: grid; grid-template-columns: 1fr 1fr; gap: 1rem; }
  @media (max-width: 700px) { .two-col { grid-template-columns: 1fr; } }

  .cond-list { list-style: none; padding: 0; display: flex; flex-direction: column; gap: 0.25rem; }
  .cond-list li {
    background: #11161f; border: 1px solid #1f2733; border-radius: 4px;
    padding: 0.35rem 0.6rem; display: flex; gap: 0.5rem; align-items: baseline; flex-wrap: wrap;
  }
  .cond-list li.dropped { background: rgba(243, 139, 168, 0.08); border-color: rgba(243, 139, 168, 0.3); }
  .cond-type {
    font-size: 0.65rem; text-transform: uppercase; color: #89b4fa;
    background: rgba(137, 180, 250, 0.1); padding: 0.05rem 0.3rem; border-radius: 3px;
  }
  code { background: #0a0d14; padding: 0.05rem 0.3rem; border-radius: 3px; font-size: 0.85rem; }

  .badge {
    display: inline-block; padding: 0.05rem 0.4rem; border-radius: 4px; font-size: 0.7rem;
  }
  .badge.danger { background: rgba(243, 139, 168, 0.18); color: rgb(243, 139, 168); }
  .badge.ok { background: rgba(166, 227, 161, 0.15); color: rgb(166, 227, 161); }

  .hist { list-style: none; padding: 0; display: flex; flex-direction: column; gap: 0.25rem; }
  .hist li {
    background: #11161f; border: 1px solid #1f2733; border-radius: 4px;
    padding: 0.3rem 0.5rem; display: flex; gap: 0.5rem; align-items: baseline; flex-wrap: wrap;
  }

  .raw { margin-top: 1rem; }
  .raw summary { cursor: pointer; color: #6c7086; font-size: 0.75rem; }
  .raw pre { background: #0a0d14; padding: 0.5rem; border-radius: 4px; font-size: 0.75rem; overflow-x: auto; color: #bac2de; }

  .substance {
    background: rgba(180, 190, 254, 0.06); border: 1px solid #2a3548;
    border-radius: 6px; padding: 0.6rem 0.8rem; margin-bottom: 0.75rem;
  }
  .substance.skeleton {
    background: rgba(243, 139, 168, 0.06); border-color: rgba(243, 139, 168, 0.3);
  }
  .sub-hdr { display: flex; align-items: baseline; gap: 0.5rem; flex-wrap: wrap; }
  .score {
    background: rgba(137, 180, 250, 0.15); color: #89b4fa;
    padding: 0.1rem 0.45rem; border-radius: 4px; font-size: 0.75rem;
  }
  .slots {
    list-style: none; padding: 0; margin: 0.5rem 0 0 0;
    display: grid; grid-template-columns: repeat(auto-fill, minmax(180px, 1fr)); gap: 0.25rem 0.75rem;
    font-size: 0.85rem;
  }
  .slots li.on { color: #a6e3a1; }
  .slots li.off { color: #f38ba8; }
  .slots li .muted { margin-left: 0.4rem; }
</style>
