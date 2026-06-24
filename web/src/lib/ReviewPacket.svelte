<script lang="ts">
  import type { AttentionReviewPacket, ReviewPacketAction } from "./api";

  type Props = {
    packet?: AttentionReviewPacket | null;
    loading?: boolean;
    error?: string | null;
    onAction?: (action: ReviewPacketAction, packet: AttentionReviewPacket) => void;
  };

  let {
    packet = null as AttentionReviewPacket | null,
    loading = false,
    error = null as string | null,
    onAction = (_action: ReviewPacketAction, _packet: AttentionReviewPacket) => {},
  }: Props = $props();

  function label(value?: string | null): string {
    return value ? value.replace(/_/g, " ") : "unknown";
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
  <section class="review-packet" data-testid="review-packet">
    <div class="packet-head">
      <div>
        <span class="kicker">review packet</span>
        <strong>{packet.attention.symbol ?? "System"} · {label(packet.attention.kind)}</strong>
      </div>
      <span class="badge state-{packet.attention.fsm_state ?? 'ready_for_review'}">{label(packet.attention.fsm_state)}</span>
    </div>

    <div class="packet-grid">
      {#each packet.sections as section (section.key)}
        <article class="packet-section">
          <span>{section.title}</span>
          {#if section.body}
            <p>{section.body}</p>
          {/if}
          {#if section.items?.length}
            <ul>
              {#each section.items as item}
                <li>{item}</li>
              {/each}
            </ul>
          {:else if !section.body}
            <p class="muted">No source-backed details attached.</p>
          {/if}
        </article>
      {/each}
    </div>

    <div class="packet-actions">
      {#each packet.allowed_actions as action (action.id)}
        <button type="button" onclick={() => onAction(action, packet)}>
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
    padding: .6rem;
    display: grid;
    gap: .6rem;
  }
  .review-packet.error {
    border-left-color: #f38ba8;
    color: #f38ba8;
  }
  .packet-head {
    display: flex;
    justify-content: space-between;
    gap: .5rem;
    align-items: start;
    flex-wrap: wrap;
  }
  .kicker,
  .packet-section span {
    display: block;
    color: #89b4fa;
    font-size: .7rem;
    text-transform: uppercase;
    letter-spacing: 0;
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
    background: #080c13;
    border-radius: 4px;
    padding: .45rem .5rem;
    min-width: 0;
  }
  p,
  ul {
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
  .packet-actions {
    display: flex;
    gap: .4rem;
    flex-wrap: wrap;
  }
  .packet-actions button {
    border: 1px solid #2a3447;
    background: #111827;
    color: #cdd6f4;
    border-radius: 4px;
    padding: .36rem .5rem;
    font: inherit;
    cursor: pointer;
    display: grid;
    gap: .08rem;
    text-align: left;
    max-width: 15rem;
  }
  .packet-actions button:hover {
    border-color: #45567a;
    background: #162033;
  }
  .packet-actions small {
    color: #7f849c;
    font-size: .68rem;
    line-height: 1.2;
  }
  .badge {
    border: 1px solid #2a3447;
    border-radius: 999px;
    padding: .1rem .4rem;
    color: #bac2de;
    font-size: .7rem;
    white-space: nowrap;
  }
  .muted {
    color: #7f849c;
  }
</style>
