# AGENTS.md

> Symlinked from `CLAUDE.md`. Both names auto-load in Claude Code sessions.

## Operating mode

**You can work autonomously.** Don't stop at natural seams — keep going. Specifically:

- After landing a PR, **branch immediately** for the next issue. Don't ask "what's next?" — pick the next-highest-leverage open issue and start.
- After live-verifying a change in the docker dev stack, **commit and PR it**. Don't wait for review unless something is genuinely ambiguous.
- After hitting an honest unknown that requires the user's input (an API key, a product decision, an external account), **surface the ask cleanly and stop only then**.
- Merge your own PRs when they're tested, live-verified, and properly close their issue. Squash-merges keep history linear.
- If you discover a real gap or design issue mid-stream, file it as a `gh issue create` with the appropriate labels, then either continue with current work or pivot if the new issue is genuinely blocking.
- **The user trusts your judgment on scope.** Stack PRs when natural; bundle related changes into one PR when they're truly inseparable. Don't optimise for "small PR per issue" if it creates artificial seams.

## What requires the user (the only true stops)

1. **External API keys** the system depends on (z.ai, FMP, IBKR, etc). Add the secret name + signup URL to Infisical dev env, then stop with a direct ask.
2. **Architectural decisions** you genuinely cannot resolve from SPEC + memory + judgment. Make the call yourself first; only ask if you'd be guessing on something user-specific (their broker, their risk tolerance, their workflow preference).
3. **Destructive operations on shared state** — force-pushes to main, dropping prod tables, rewriting public git history.

Everything else: keep moving.

## Project layout (essentials)

- `SPEC.md` — design doc. Single source of truth for what the system is supposed to do.
- `Cargo.toml` + `src/` — one Rust crate, multiple binaries (gateway/ingest/regime/router/risk/goalpost/staler/devpub/llmsmoke). Shared lib in `src/platform/`, `src/llm/`, `src/thesis/`.
- `py/` — Python services (context maintainer, thesis engine). Shared `prompts/*.md` registry with Rust.
- `prompts/*.md` — first-class prompt registry. Filename stem = prompt name. SHA-256 of content = version hash. Every LLM call records `(prompt_name, prompt_hash, tokens, latency)` to the `llm_invocation` table.
- `db/migrations/` — Postgres schema. Numbered, sequential, idempotent re-run via `make migrate`.
- `deploy/local/docker-compose.yml` (infra) + `docker-compose.dev.yml` (full dev stack with hot reload).
- `deploy/k8s/base/` — Kustomize manifests for production.
- `web/` — Svelte 5 SPA. Built into `web/dist/` and embedded into the gateway binary at compile time via `rust-embed`. Dev mode: Vite HMR at `:5173`.

## Conventions (don't violate)

### Secrets
Live in **Infisical** (`dev` env). Never `.env`. Run things through `infisical run --env=dev -- <cmd>` — the `Makefile`'s `RUN` variable does this automatically for `run-*` and `dev` targets. `.env.example` is documentation only; user's local `.env` is gitignored and not required (compose has shell-defaults built in).

### TDD
Write the failing test first. Extract pure logic from I/O paths so it's unit-testable. Cover degraded inputs (missing data, malformed payloads, ties) not just happy paths. See `src/thesis/substance.rs::tests` for the shape — pure function + comprehensive case coverage.

### Dependency pinning
Every Rust crate and Python package pinned with `=X.Y.Z` to a version **verified against crates.io / PyPI at write time**. No range pins. Lockfiles committed.

### Prompts
Edit `prompts/*.md` to iterate on system behaviour. Hash changes on every save, audit row records which version produced which output. Keep prompts in the repo; never inline LLM prompts in code.

### LLM calls
Always go through `llm::prompts::invoke()` (Rust) or `stocks.prompts.invoke()` (Python). Never call providers directly — bypasses the audit trail.

### State machine
Use `thesis::substance::promotion_allowed()` to gate state transitions. Don't update `thesis.state` directly. The transition endpoint at `POST /api/theses/:id/transition` is the canonical path.

### Append-only
`ingest_event`, `thesis_state_history`, `thesis_version_history`, `decision`, `outcome` are all append-only. Never `UPDATE` them.

### Edge framing (from SPEC §0)
The system's value is being **earlier than the late retail crowd / passive tech beta** at spotting tech-infrastructure inflections via information diffusion. **Not** out-forecasting institutions; **not** beating Renaissance. Target ~9-13% annualised over QQQ/SMH. Validation is forward-only (lead-time-to-consensus + forecast calibration); backtesting is not a primary requirement.

## Build / test / run

```bash
make dev              # full docker stack with hot reload (gateway :8080→5173, vite :5173, all services)
make build            # release build of all Rust binaries
make test             # cargo test (Rust)
make py-check         # ruff + pytest (Python)
make verify           # vet + build + test + supply-chain scan + k8s render

make refresh-context SYMBOL=NVDA   # synth ticker_context via LLM
make draft-thesis    SYMBOL=NVDA   # draft thesis from latest context (or decline honestly)
make llmsmoke MSG="..."             # one-shot LLM round-trip with audit row

make migrate          # apply pending SQL migrations
make psql             # open psql against local DB
make seed-demo        # seed 8 sample tickers (no theses — those come from the LLM)
```

## Work tracking

GitHub Issues at `noeljackson/stocks` are the source of truth. Labels:
- `tier-1` show-stopper, `tier-2` notable, `tier-3` small
- `cognition` LLM-driven, `safety-net` deterministic guard
- `data-source`, `validation`, `infra`, `ui`, `epic`

When picking work: pick the highest-tier open issue with no unresolved blockers. Cascade order is enforced by the issue bodies' "Blocked by #N" / "Stacked on #N" lines.

## When in doubt

- **"Is this bullshit?"** → can the LLM/system be wrong about it in a measurable way? If no, it's bullshit.
- **"Is this a thesis?"** → does it have falsifiable conditions with `target` + `deadline_at` + `evidence_source`? If no, it's a vibe.
- **"Should I build this safety net?"** → does it block a specific action, or just surface information? Information-only safety nets become fancy information. Action-blocking ones earn their keep.
- **"Should I commit?"** → are the tests green AND did you live-verify in the dev stack? If both yes, commit. If neither, don't.
