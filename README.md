# stocks — thesis-driven trading intelligence

An LLM-augmented **investment intelligence amplifier** (Product A): it maintains
synthesized per-ticker context, discovers evidence-backed market inflections
before consensus, and generates trade ideas for a human to evaluate and execute
manually. Tech infrastructure is a current strength, not the product boundary:
copper, wheat, financials, staples, energy, healthcare, and any liquid market
can matter when the evidence creates a falsifiable money-making view. Full
design in **[SPEC.md](./SPEC.md)**.

> Status: active v0. The repo has the event fabric, Postgres schema, Rust
> service framework, gateway + embedded Svelte SPA, deterministic safety-net
> services, LLM prompt registry, Python cognition commands, and several data
> adapters/workflows in place. Product workflows are still being filled in
> against the SPEC and GitHub issue backlog.

## How the system works

The system is built around one repeated loop:

```text
continuous scan
  -> fresh evidence
  -> maintained ticker context
  -> structured theses
  -> risk-gated attention
  -> human decisions
  -> outcomes/reflection
  -> better future signals/prompts
```

The core product object is a **thesis**: a versioned, falsifiable state-machine
record with an edge rationale, forecast, conviction conditions, trigger
conditions, invalidation conditions, and fulfillment conditions. The system is
useful only when those objects make the operator faster and more disciplined
than manual research alone.

The background system should always scan with tiered depth:

```text
discovery_pool  broad cheap radar over possible symbols
watchlists      focused radar over symbols the operator cares about
active theses   deep monitoring, risk checks, and decision support
```

Routine scans should update evidence, context, freshness, and thesis condition
state silently. The operator should only be interrupted when the system creates
an attention item that needs judgment.

Important mental models:

- **Product A, not Product B:** the system proposes and explains; the human
  decides and executes manually.
- **Ticker context is memory:** structural, narrative, and market bands are
  stored separately, versioned, and freshness-tracked.
- **Watchlists are navigation:** tickers are browsed through watchlists,
  including a system `Universe` / `All Tickers` list.
- **Risk is independent:** the thesis engine proposes; the risk overlay
  constrains; the human decides.
- **Validation is forward:** lead-time-to-consensus, forecast calibration, and
  decision quality versus the relevant passive benchmark matter more than
  historical backtests.

See **[docs/SYSTEM.md](./docs/SYSTEM.md)** for the system model, object model,
service map, UI direction, safety model, and validation loop.
See **[docs/PRODUCT_PLAN.md](./docs/PRODUCT_PLAN.md)** for the connected brain,
discovery, cognition, thesis, decision, position, and reflection plan.

## Architecture (SPEC §3)

Event-driven services over **NATS JetStream**, state in **Postgres 17**,
**Rust** for the services, **Python** for LLM/ML/research, a **Svelte 5**
SPA embedded in the Rust gateway via `rust-embed`, deployed on
**Kubernetes**.

```
ingestion (Rust) ─ingest.*→ JetStream ─→ context maintainer (Py, LLM)
   EDGAR, FRED                  │     └→ regime classifier (Rust) ─regime.*→
                                ↓                                         │
                            Postgres ←── thesis engine (Py, LLM) ←───────┘
                                              │ thesis.*  │ risk.*
                                              ↓           ↓
                                   gateway (Rust): durable consumers →
                                   SSE + REST + embedded Svelte SPA
```

Streams (JetStream, file-backed): `INGEST` (ingest.*), `MARKET` (regime.*,
discovery.*), `THESIS` (thesis.*), `DECISIONS` (risk.*, decision.*), `TICKER`
(route.ticker.>), `CONTEXT` (context.*). Each consumer is a named durable
(e.g. `regime-classifier`, `gateway-thesis-alerts`) with explicit ack and
bounded redelivery.

## Current operator workflow

Local development is usually:

```text
make dev
  -> Postgres + NATS + Rust services + Vite UI

seed or ingest ticker data
  -> refresh context
  -> draft/sharpen/challenge thesis
  -> review alerts/theses in gateway UI
  -> record decisions
```

Useful commands:

```bash
make seed-demo
make refresh-context SYMBOL=NVDA
make draft-thesis SYMBOL=NVDA
make sharpen-thesis THESIS_ID=<uuid>
make challenge-thesis THESIS_ID=<uuid>
make classify-candidates
make smoketest
```

## Layout

```
Cargo.toml      Single crate, multiple binaries (gateway, ingest, regime,
                router, risk, goalpost, evaluator, discovery, reflection,
                staler, consensus, devpub) sharing a library.
src/
  platform/     bus (async-nats+JetStream), store (sqlx), config (env),
                subjects, domain (enums w/ serde), logging (tracing JSON)
  ingest/       market/company/news/macro/crowd ingest adapters and services
  llm/          provider trait (mock | anthropic | openai_compat)
  regime/       deterministic macro classifier (SPEC §4)
  router/       fan-out: ingest.* → route.ticker.>
  risk/         risk overlay (SPEC §7) — pure Evaluate + service
  goalpost/     thesis integrity guard (SPEC §5.3)
  thesis/       substance/state-machine helpers
  gateway/      axum: REST + SSE hub + SPA fallback
  web/          rust-embed of the built Svelte SPA
  bin/          thin binary entry points for each service
web/            Svelte 5 + Vite SPA source (pinned, hardened deps)
py/             Python package: config, llm transports, prompts, context,
                thesis, challenge/sharpen/classification workflows
db/migrations/  Postgres schema (SPEC §5) + config/taxonomy seed
deploy/local/   docker-compose (Postgres + NATS) — local dev only
deploy/k8s/     kustomize base (production)
scripts/        scan-deps.mjs (supply-chain scanner)
```

