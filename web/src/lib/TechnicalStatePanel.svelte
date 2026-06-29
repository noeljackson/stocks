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

  function ratio(value: number | null | undefined): string {
    if (value === null || value === undefined || Number.isNaN(value)) return "-";
    return `${value.toLocaleString(undefined, { maximumFractionDigits: 2 })}x`;
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
        <span class="state-badge setup-{state.setup.kind}">{titleize(state.setup.kind)}</span>
        <span class="state-badge stance-{state.setup.entry_stance}">{titleize(state.setup.entry_stance)}</span>
        <span class="muted">as of {shortTs(state.as_of)}</span>
      </div>
      <p>{state.summary}</p>
      <p class="muted">{state.setup.summary}</p>
    </header>

    {#if state.cross}
      <section class="tech-section cross-read">
        <div class="section-hdr">
          <h4>Cross Analysis</h4>
          <span class="zone zone-{state.cross.buy_timing}">{titleize(state.cross.buy_timing)}</span>
        </div>
        <div class="cross-grid">
          <div>
            <span>trend</span>
            <strong>{titleize(state.cross.trend_state)}</strong>
          </div>
          <div>
            <span>momentum</span>
            <strong>{titleize(state.cross.momentum_state)}</strong>
          </div>
          <div>
            <span>VWAP</span>
            <strong>{titleize(state.cross.vwap_state)}</strong>
          </div>
          <div>
            <span>reversal</span>
            <strong>{titleize(state.cross.reversal_signal)}</strong>
          </div>
          <div>
            <span>relative strength</span>
            <strong>{titleize(state.cross.relative_strength_state)}</strong>
          </div>
          <div>
            <span>volume</span>
            <strong>{titleize(state.cross.volume_state)}</strong>
          </div>
          <div>
            <span>volatility</span>
            <strong>{titleize(state.cross.volatility_state)}</strong>
          </div>
        </div>
        <p>{state.cross.summary}</p>
      </section>
    {/if}

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
        {#if (state.daily.vwap ?? []).length > 0}
          <div class="sma-grid vwap-grid">
            {#each state.daily.vwap ?? [] as vwap (vwap.window)}
              <div class="sma-cell">
                <span>{vwap.window}D VWAP</span>
                <strong>{num(vwap.value)}</strong>
                <em class:pos={(vwap.pct_vs ?? 0) > 0} class:neg={(vwap.pct_vs ?? 0) < 0}>{pct(vwap.pct_vs)}</em>
                <span class="zone zone-{vwap.state}">{titleize(vwap.state)}</span>
              </div>
            {/each}
          </div>
        {/if}

        {#if state.daily.macd || state.daily.dmi || state.daily.atr || state.daily.bollinger || state.daily.volume}
          <div class="section-hdr subhdr">
            <h4>Daily Internals</h4>
          </div>
          <div class="indicator-grid">
            {#if state.daily.macd}
              <div class="indicator-cell">
                <span>MACD histogram</span>
                <strong>{num(state.daily.macd.histogram)}</strong>
                <em class:pos={(state.daily.macd.histogram_delta ?? 0) > 0} class:neg={(state.daily.macd.histogram_delta ?? 0) < 0}>
                  delta {num(state.daily.macd.histogram_delta)}
                </em>
                <span class="zone zone-{state.daily.macd.state}">{titleize(state.daily.macd.state)}</span>
              </div>
            {/if}
            {#if state.daily.dmi}
              <div class="indicator-cell">
                <span>ADX / DI</span>
                <strong>{num(state.daily.dmi.adx14)}</strong>
                <em>+DI {num(state.daily.dmi.plus_di14)} / -DI {num(state.daily.dmi.minus_di14)}</em>
                <span class="zone zone-{state.daily.dmi.state}">{titleize(state.daily.dmi.state)}</span>
              </div>
            {/if}
            {#if state.daily.atr}
              <div class="indicator-cell">
                <span>ATR 14</span>
                <strong>{num(state.daily.atr.atr14)}</strong>
                <em>NATR {num(state.daily.atr.natr14_pct)}%</em>
                <span class="zone zone-{state.daily.atr.state}">{titleize(state.daily.atr.state)}</span>
              </div>
            {/if}
            {#if state.daily.bollinger}
              <div class="indicator-cell">
                <span>Bollinger 20</span>
                <strong>%B {num(state.daily.bollinger.pct_b)}</strong>
                <em>width {num(state.daily.bollinger.bandwidth_pct)}%</em>
                <span class="zone zone-{state.daily.bollinger.state}">{titleize(state.daily.bollinger.state)}</span>
              </div>
            {/if}
            {#if state.daily.volume}
              <div class="indicator-cell">
                <span>Volume</span>
                <strong>{num(state.daily.volume.latest)}</strong>
                <em>vs 20D {ratio(state.daily.volume.ratio_vs_20)}</em>
                <span class="zone zone-{state.daily.volume.state}">{titleize(state.daily.volume.state)}</span>
              </div>
            {/if}
          </div>
        {/if}

        {#if (state.daily.relative_strength ?? []).length > 0}
          <div class="section-hdr subhdr">
            <h4>Relative Strength</h4>
          </div>
          <div class="table-scroll">
            <table class="tech-table">
              <thead>
                <tr>
                  <th>benchmark</th>
                  <th>20D</th>
                  <th>60D</th>
                  <th>120D</th>
                  <th>state</th>
                </tr>
              </thead>
              <tbody>
                {#each state.daily.relative_strength ?? [] as row (row.benchmark)}
                  <tr>
                    <td>{row.benchmark}</td>
                    <td class:pos={(row.rel_20d_pct ?? 0) > 0} class:neg={(row.rel_20d_pct ?? 0) < 0}>{pct(row.rel_20d_pct)}</td>
                    <td class:pos={(row.rel_60d_pct ?? 0) > 0} class:neg={(row.rel_60d_pct ?? 0) < 0}>{pct(row.rel_60d_pct)}</td>
                    <td class:pos={(row.rel_120d_pct ?? 0) > 0} class:neg={(row.rel_120d_pct ?? 0) < 0}>{pct(row.rel_120d_pct)}</td>
                    <td><span class="zone zone-{row.state}">{titleize(row.state)}</span></td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
        {/if}
      </section>
    {/if}

    <section class="tech-section">
      <div class="section-hdr">
        <h4>Momentum By Interval</h4>
      </div>
      <div class="table-scroll">
        <table class="tech-table">
          <thead>
            <tr>
              <th>bar</th>
              <th>close</th>
              <th>RSI 14</th>
              <th>Stoch %K/%D</th>
              <th>PSO 8/25</th>
              <th>PSO 32</th>
              <th>zone</th>
              <th>span</th>
            </tr>
          </thead>
          <tbody>
            {#each state.intervals as interval (interval.interval)}
              <tr>
                <td>{interval.interval}</td>
                <td>{num(interval.close)}</td>
                <td>{num(interval.rsi14)}</td>
                <td>{num(interval.stochastic_k14)} / {num(interval.stochastic_d3)}</td>
                <td>
                  {num(interval.pso)}
                  {#if interval.pso_delta !== null && interval.pso_delta !== undefined}
                    <span class:pos={interval.pso_delta > 0} class:neg={interval.pso_delta < 0}>({interval.pso_delta > 0 ? "+" : ""}{num(interval.pso_delta)})</span>
                  {/if}
                </td>
                <td>
                  {num(interval.pso32)}
                  {#if interval.pso32_delta !== null && interval.pso32_delta !== undefined}
                    <span class:pos={interval.pso32_delta > 0} class:neg={interval.pso32_delta < 0}>({interval.pso32_delta > 0 ? "+" : ""}{num(interval.pso32_delta)})</span>
                  {/if}
                </td>
                <td class="zone-stack">
                  <span class="zone zone-{interval.pso_zone}">8 {titleize(interval.pso_zone)}</span>
                  <span class="zone zone-{interval.pso32_zone}">32 {titleize(interval.pso32_zone)}</span>
                </td>
                <td>
                  {interval.pso_zone_bars > 0 ? `${interval.pso_zone_bars}` : "-"}
                  /
                  {interval.pso32_zone_bars > 0 ? `${interval.pso32_zone_bars}` : "-"}
                  bars
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
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
  .tech-top.state-reversal_confirming,
  .tech-top.state-pullback_watch { border-left-color: rgb(137, 180, 250); }
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
  .state-badge.state-reversal_confirming,
  .state-badge.setup-200d_reclaim,
  .state-badge.setup-pullback_reversal,
  .state-badge.stance-actionable,
  .state-badge.stance-starter_ok,
  .state-badge.stance-constructive,
  .zone-bullish,
  .zone-bull_trend,
  .zone-positive,
  .zone-confirmed,
  .zone-outperforming,
  .zone-accumulation,
  .zone-constructive,
  .zone-pullback_reversal,
  .zone-strong,
  .zone-up {
    background: rgba(166, 227, 161, .18);
    color: rgb(166, 227, 161);
  }

  .state-badge.state-extended,
  .state-badge.state-pullback_watch,
  .state-badge.setup-extended_run,
  .state-badge.setup-200d_reclaim_watch,
  .state-badge.setup-pullback_watch,
  .state-badge.stance-wait_reclaim,
  .state-badge.stance-wait_retest,
  .state-badge.stance-wait_breakout,
  .state-badge.stance-wait_reversal,
  .state-badge.stance-avoid_chase,
  .zone-improving,
  .zone-early,
  .zone-compressed,
  .zone-expanded,
  .zone-quiet,
  .zone-pullback_watch,
  .zone-pullback_in_uptrend,
  .zone-wait,
  .zone-testing_200d,
  .zone-avoid_chase,
  .zone-extended,
  .zone-extended_chase,
  .zone-lower_band,
  .zone-upper_band,
  .zone-overbought {
    background: rgba(249, 226, 175, .15);
    color: rgb(249, 226, 175);
  }

  .state-badge.state-deteriorating,
  .state-badge.setup-breakdown,
  .state-badge.stance-avoid,
  .zone-bearish,
  .zone-bear_trend,
  .zone-breakdown,
  .zone-avoid_breakdown,
  .zone-underperforming,
  .zone-distribution,
  .zone-weak,
  .zone-oversold,
  .zone-down {
    background: rgba(243, 139, 168, .18);
    color: rgb(243, 139, 168);
  }

  .cross-grid,
  .indicator-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(126px, 1fr));
    gap: .4rem;
    margin-bottom: .5rem;
  }

  .cross-grid div,
  .indicator-cell {
    border: 1px solid #1f2733;
    border-radius: 4px;
    padding: .35rem;
    display: grid;
    gap: .14rem;
    min-width: 0;
  }

  .cross-grid span,
  .indicator-cell span,
  .indicator-cell em {
    color: #6c7693;
    font-style: normal;
    font-size: .72rem;
  }

  .cross-grid strong,
  .indicator-cell strong {
    font-size: .86rem;
  }

  .subhdr {
    margin-top: .65rem;
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

  .table-scroll {
    overflow-x: auto;
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

  .zone-stack {
    display: flex;
    gap: .25rem;
    flex-wrap: wrap;
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
