<script lang="ts">
  import type {
    BrainJournal,
    BrainJournalCategory,
    BrainJournalDecisionItem,
    BrainJournalEntry,
    BrainJournalMemoEvidence,
    BrainJournalMemoSymbol,
    BrainJournalMemoTheme,
  } from "./api";

  type Props = {
    journal?: BrainJournal | null;
    date: string;
    today: string;
    loading?: boolean;
    error?: string | null;
    onDateChange?: (date: string) => void;
    onPageChange?: (page: number) => void;
    onOpenEntry?: (entry: BrainJournalEntry) => void;
    onOpenSymbol?: (symbol: string) => void;
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
    onOpenSymbol = (_symbol: string) => {},
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
  let overview = $derived(journal?.overview ?? null);
  let decisionSections = $derived.by<{ key: string; title: string; tone: string; items: BrainJournalDecisionItem[] }[]>(() => {
    const brief = overview?.decision_brief;
    if (!brief) return [];
    return [
      { key: "consider", title: "Would Consider", tone: "consider", items: brief.consider ?? [] },
      { key: "wait", title: "Would Wait", tone: "wait", items: brief.wait ?? [] },
      { key: "avoid", title: "Would Avoid", tone: "avoid", items: brief.avoid ?? [] },
      { key: "research", title: "Research First", tone: "research", items: brief.research ?? [] },
    ];
  });

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

  function label(value?: string | null): string {
    return value ? value.replace(/_/g, " ") : "unknown";
  }

  function directionLabel(value?: string | null): string {
    if (value === "up" || value === "bull" || value === "long") return "bullish";
    if (value === "down" || value === "bear" || value === "short") return "bearish";
    return label(value);
  }

  function pct(value?: number | null): string {
    if (typeof value !== "number" || !Number.isFinite(value)) return "";
    return `${value >= 0 ? "+" : ""}${value.toFixed(1)}% vs 200D`;
  }

  function symbolTone(item: BrainJournalMemoSymbol): string {
    if (item.entry_stance === "avoid_chase" || item.technical_state === "extended") return "wait";
    if (item.freshness_status === "blocked" || item.technical_state === "deteriorating") return "risk";
    return "constructive";
  }

  function decisionMeta(item: BrainJournalDecisionItem): string {
    const parts = [
      directionLabel(item.thesis_direction),
      label(item.thesis_state),
      label(item.entry_stance),
      label(item.freshness_status),
    ].filter((part) => part && part !== "unknown");
    return parts.join(" · ") || "research-only";
  }

  function decisionMetric(item: BrainJournalDecisionItem): string {
    const pctText = pct(item.technical_pct_vs_200d);
    const blockers = item.blockers?.length ? `${item.blockers.length} blocker${item.blockers.length === 1 ? "" : "s"}` : "";
    return [pctText || label(item.technical_state), blockers].filter(Boolean).join(" · ");
  }

  function decisionStatus(item: BrainJournalDecisionItem): string {
    if (item.tier) return `Universe T${item.tier}`;
    return "Universe";
  }

  function decisionAction(sectionKey: string, item: BrainJournalDecisionItem): string {
    if ((item.open_attention ?? 0) > 0) return "Open review packet";
    if (!item.thesis_id) return "Research first";
    if (sectionKey === "wait") return "Review thesis";
    if (sectionKey === "avoid") return "Open symbol";
    return "Review setup";
  }

  function themeMissing(theme: BrainJournalMemoTheme): string {
    const missing = Array.isArray(theme.missing_evidence) ? theme.missing_evidence : [];
    return missing.length ? missing.slice(0, 3).map(label).join(" · ") : "evidence current enough to read";
  }

  function evidenceTitle(item: BrainJournalMemoEvidence): string {
    if (item.title) return item.title;
    if (item.symbol && item.kind) return `${item.symbol} ${label(item.kind)}`;
    if (item.symbol) return item.symbol;
    return label(item.category ?? item.source_kind);
  }

  function evidenceTime(item: BrainJournalMemoEvidence): string {
    const t = item.observed_at ?? item.occurred_at;
    return t ? relativeTime(t) : "";
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

  {#if overview}
    <section class="trade-desk" data-testid="daily-trade-desk">
      <div class="memo-head">
        <div>
          <span class="eyebrow">Daily Trade Desk</span>
          <h2>What the system would review today</h2>
        </div>
        <span class="memo-chip">{overview.market.label}</span>
      </div>

      <div class="trade-desk-grid">
        {#each decisionSections as section (section.key)}
          <article class={`trade-section ${section.tone}`}>
            <div class="memo-panel-title">
              <strong>{section.title}</strong>
              <span>{section.items.length}</span>
            </div>
            {#if section.items.length}
              <div class="trade-items">
                {#each section.items as item, i (`${item.symbol}-${i}`)}
                  <button type="button" class="trade-item" onclick={() => onOpenSymbol(item.symbol)}>
                    <span class="memo-symbol-top">
                      <strong>{item.symbol}</strong>
                      <span>{item.score}</span>
                    </span>
                    <span class="trade-status">
                      <span>{decisionStatus(item)}</span>
                      {#if !item.thesis_id}<span>No thesis</span>{/if}
                      {#if (item.open_attention ?? 0) > 0}<span>Needs review</span>{/if}
                    </span>
                    <span>{item.why_now}</span>
                    <small>{decisionMeta(item)}</small>
                    <small>{decisionMetric(item)}</small>
                    <em>{item.why_not}</em>
                    <b>{decisionAction(section.key, item)}</b>
                  </button>
                {/each}
              </div>
            {:else}
              <p class="muted">No symbols in this bucket.</p>
            {/if}
          </article>
        {/each}
      </div>
    </section>

    <section class="journal-memo" data-testid="brain-journal-memo">
      <div class="memo-head">
        <div>
          <span class="eyebrow">Daily Brain Memo</span>
          <h2>{overview.headline}</h2>
        </div>
        <span class="memo-chip">{overview.market.label}</span>
      </div>

      <div class="memo-grid">
        <article class="memo-panel memo-market">
          <div class="memo-panel-title">
            <strong>Market Read</strong>
            <span>{label(overview.market.freshness)}</span>
          </div>
          <p>{overview.market.summary}</p>
          {#if overview.market.missing_evidence?.length}
            <div class="memo-tags">
              {#each overview.market.missing_evidence.slice(0, 5) as missing}
                <span>{label(missing)}</span>
              {/each}
            </div>
          {/if}
        </article>

        <article class="memo-panel">
          <div class="memo-panel-title">
            <strong>Top Candidates</strong>
            <span>{overview.top_candidates.length}</span>
          </div>
          {#if overview.top_candidates.length}
            <div class="memo-symbols">
              {#each overview.top_candidates as item, i (`${item.symbol}-${i}`)}
                <button type="button" class={`memo-symbol ${symbolTone(item)}`} onclick={() => onOpenSymbol(item.symbol)}>
                  <span class="memo-symbol-top">
                    <strong>{item.symbol}</strong>
                    <span>{item.score}</span>
                  </span>
                  <span>{directionLabel(item.thesis_direction)} · {label(item.thesis_state)} · {label(item.entry_stance)}</span>
                  <small>{pct(item.technical_pct_vs_200d) || label(item.technical_state)} · {label(item.freshness_status)}</small>
                </button>
              {/each}
            </div>
          {:else}
            <p class="muted">No clean candidates passed thesis, freshness, and setup gates.</p>
          {/if}
        </article>

        <article class="memo-panel">
          <div class="memo-panel-title">
            <strong>Wait For Setup</strong>
            <span>{overview.wait_for_setup.length}</span>
          </div>
          {#if overview.wait_for_setup.length}
            <div class="memo-symbols">
              {#each overview.wait_for_setup as item, i (`${item.symbol}-${i}`)}
                <button type="button" class="memo-symbol wait" onclick={() => onOpenSymbol(item.symbol)}>
                  <span class="memo-symbol-top">
                    <strong>{item.symbol}</strong>
                    <span>{item.score}</span>
                  </span>
                  <span>{directionLabel(item.thesis_direction)} thesis, but {label(item.entry_stance)}</span>
                  <small>{pct(item.technical_pct_vs_200d) || label(item.technical_state)} · not an entry read</small>
                </button>
              {/each}
            </div>
          {:else}
            <p class="muted">No bullish or active thesis is currently flagged as overextended.</p>
          {/if}
        </article>

        <article class="memo-panel">
          <div class="memo-panel-title">
            <strong>Theme Pressure</strong>
            <span>{overview.themes.length}</span>
          </div>
          {#if overview.themes.length}
            <div class="memo-list">
              {#each overview.themes.slice(0, 4) as theme, i (`${theme.name}-${i}`)}
                <div class="memo-line">
                  <strong>{theme.name}</strong>
                  <span>{label(theme.direction)} · {label(theme.state)} · {theme.linked_tickers} tickers</span>
                  <small>{themeMissing(theme)}</small>
                </div>
              {/each}
            </div>
          {:else}
            <p class="muted">No active macro, sector, or theme pressure recorded.</p>
          {/if}
        </article>

        <article class="memo-panel">
          <div class="memo-panel-title">
            <strong>News And Evidence</strong>
            <span>{overview.news_recap.length}</span>
          </div>
          {#if overview.news_recap.length}
            <div class="memo-list">
              {#each overview.news_recap.slice(0, 5) as item, i (`${item.symbol ?? "global"}-${item.kind ?? item.source_kind ?? "evidence"}-${i}`)}
                <div class="memo-line">
                  <strong>{evidenceTitle(item)}</strong>
                  <span>{item.summary}</span>
                  <small>{label(item.source ?? item.kind)} {evidenceTime(item)}</small>
                </div>
              {/each}
            </div>
          {:else}
            <p class="muted">No new high-signal news or evidence rows landed for this date.</p>
          {/if}
        </article>

        <article class="memo-panel">
          <div class="memo-panel-title">
            <strong>Research Focus</strong>
            <span>{overview.research_focus.length}</span>
          </div>
          {#if overview.research_focus.length}
            <div class="memo-list">
              {#each overview.research_focus.slice(0, 5) as item, i (`${item.source_kind ?? "focus"}-${item.source_id ?? "none"}-${i}`)}
                <div class={`memo-line focus-${item.category ?? "curious"}`}>
                  <strong>{evidenceTitle(item)}</strong>
                  <span>{item.summary}</span>
                  <small>{label(item.category)} · importance {item.importance ?? "n/a"} {evidenceTime(item)}</small>
                </div>
              {/each}
            </div>
          {:else}
            <p class="muted">No blockers or research questions recorded for this date.</p>
          {/if}
        </article>
      </div>
    </section>
  {/if}

  {#if loading}
    <p class="muted">Loading journal…</p>
  {:else if error}
    <p class="error-text">{error}</p>
  {:else if groups.length}
    <div class="journal-section-title">
      <strong>Receipts</strong>
      <span class="muted">Source events behind the memo</span>
    </div>
    <div class="journal-grid">
      {#each groups as group (group.category)}
        <section class="journal-group journal-{group.category}">
          <div class="journal-group-title">
            <strong>{categoryLabel(group.category)}</strong>
            <span class="badge">{group.entries.length}/{group.total}</span>
          </div>
          <div class="journal-entries">
            {#each group.entries as entry, i (`${entry.event_key}-${i}`)}
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
  .trade-desk,
  .journal-memo,
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
  .journal-memo {
    display: grid;
    gap: .65rem;
  }
  .trade-desk {
    display: grid;
    gap: .65rem;
  }
  .memo-head,
  .memo-panel-title,
  .memo-symbol-top,
  .journal-section-title {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: .5rem;
  }
  .memo-head {
    align-items: start;
    flex-wrap: wrap;
  }
  h2 {
    margin: 0;
    font-size: 1.05rem;
    line-height: 1.25;
  }
  .memo-chip,
  .memo-panel-title span,
  .memo-symbol-top span {
    border: 1px solid #2a3447;
    border-radius: 999px;
    padding: .12rem .42rem;
    color: #bac2de;
    font-size: .72rem;
    white-space: nowrap;
  }
  .memo-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(280px, 1fr));
    gap: .55rem;
  }
  .trade-desk-grid {
    display: grid;
    grid-template-columns: repeat(4, minmax(220px, 1fr));
    gap: .55rem;
  }
  .memo-panel {
    display: grid;
    gap: .45rem;
    align-content: start;
    min-width: 0;
    border: 1px solid #1f2733;
    background: #0c1019;
    border-radius: 4px;
    padding: .55rem;
  }
  .memo-market {
    border-left: 3px solid #89b4fa;
  }
  .trade-section {
    display: grid;
    align-content: start;
    gap: .45rem;
    min-width: 0;
    border: 1px solid #1f2733;
    background: #0c1019;
    border-radius: 4px;
    padding: .55rem;
    border-left: 3px solid #45567a;
  }
  .trade-section.consider { border-left-color: #a6e3a1; }
  .trade-section.wait { border-left-color: #f9e2af; }
  .trade-section.avoid { border-left-color: #f38ba8; }
  .trade-section.research { border-left-color: #89b4fa; }
  .memo-panel-title {
    color: #bac2de;
    font-size: .78rem;
    text-transform: uppercase;
  }
  .memo-panel p {
    color: #a6adc8;
    font-size: .82rem;
    line-height: 1.4;
  }
  .memo-tags {
    display: flex;
    flex-wrap: wrap;
    gap: .25rem;
  }
  .memo-tags span {
    border: 1px solid #2a3447;
    border-radius: 999px;
    padding: .08rem .36rem;
    color: #9399b2;
    font-size: .7rem;
  }
  .memo-symbols,
  .memo-list,
  .trade-items {
    display: grid;
    gap: .35rem;
  }
  .memo-symbol,
  .trade-item,
  .memo-line {
    display: grid;
    gap: .15rem;
    width: 100%;
    border: 1px solid #1f2733;
    border-left: 3px solid #45567a;
    background: #080c13;
    color: #cdd6f4;
    border-radius: 4px;
    padding: .42rem .5rem;
    text-align: left;
    font: inherit;
    min-width: 0;
  }
  button.memo-symbol {
    cursor: pointer;
  }
  button.memo-symbol:hover,
  button.trade-item:hover {
    border-color: #45567a;
    background: #101723;
  }
  .trade-item {
    cursor: pointer;
  }
  .trade-status {
    display: flex;
    flex-wrap: wrap;
    gap: .25rem;
  }
  .trade-status span {
    border: 1px solid #2a3447;
    border-radius: 999px;
    padding: .08rem .36rem;
    color: #bac2de;
    font-size: .68rem;
    line-height: 1.2;
  }
  .memo-symbol.constructive {
    border-left-color: #a6e3a1;
  }
  .memo-symbol.wait,
  .memo-line.focus-research {
    border-left-color: #f9e2af;
  }
  .memo-symbol.risk,
  .memo-line.focus-blocked {
    border-left-color: #f38ba8;
  }
  .memo-symbol span,
  .trade-item span,
  .trade-item em,
  .memo-line span,
  .memo-line small,
  .trade-item small,
  .memo-symbol small {
    overflow-wrap: anywhere;
  }
  .memo-symbol > span:not(.memo-symbol-top),
  .trade-item > span:not(.memo-symbol-top),
  .memo-line span {
    color: #a6adc8;
    font-size: .78rem;
    line-height: 1.35;
  }
  .memo-symbol small,
  .trade-item small,
  .memo-line small {
    color: #7f849c;
    font-size: .7rem;
    line-height: 1.25;
  }
  .trade-item em {
    color: #9399b2;
    font-size: .72rem;
    font-style: normal;
    line-height: 1.3;
  }
  .trade-item b {
    color: #a6e3a1;
    font-size: .72rem;
    font-weight: 700;
  }
  .journal-section-title {
    color: #bac2de;
    font-size: .78rem;
    text-transform: uppercase;
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
    .memo-grid {
      grid-template-columns: 1fr;
    }
    .trade-desk-grid {
      grid-template-columns: 1fr;
    }
    .memo-head,
    .memo-panel-title,
    .journal-section-title {
      align-items: start;
    }
  }
</style>
