<script lang="ts">
  import type { BrainJournal, BrainJournalCategory, BrainJournalEntry } from "./api";

  type Props = {
    journal?: BrainJournal | null;
    date: string;
    today: string;
    loading?: boolean;
    error?: string | null;
    onDateChange?: (date: string) => void;
    onPageChange?: (page: number) => void;
    onOpenEntry?: (entry: BrainJournalEntry) => void;
    onBack?: () => void;
  };

  let {
    journal = null as BrainJournal | null,
    date,
    today,
    loading = false,
    error = null as string | null,
    onDateChange = (_date: string) => {},
    onPageChange = (_page: number) => {},
    onOpenEntry = (_entry: BrainJournalEntry) => {},
    onBack = () => {},
  }: Props = $props();

  const CATEGORY_ORDER = ["changed", "ignored_or_hated", "crowded_or_extended", "research", "curious", "blocked"];

  let groups = $derived.by<{ category: BrainJournalCategory | string; entries: BrainJournalEntry[]; total: number }[]>(() => {
    const map = new Map<string, BrainJournalEntry[]>();
    for (const entry of journal?.entries ?? []) {
      const bucket = map.get(entry.category) ?? [];
      bucket.push(entry);
      map.set(entry.category, bucket);
    }
    const totals = journal?.summary.all_by_category ?? journal?.summary.by_category ?? {};
    return [...map.entries()]
      .sort(([a], [b]) => {
        const ai = CATEGORY_ORDER.indexOf(a);
        const bi = CATEGORY_ORDER.indexOf(b);
        return (ai === -1 ? 99 : ai) - (bi === -1 ? 99 : bi);
      })
      .map(([category, entries]) => ({
        category,
        total: Number(totals[category] ?? entries.length),
        entries: entries.sort((a, b) => b.importance - a.importance || Date.parse(b.occurred_at) - Date.parse(a.occurred_at)),
      }));
  });

  let visibleCount = $derived(journal?.entries.length ?? 0);
  let totalCount = $derived(journal?.pagination?.total ?? journal?.summary.total ?? 0);
  let page = $derived(journal?.pagination?.page ?? 1);
  let totalPages = $derived(journal?.pagination?.total_pages ?? (totalCount ? 1 : 0));
  let hasPreviousPage = $derived(journal?.pagination?.has_previous ?? false);
  let hasNextPage = $derived(journal?.pagination?.has_next ?? false);

  function categoryLabel(category: BrainJournalCategory | string): string {
    switch (category) {
      case "changed":              return "we think this changed";
      case "curious":              return "we are curious";
      case "research":             return "needs research";
      case "crowded_or_extended":  return "crowded or extended";
      case "ignored_or_hated":     return "ignored or hated";
      case "blocked":              return "blocked";
      default:                     return category.replace(/_/g, " ");
    }
  }

  function relativeTime(iso: string): string {
    const t = new Date(iso).getTime();
    const dt = Date.now() - t;
    if (dt < -86_400_000) return new Date(t).toLocaleDateString();
    if (dt < -3_600_000) return `in ${Math.ceil(Math.abs(dt) / 3_600_000)}h`;
    if (dt < -60_000) return `in ${Math.ceil(Math.abs(dt) / 60_000)}m`;
    if (dt < 60_000) return "just now";
    if (dt < 3_600_000) return `${Math.floor(dt / 60_000)}m ago`;
    if (dt < 86_400_000) return new Date(t).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
    if (dt < 7 * 86_400_000) return `${Math.floor(dt / 86_400_000)}d ago`;
    return new Date(t).toLocaleDateString();
  }

  function addDays(value: string, days: number): string {
    const d = new Date(`${value}T00:00:00Z`);
    d.setUTCDate(d.getUTCDate() + days);
    return d.toISOString().slice(0, 10);
  }

  function changeDate(next: string) {
    if (!/^\d{4}-\d{2}-\d{2}$/.test(next)) return;
    onDateChange(next);
  }

  function sourceLabel(source: string): string {
    return source.replace(/_/g, " ");
  }
