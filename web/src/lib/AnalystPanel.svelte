<script lang="ts">
  import { askChatAnalyst, type ChatAnalystResponse } from "./api";

  let { symbol }: { symbol: string } = $props();

  let scope = $state<"symbol" | "technical" | "decision">("technical");
  let question = $state("");
  let response = $state<ChatAnalystResponse | null>(null);
  let status = $state<"idle" | "asking">("idle");
  let error = $state<string | null>(null);
  let lastSymbol = "";

  $effect(() => {
    if (symbol === lastSymbol) return;
    lastSymbol = symbol;
    question = `Is ${symbol}'s thesis contradicted by the current technical state?`;
    response = null;
    error = null;
    scope = "technical";
  });

  async function submit() {
    const q = question.trim();
    if (!q || status === "asking") return;
    status = "asking";
    error = null;
    try {
      response = await askChatAnalyst({ symbol, scope, question: q });
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      status = "idle";
    }
  }

  function titleize(value: string | null | undefined): string {
    return (value ?? "unknown").replace(/_/g, " ");
  }

  function shortTs(value: string | null | undefined): string {
    if (!value) return "";
    return new Date(value).toLocaleString();
  }
</script>

<form class="analyst-form" onsubmit={(e) => { e.preventDefault(); void submit(); }}>
  <div class="analyst-controls">
    <select bind:value={scope} aria-label="analyst scope">
      <option value="technical">technical</option>
      <option value="symbol">symbol</option>
      <option value="decision">decision</option>
    </select>
    <button type="submit" disabled={status === "asking" || !question.trim()}>
      {status === "asking" ? "asking..." : "ask"}
    </button>
  </div>
  <textarea bind:value={question} rows="4" aria-label="analyst question"></textarea>
</form>

{#if error}
  <p class="error-text">{error}</p>
{/if}

{#if response}
  <section class="analyst-answer">
    <div class="answer-hdr">
      <span class="badge conf-{response.answer.confidence}">{response.answer.confidence}</span>
      <span class="muted">{response.scope}</span>
      {#if response.used_fallback}<span class="muted">fallback{response.fallback_reason ? `: ${response.fallback_reason}` : ""}</span>{/if}
      {#if response.queued_evidence > 0}<span class="badge queued">{response.queued_evidence} source tasks</span>{/if}
    </div>
    <p>{response.answer.answer}</p>
  </section>

  {#if response.answer.technical_read?.summary || response.answer.technical_read?.state}
    <section class="analyst-block">
      <h4>Technical Read</h4>
      <div class="answer-hdr">
        {#if response.answer.technical_read.state}
          <span class="badge tech-{response.answer.technical_read.state}">
            {titleize(response.answer.technical_read.state)}
          </span>
        {/if}
      </div>
      {#if response.answer.technical_read.summary}
        <p>{response.answer.technical_read.summary}</p>
      {/if}
      {#if response.answer.technical_read.timing_implication}
        <p class="muted">{response.answer.technical_read.timing_implication}</p>
      {/if}
    </section>
  {/if}

  <section class="analyst-block">
    <h4>Thesis Impact</h4>
    <div class="answer-hdr">
      <span class="badge impact-{response.answer.thesis_impact.kind}">
        {titleize(response.answer.thesis_impact.kind)}
      </span>
    </div>
    {#if response.answer.thesis_impact.reason}
      <p>{response.answer.thesis_impact.reason}</p>
    {/if}
  </section>

  {#if response.answer.evidence_used.length > 0}
    <section class="analyst-block">
      <h4>Evidence Used</h4>
      <ul>
        {#each response.answer.evidence_used as item, i (`${item.source}-${item.evidence_id ?? i}`)}
          <li>
            <strong>{item.summary}</strong>
            <span class="muted">
              {item.source}{item.evidence_id ? ` #${item.evidence_id}` : ""}
              {item.observed_at ? ` · ${shortTs(item.observed_at)}` : ""}
            </span>
          </li>
        {/each}
      </ul>
    </section>
  {/if}

  {#if response.answer.requested_evidence.length > 0}
    <section class="analyst-block">
      <h4>Requested Evidence</h4>
      <ul>
        {#each response.answer.requested_evidence as req (`${req.requirement_key}-${req.source_type}`)}
          <li>
            <div class="answer-hdr">
              <span class="badge priority-{req.priority}">{req.priority}</span>
              <strong>{titleize(req.requirement_key)}</strong>
              <span class="muted">{titleize(req.source_type)}</span>
            </div>
            <p>{req.reason}</p>
          </li>
        {/each}
      </ul>
    </section>
  {/if}
{/if}

<style>
  .analyst-form {
    display: flex;
    flex-direction: column;
    gap: .45rem;
  }

  .analyst-controls,
  .answer-hdr {
    display: flex;
    align-items: center;
    gap: .45rem;
    flex-wrap: wrap;
  }

  select,
  textarea {
    background: #0a0d14;
    color: #cdd6f4;
    border: 1px solid #2a3548;
    border-radius: 4px;
    font: inherit;
  }

  select {
    padding: .25rem .35rem;
  }

  textarea {
    width: 100%;
    resize: vertical;
    min-height: 5rem;
    padding: .45rem .55rem;
    line-height: 1.35;
  }

  button {
    background: #1b2230;
    color: #cdd6f4;
    border: 1px solid #2a3548;
    border-radius: 4px;
    padding: .25rem .65rem;
    font: inherit;
    cursor: pointer;
  }

  button:disabled {
    opacity: .55;
    cursor: default;
  }

  .analyst-answer,
  .analyst-block {
    border: 1px solid #1f2733;
    background: #0a0d14;
    border-radius: 4px;
    padding: .6rem .7rem;
    margin-top: .65rem;
  }

  .analyst-answer {
    border-left: 3px solid #89b4fa;
  }

  h4,
  p {
    margin: 0;
  }

  .analyst-block h4 {
    margin-bottom: .35rem;
    font-size: .85rem;
  }

  .analyst-block p,
  .analyst-answer p {
    line-height: 1.38;
  }

  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: .45rem;
  }

  li {
    display: grid;
    gap: .18rem;
  }

  .muted {
    color: #6c7693;
  }

  .error-text {
    color: rgb(243, 139, 168);
    margin: .5rem 0 0;
  }

  .badge {
    border-radius: 999px;
    padding: .08rem .4rem;
    font-size: .68rem;
    text-transform: lowercase;
    background: rgba(108,112,134,.2);
    color: #9aa3b8;
    white-space: nowrap;
  }

  .badge.conf-high,
  .badge.tech-constructive,
  .badge.tech-base_building,
  .badge.impact-supports { background: rgba(166,227,161,.18); color: rgb(166,227,161); }

  .badge.conf-medium,
  .badge.tech-extended,
  .badge.priority-high,
  .badge.priority-blocking,
  .badge.impact-weakens,
  .badge.impact-needs_reconciliation,
  .badge.queued { background: rgba(249,226,175,.15); color: rgb(249,226,175); }

  .badge.conf-low,
  .badge.tech-deteriorating,
  .badge.impact-contradicts { background: rgba(243,139,168,.18); color: rgb(243,139,168); }
</style>
