# stocks — thesis-driven trading intelligence

An LLM-augmented **investment intelligence amplifier** (Product A): it maintains
synthesized per-ticker context, discovers emerging tech-infrastructure theses
before consensus, and generates trade ideas for a human to evaluate and execute
manually. Full design in **[SPEC.md](./SPEC.md)**.

> Status: **Phase 0 foundation** — repo, infra, data model, platform libs, two
> ingestion adapters, the deterministic **regime classifier**, the decision/UI
> gateway (SSE + embedded SPA), the context-maintainer skeleton, and k8s
> manifests. All event paths use **JetStream durable consumers** end-to-end
> (publish ⇒ persisted; subscribers replay on restart). Remaining service
> business logic (discovery funnel, thesis engine, risk overlay, reflection)
> is stubbed against the SPEC.

## Architecture (SPEC §3)

Event-driven services over **NATS JetStream**, state in **Postgres**, **Go** for
services, **Python** for LLM/ML/research, a **Svelte** SPA embedded in the Go
gateway, deployed on **Kubernetes**.

```
ingestion (Go) ─ingest.*→ JetStream ─→ context maintainer (Py, LLM)
   EDGAR, FRED                  │  └→ regime classifier (Go) ─regime.*→
                                ↓                                      │
                            Postgres ←── thesis engine (Py) ←──────────┘
                                              │ thesis.*  │ risk.*
                                              ↓           ↓
                                   gateway (Go): durable consumers →
                                   SSE + REST + embedded Svelte SPA
```

Streams (JetStream, file-backed): `INGEST` (ingest.*), `MARKET` (regime.*,
discovery.*), `THESIS` (thesis.*), `DECISIONS` (risk.*, decision.*), `CONTEXT`
(context.*). Each consumer is a named durable (e.g. `regime-classifier`,
`gateway-thesis-alerts`) with explicit ack and bounded redelivery.

## Layout

```
cmd/            Go binaries: gateway, ingest, regime, devpub (smoke)
internal/
  platform/     config, logging, bus (NATS+JetStream), store (pgx), llm (provider iface), subjects
  domain/       core types (thesis state machine, regime, alert)
  ingest/       adapter framework + edgar/ + fred/
  regime/       deterministic macro classifier (SPEC §4)
  gateway/      HTTP server: REST + SSE hub + SPA fallback
  web/          go:embed of the built SPA (dist/)
web/            Svelte 5 + Vite SPA source (pinned, hardened deps)
py/             Python package: config, llm, models (pydantic), context_maintainer
db/migrations/  Postgres schema (SPEC §5) + config/taxonomy seed
deploy/local/   docker-compose (Postgres + NATS) — local dev only
deploy/k8s/     kustomize base (production)
scripts/        scan-deps.mjs (supply-chain scanner)
```

## Prerequisites

Go 1.26, Node 26 / npm 11, Python 3.12+, Docker, `kubectl`/`kustomize`, `psql`.

## Local development

```bash
cp .env.example .env
make up                 # start Postgres + NATS (docker compose)
make migrate            # apply db/migrations/*.sql

# backend
make run-gateway        # :8080  (REST /api/*, SSE /api/stream, serves SPA)
make run-ingest         # EDGAR + FRED adapters → JetStream + append-only store
make run-regime         # ingest.macro → market_state + regime.state
make py-setup && make run-context

# frontend (dev, with API proxy to :8080)
make web-install        # npm ci --ignore-scripts (from lockfile)
make web-dev            # vite dev server

# or build the SPA into the gateway binary:
make web-build && make run-gateway   # SPA served at :8080
```

## Build & verify

```bash
make verify   # go vet + go build + web supply-chain scan + npm audit + k8s render
make py-check # ruff
```

## Production (Kubernetes)

```bash
make images                              # build gateway / ingest / context images
# install the CloudNativePG operator (see deploy/k8s/base/postgres.yaml header)
make k8s-apply                           # kubectl apply -k deploy/k8s/base
```
Set real image tags via the `images:` block in `deploy/k8s/base/kustomization.yaml`,
manage `stocks-secrets` with sealed-secrets/external-secrets, and point
`DATABASE_URL` at the CloudNativePG `stocks-pg-rw` service.

## JavaScript supply-chain policy (enforced)

The May-2026 npm/PyPI worm wave (TanStack, @antv→echarts-for-react, node-ipc,
@bitwarden/cli, @cap-js/*) makes this non-negotiable:

- **Minimal _runtime_ surface** — production bundle is **39 kB (15 kB gzip)**:
  only Svelte (compiled away) + uPlot + lightweight-charts; dep-free routing.
  Build-time tree ≈ **48 packages** (esbuild/rollup/vite/ts), all devDependencies.
- **Exact pins only** (`web/.npmrc` sets `save-exact=true`); committed lockfile.
- **No install scripts** (`ignore-scripts=true`; install via `npm ci --ignore-scripts`)
  — the worm executes via lifecycle scripts.
- **Scanned**: `scripts/scan-deps.mjs` hard-fails on the known-compromised set;
  `npm audit` must be clean.

**Attestation (2026-05-30):** `npm install --ignore-scripts` → 48 packages installed
(79 lockfile entries incl. optional platform binaries), `npm audit` →
**0 vulnerabilities**, supply-chain scan → none compromised. Verified compatible pins:

| package | version | note |
|---|---|---|
| svelte | 5.56.0 | runtime compiles away |
| @sveltejs/vite-plugin-svelte | 7.1.2 | peers: vite `^8`, svelte `^5.46.4` |
| vite | 8.0.14 | satisfies plugin peer |
| typescript | 5.9.3 | latest 5.x |
| svelte-check | 4.4.8 | |
| uplot | 1.6.32 | **zero dependencies** |
| lightweight-charts | 5.2.0 | TradingView (1 dep: fancy-canvas 2.1.0) |

Python deps pinned exactly (`py/pyproject.toml`, verified on PyPI):
pydantic 2.12.4, asyncpg 0.31.0, nats-py 2.12.0, ruff 0.14.4.