</script>

<main class="brain-journal-page" data-testid="brain-journal-page">
  <section class="journal-hero">
    <div>
      <span class="eyebrow">Brain Journal</span>
      <h1>{date}</h1>
      <p class="muted">Daily history of what changed, what needs research, what is blocked, and where the Brain sees crowded or ignored pockets.</p>
    </div>
    <div class="journal-actions">
      <button type="button" onclick={onBack}>Workspace</button>
      <button type="button" onclick={() => changeDate(addDays(date, -1))}>Previous day</button>
      <label>
        <span class="sr-only">Journal date</span>
        <input
          type="date"
          value={date}
          max={today}
          onchange={(e) => changeDate((e.target as HTMLInputElement).value)}
        />
      </label>
      <button type="button" disabled={date >= today} onclick={() => changeDate(addDays(date, 1))}>Next day</button>
      <button type="button" disabled={date === today} onclick={() => changeDate(today)}>Today</button>
    </div>
  </section>

  <section class="journal-summary">
    <span><strong>{totalCount}</strong> total entries</span>
    <span><strong>{visibleCount}</strong> shown</span>
    <span><strong>{totalPages}</strong> page{totalPages === 1 ? "" : "s"}</span>
    {#if journal?.as_of}<span class="muted">refreshed {relativeTime(journal.as_of)}</span>{/if}
  </section>

  {#if journal?.synthesis}
    <section class="journal-synthesis">
      <strong>Synthesis</strong>
      <p>{journal.synthesis}</p>
    </section>
  {/if}

  {#if loading}
    <p class="muted">Loading journal…</p>
  {:else if error}
    <p class="error-text">{error}</p>
  {:else if groups.length}
    <div class="journal-grid">
      {#each groups as group (group.category)}
        <section class="journal-group journal-{group.category}">
          <div class="journal-group-title">
            <strong>{categoryLabel(group.category)}</strong>
            <span class="badge">{group.entries.length}/{group.total}</span>
          </div>
          <div class="journal-entries">
            {#each group.entries as entry (entry.event_key)}
              {#if entry.symbol}
                <button type="button" class="journal-entry" onclick={() => onOpenEntry(entry)}>
                  <span class="journal-entry-title">
                    <strong>{entry.symbol}</strong>
                    {entry.title}
                  </span>
                  <span class="muted">{entry.summary}</span>
                  <span class="journal-meta">
                    {sourceLabel(entry.source_kind)}
                    · importance {entry.importance}
                    · {relativeTime(entry.occurred_at)}
                  </span>
                </button>
              {:else}
                <div class="journal-entry static">
                  <span class="journal-entry-title">{entry.title}</span>
                  <span class="muted">{entry.summary}</span>
                  <span class="journal-meta">
                    {sourceLabel(entry.source_kind)}
                    · importance {entry.importance}
                    · {relativeTime(entry.occurred_at)}
                  </span>
                </div>
              {/if}
            {/each}
          </div>
        </section>
      {/each}
    </div>

    <nav class="journal-pager" aria-label="Journal entry pages">
      <button type="button" disabled={!hasPreviousPage} onclick={() => onPageChange(page - 1)}>Previous page</button>
      <span class="muted">Page {page}{totalPages ? ` of ${totalPages}` : ""}</span>
      <button type="button" disabled={!hasNextPage} onclick={() => onPageChange(page + 1)}>Next page</button>
    </nav>
  {:else}
    <p class="muted">No journal entries recorded for this date.</p>
  {/if}
</main>

<style>
  .brain-journal-page {
    grid-row: 2 / -1;
    flex: 1;
    overflow: auto;
    padding: .75rem;
    display: flex;
    flex-direction: column;
    gap: .75rem;
    min-height: 0;
  }
  .journal-hero,
  .journal-summary,
  .journal-synthesis,
  .journal-pager {
    border: 1px solid #1f2733;
    background: #0a0d14;
    border-radius: 4px;
    padding: .65rem .75rem;
  }
  .journal-hero {
    display: flex;
    justify-content: space-between;
    align-items: start;
    gap: 1rem;
    flex-wrap: wrap;
  }
  .eyebrow {
    display: block;
    color: #89b4fa;
    font-size: .72rem;
    text-transform: uppercase;
    letter-spacing: 0;
    margin-bottom: .2rem;
  }
  h1 {
    margin: 0 0 .2rem;
    font-size: 1.35rem;
  }
  p {
    margin: 0;
  }
  .journal-actions,
  .journal-summary,
  .journal-pager {
    display: flex;
    align-items: center;
    gap: .45rem;
    flex-wrap: wrap;
  }
  .journal-actions button,
  .journal-actions input,
  .journal-pager button {
    border: 1px solid #2a3447;
    background: #111827;
    color: #cdd6f4;
    border-radius: 4px;
    padding: .38rem .55rem;
    font: inherit;
  }
  .journal-actions button,
  .journal-pager button {
    cursor: pointer;
  }
  .journal-actions button:hover:not(:disabled),
  .journal-pager button:hover:not(:disabled) {
    border-color: #45567a;
    background: #162033;
  }
  .journal-actions button:disabled,
  .journal-pager button:disabled {
    opacity: .45;
    cursor: not-allowed;
  }
  .journal-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(320px, 1fr));
    gap: .65rem;
  }
  .journal-group {
    display: grid;
    align-content: start;
    gap: .4rem;
    min-width: 0;
  }
  .journal-group-title {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: .4rem;
    color: #bac2de;
    font-size: .78rem;
    text-transform: uppercase;
  }
  .journal-entries {
    display: grid;
    gap: .35rem;
  }
  .journal-entry {
    text-align: left;
    display: grid;
    gap: .18rem;
    width: 100%;
    border: 1px solid #1f2733;
    border-left: 3px solid #45567a;
    background: #0c1019;
    color: #cdd6f4;
    border-radius: 4px;
    padding: .46rem .55rem;
    font: inherit;
  }
  button.journal-entry {
    cursor: pointer;
  }
  button.journal-entry:hover {
    border-color: #45567a;
    background: #11161f;
  }
  .journal-changed .journal-entry { border-left-color: rgb(137,180,250); }
  .journal-research .journal-entry { border-left-color: rgb(249,226,175); }
  .journal-curious .journal-entry { border-left-color: rgb(203,166,247); }
  .journal-blocked .journal-entry { border-left-color: rgb(243,139,168); }
  .journal-crowded_or_extended .journal-entry { border-left-color: rgb(245,194,231); }
  .journal-ignored_or_hated .journal-entry { border-left-color: rgb(166,227,161); }
  .journal-entry-title {
    display: flex;
    align-items: baseline;
    gap: .35rem;
    flex-wrap: wrap;
    font-size: .86rem;
  }
  .journal-entry .muted {
    font-size: .78rem;
    line-height: 1.35;
  }
  .journal-meta {
    color: #6c7086;
    font-size: .7rem;
    text-transform: uppercase;
  }
  .badge {
    border: 1px solid #2a3447;
    border-radius: 999px;
    padding: .08rem .38rem;
    color: #bac2de;
    font-size: .7rem;
  }
  .muted {
    color: #7f849c;
  }
  .error-text {
    color: #f38ba8;
  }
  .sr-only {
    position: absolute;
    width: 1px;
    height: 1px;
    padding: 0;
    margin: -1px;
    overflow: hidden;
    clip: rect(0, 0, 0, 0);
    white-space: nowrap;
    border: 0;
  }
  @media (max-width: 720px) {
    .brain-journal-page {
      padding: .55rem;
    }
    .journal-grid {
      grid-template-columns: 1fr;
    }
  }
</style>