## Prerequisites

Rust 1.95 (`rustup default stable`), Node 26 / npm 11, Python 3.12+, Docker,
`kubectl`/`kustomize`, `psql`.

## Local development

```bash
make dev                # full docker stack with hot reload
make dev-logs           # follow all dev-service logs
make dev-down           # stop the dev stack

# smaller infra-only loop:
make up                 # start Postgres + NATS
make migrate            # apply db/migrations/*.sql

# backend (each `make run-*` uses Infisical when available):
make run-gateway        # :8080  (REST /api/*, SSE /api/stream, serves SPA)
make run-ingest         # adapters → JetStream + append-only store
make run-regime         # ingest.macro → market_state + regime.state
make run-router         # ingest.* → route.ticker.<SYMBOL>
make run-risk           # thesis.actionable → risk.veto / risk.warning
make run-goalpost       # thesis.updated → integrity check, weakens_invalidation
make py-setup && make run-context

# frontend (dev, with API proxy to :8080)
make web-install        # bun install from lockfile with lifecycle scripts disabled
make web-dev            # vite dev server
make web-e2e            # Playwright workflow tests with mocked API

# or build the SPA into the gateway binary:
make web-build && make run-gateway   # SPA served at :8080
```

### Dev-stack health and recovery

Run `make doctor` when the UI starts returning broad 500s, Docker restarts
services unexpectedly, or Postgres refuses connections. It checks root and repo
filesystem free space, Docker disk usage, local Postgres reachability, and the
gateway diagnostics endpoint's database status.

The frontend tooling uses repo-local ignored paths by default:

```bash
web/.cache/bun-install
web/.cache/playwright-work
.runtime/
```

Recommended recovery flow:

```bash
make doctor
docker system df
docker builder prune
docker image prune
docker compose -f deploy/local/docker-compose.yml restart postgres
make web-install
make web-e2e
```

## Build & verify

```bash
make verify   # cargo check + cargo test + web supply-chain scan + npm audit + Playwright + k8s render
make py-check # ruff + pytest
```

## Production (Kubernetes)

```bash
make images                              # build TWO images: stocks (Rust) + stocks-py (Python)
# install the CloudNativePG operator (see deploy/k8s/base/postgres.yaml header)
make k8s-apply                           # kubectl apply -k deploy/k8s/base
```

**Image strategy.** One `ghcr.io/noeljackson/stocks` image contains every
Rust binary (gateway, ingest, regime, router, risk, goalpost, evaluator,
staler, consensus, discovery, reflection, devpub, llmsmoke) plus the embedded
SPA; each pod selects its entrypoint via `command:`
(`["/gateway"]`, `["/ingest"]`, …). Same supply-chain surface, one pull per
node, single SBOM. The Python services (context maintainer, future thesis
engine) ship as `stocks-py`. Override tags via the `images:` block in
`deploy/k8s/base/kustomization.yaml`; manage `stocks-secrets` with
sealed-secrets / external-secrets; point `DATABASE_URL` at the CloudNativePG
`stocks-pg-rw` service.

## Why Rust + Python (and not all-one-language)?

- **Rust** owns deterministic event processing (ingest, regime, router, risk,
  goalpost, gateway): typed enums for state machines, `Result<T, E>` for
  error chains, `async-nats` + `sqlx` + `axum` for the I/O paths. Single
  static binary, ~40 MB distroless image, end-to-end JetStream durable
  consumers with at-least-once + replay-on-restart.
- **Python** owns the LLM-driven side (context maintainer, future thesis
  engine + reflection layer). The prompt-iteration loop and eval ecosystem
  (`instructor`, `pydantic-ai`, `inspect-ai`, notebooks for calibration)
  are best-in-class in Python and would be expensive to recreate in Rust.

The boundary is the cheapest possible interface: same NATS subjects, same
JSON shapes, same Postgres schema. No FFI, no PyO3, no bindings to maintain.

## JavaScript supply-chain policy (enforced)

The May-2026 npm/PyPI worm wave (TanStack, @antv→echarts-for-react,
node-ipc, @bitwarden/cli, @cap-js/*) makes this non-negotiable:

- **Minimal _runtime_ surface** — keep production dependencies narrow and
  purposeful: Svelte compiled output, charting libraries, and focused UI
  primitives only when they replace non-trivial hand-rolled interaction code.
  See `web/package.json` for the current exact set.
- **Exact pins only** (`web/.npmrc` sets `save-exact=true`); committed lockfile.
- **No install scripts** (`ignore-scripts=true`; install via `npm ci --ignore-scripts`)
  — the worm executes via lifecycle scripts.
- **Scanned**: `scripts/scan-deps.mjs` hard-fails on the known-compromised set;
  `npm audit` must be clean.

## Rust supply-chain policy

Same rigor as the JS side: every crate in `Cargo.toml` is pinned with `=X.Y.Z`
to a version verified against crates.io's `max_stable_version` at write time.
No range pins (`^`, `~`, `>=`). The lockfile is committed. Re-verify by
re-running the loop in commit-message scripts when bumping pins.

Python deps pinned exactly (`py/pyproject.toml`, verified on PyPI):
pydantic 2.12.4, asyncpg 0.31.0, nats-py 2.12.0, httpx 0.28.1, ruff 0.14.4,
pytest 9.0.3, pytest-asyncio 1.4.0.
