<script lang="ts">
  import type { TechnicalState } from "./api";

  let { state }: { state: TechnicalState | null | undefined } = $props();

  function titleize(value: string | null | undefined): string {
    return (value ?? "unknown").replace(/_/g, " ");
  }

  function shortTs(value: string | null | undefined): string {
    if (!value) return "-";
    return new Date(value).toLocaleString();
  }

  function num(value: number | null | undefined): string {
    if (value === null || value === undefined || Number.isNaN(value)) return "-";
    return value.toLocaleString(undefined, { maximumFractionDigits: 2 });
  }

  function pct(value: number | null | undefined): string {
    if (value === null || value === undefined || Number.isNaN(value)) return "-";
    const sign = value > 0 ? "+" : "";
    return `${sign}${value.toFixed(1)}%`;
  }

  function directionLabel(direction: string): string {
    return direction === "up" ? "crossed above" : direction === "down" ? "crossed below" : direction;
  }

  function analogKind(kind: string): string {
    return kind.replace(/^daily_rsi_entered_/, "daily RSI entered ").replace(/_/g, " ");
  }
</script>

{#if state === undefined}
  <p class="muted">Loading technical state...</p>
{:else if state === null}
  <p class="muted">Technical state unavailable.</p>
{:else}
  <div class="technical">
    <header class="tech-top state-{state.state}">
      <div class="tech-title">
        <strong>{state.symbol}</strong>
        <span class="state-badge state-{state.state}">{titleize(state.state)}</span>
        <span class="muted">as of {shortTs(state.as_of)}</span>
      </div>
      <p>{state.summary}</p>
    </header>

    {#if state.daily}
      <section class="tech-section">
        <div class="section-hdr">
          <h4>Daily Position</h4>
          <span class="muted">{shortTs(state.daily.as_of)}</span>
        </div>
        <dl class="daily-grid">
          <dt>close</dt><dd>{num(state.daily.close)}</dd>
          <dt>vs 252d high</dt><dd class:pos={(state.daily.pct_vs_252d_high ?? 0) > 0} class:neg={(state.daily.pct_vs_252d_high ?? 0) < 0}>{pct(state.daily.pct_vs_252d_high)}</dd>
          <dt>vs 252d low</dt><dd class:pos={(state.daily.pct_vs_252d_low ?? 0) > 0} class:neg={(state.daily.pct_vs_252d_low ?? 0) < 0}>{pct(state.daily.pct_vs_252d_low)}</dd>
        </dl>
        <div class="sma-grid">
          {#each state.daily.sma as sma (sma.window)}
            <div class="sma-cell">
              <span>{sma.window}D SMA</span>
              <strong>{num(sma.value)}</strong>
              <em class:pos={(sma.pct_vs ?? 0) > 0} class:neg={(sma.pct_vs ?? 0) < 0}>{pct(sma.pct_vs)}</em>
            </div>
          {/each}
        </div>
      </section>
    {/if}

    <section class="tech-section">
      <div class="section-hdr">
        <h4>RSI By Interval</h4>
      </div>
      <table class="tech-table">
        <thead>
          <tr><th>bar</th><th>close</th><th>RSI 14</th><th>zone</th><th>span</th></tr>
        </thead>
        <tbody>
          {#each state.intervals as interval (interval.interval)}
            <tr>
              <td>{interval.interval}</td>
              <td>{num(interval.close)}</td>
              <td>{num(interval.rsi14)}</td>
              <td><span class="zone zone-{interval.rsi_zone}">{titleize(interval.rsi_zone)}</span></td>
              <td>{interval.rsi_zone_bars > 0 ? `${interval.rsi_zone_bars} bars` : "-"}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    </section>

    <section class="tech-section">
      <div class="section-hdr">
        <h4>SMA Crosses</h4>
      </div>
      {#if state.last_crosses.length === 0}
        <p class="muted">No recent 50D or 200D cross events in stored daily history.</p>
      {:else}
        <ul class="event-list">
          {#each state.last_crosses as event (`${event.window}-${event.direction}-${event.at}`)}
            <li>
              <span class="zone zone-{event.direction === 'up' ? 'strong' : 'weak'}">
                {event.window}D {directionLabel(event.direction)}
              </span>
              <span>{num(event.close)} vs SMA {num(event.sma)}</span>
              <span class="muted">{shortTs(event.at)}</span>
            </li>
          {/each}
        </ul>
      {/if}
    </section>

    <section class="tech-section">
      <div class="section-hdr">
        <h4>Daily RSI Analogs</h4>
      </div>
      {#if state.analog_events.length === 0}
        <p class="muted">No matching daily RSI analog events with 20-day forward windows yet.</p>
      {:else}
        <ul class="event-list">
          {#each state.analog_events as event (`${event.kind}-${event.at}`)}
            <li>
              <span>{analogKind(event.kind)}</span>
              <span>20d return <strong class:pos={(event.forward_return_20d_pct ?? 0) > 0} class:neg={(event.forward_return_20d_pct ?? 0) < 0}>{pct(event.forward_return_20d_pct)}</strong></span>
              <span>max drawdown <strong class="neg">{pct(event.max_drawdown_20d_pct)}</strong></span>
              <span class="muted">{shortTs(event.at)}</span>
            </li>
          {/each}
        </ul>
      {/if}
    </section>
  </div>
{/if}

<style>
  .technical {
    display: flex;
    flex-direction: column;
    gap: .65rem;
    min-width: 0;
  }

  .tech-top,
  .tech-section {
    border: 1px solid #1f2733;
    background: #0a0d14;
    border-radius: 4px;
    padding: .6rem .7rem;
  }

  .tech-top {
    border-left: 3px solid #6c7693;
  }

  .tech-top.state-constructive,
  .tech-top.state-base_building { border-left-color: rgb(166, 227, 161); }
  .tech-top.state-extended { border-left-color: rgb(249, 226, 175); }
  .tech-top.state-deteriorating { border-left-color: rgb(243, 139, 168); }

  .tech-title,
  .section-hdr,
  .event-list li {
    display: flex;
    align-items: center;
    gap: .45rem;
    flex-wrap: wrap;
  }

  .section-hdr {
    justify-content: space-between;
    margin-bottom: .45rem;
  }

  h4,
  p {
    margin: 0;
  }

  .muted {
    color: #6c7693;
  }

  .state-badge,
  .zone {
    border-radius: 999px;
    padding: .08rem .4rem;
    font-size: .68rem;
    text-transform: lowercase;
    background: rgba(108, 112, 134, .2);
    color: #9aa3b8;
    white-space: nowrap;
  }

  .state-badge.state-constructive,
  .state-badge.state-base_building,
  .zone-strong,
  .zone-up {
    background: rgba(166, 227, 161, .18);
    color: rgb(166, 227, 161);
  }

  .state-badge.state-extended,
  .zone-overbought {
    background: rgba(249, 226, 175, .15);
    color: rgb(249, 226, 175);
  }

  .state-badge.state-deteriorating,
  .zone-weak,
  .zone-oversold,
  .zone-down {
    background: rgba(243, 139, 168, .18);
    color: rgb(243, 139, 168);
  }

  .daily-grid {
    display: grid;
    grid-template-columns: repeat(3, auto 1fr);
    gap: .25rem .5rem;
    margin: 0 0 .55rem;
  }

  .daily-grid dt {
    color: #6c7693;
  }

  .daily-grid dd {
    margin: 0;
  }

  .sma-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(92px, 1fr));
    gap: .4rem;
  }

  .sma-cell {
    border: 1px solid #1f2733;
    border-radius: 4px;
    padding: .35rem;
    display: grid;
    gap: .12rem;
  }

  .sma-cell span,
  .sma-cell em {
    color: #6c7693;
    font-style: normal;
    font-size: .72rem;
  }

  .tech-table {
    width: 100%;
    border-collapse: collapse;
    font-size: .78rem;
  }

  .tech-table th,
  .tech-table td {
    border-bottom: 1px solid #1f2733;
    padding: .28rem .25rem;
    text-align: left;
    white-space: nowrap;
  }

  .tech-table th {
    color: #6c7693;
    font-weight: 500;
  }

  .event-list {
    display: grid;
    gap: .35rem;
    list-style: none;
    margin: 0;
    padding: 0;
  }

  .event-list li {
    justify-content: space-between;
    border-bottom: 1px solid #1f2733;
    padding-bottom: .3rem;
  }

  .event-list li:last-child {
    border-bottom: none;
    padding-bottom: 0;
  }

  .pos {
    color: rgb(166, 227, 161);
  }

  .neg {
    color: rgb(243, 139, 168);
  }
</style>
