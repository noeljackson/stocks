.DEFAULT_GOAL := help
SHELL := /bin/bash

# ---- config ----
# Compose has shell-style defaults built in (POSTGRES_USER etc.) so no env-file
# needed. Secrets live in Infisical, never in .env.
COMPOSE := docker compose -f deploy/local/docker-compose.yml
COMPOSE_DEV := docker compose \
    -f deploy/local/docker-compose.yml \
    -f deploy/local/docker-compose.dev.yml
COMPOSE_FIRECRAWL := $(COMPOSE_DEV) -f deploy/local/docker-compose.firecrawl.yml
PSQL_URL ?= postgres://stocks:stocks_dev_only@localhost:5432/stocks?sslmode=disable
PLAYWRIGHT_WORKDIR ?= $(CURDIR)/web/.cache/playwright-work
BUN_INSTALL_CACHE_DIR ?= $(CURDIR)/web/.cache/bun-install
STOCKS_RUNTIME_DIR ?= $(CURDIR)/.runtime

# Secrets injector: when `infisical` is on PATH, wrap commands so the binaries
# get vars from your Infisical project (env defaults to `dev`; override with
# `make INFISICAL_ENV=stage run-gateway`). Falls through to local .env via
# dotenvy when infisical is absent. Force-skip with `make RUN= run-gateway`.
INFISICAL_ENV ?= dev
RUN ?= $(shell command -v infisical >/dev/null 2>&1 && echo "infisical run --env=$(INFISICAL_ENV) --")

.PHONY: help
help: ## List targets
	@grep -E '^[a-zA-Z0-9_-]+:.*?## ' $(MAKEFILE_LIST) | sort | awk 'BEGIN{FS=":.*?## "}{printf "  \033[36m%-18s\033[0m %s\n", $$1, $$2}'

# ---- local infra ----
.PHONY: up down logs migrate psql nuke doctor
up: ## Start local Postgres+NATS (docker compose)
	$(COMPOSE) up -d
	@echo "Postgres :5432  NATS :4222 (mon :8222)"

down: ## Stop local infra (keep volumes)
	$(COMPOSE) down

nuke: ## Stop local infra and DELETE volumes
	$(COMPOSE) down -v

logs: ## Tail infra logs
	$(COMPOSE) logs -f

migrate: ## Apply db/migrations/*.sql in order (idempotent re-run)
	@for f in db/migrations/*.sql; do echo "applying $$f"; psql "$(PSQL_URL)" -v ON_ERROR_STOP=1 -f "$$f"; done

psql: ## Open psql against local db
	psql "$(PSQL_URL)"

doctor: ## Check local disk, Docker, Postgres, and gateway database reachability
	PSQL_URL="$(PSQL_URL)" PLAYWRIGHT_WORKDIR="$(PLAYWRIGHT_WORKDIR)" BUN_INSTALL_CACHE_DIR="$(BUN_INSTALL_CACHE_DIR)" STOCKS_RUNTIME_DIR="$(STOCKS_RUNTIME_DIR)" ./scripts/doctor.sh

# ---- all-in-docker dev environment (#36) ----
# Brings up postgres + nats + ALL rust services + vite SPA dev server, each
# with hot-reload on source changes. Secrets injected via Infisical.
.PHONY: dev dev-down dev-logs dev-build dev-restart firecrawl-up firecrawl-enable firecrawl-down firecrawl-logs
dev: dev-warm ## Start the full dev stack (postgres + nats + 6 rust services + vite) with hot reload
	$(RUN) $(COMPOSE_DEV) up -d
	@echo
	@echo "✓ dev stack up"
	@echo "  UI (HMR):      http://localhost:5173"
	@echo "  API:           http://localhost:8080  (/ redirects to 5173 in dev mode)"
	@echo "  NATS monitor:  http://localhost:8222"
	@echo "  Postgres:      psql 'postgresql://stocks:stocks_dev_only@localhost:5432/stocks'"
	@echo
	@echo "  Tail logs:     make dev-logs"
	@echo "  Stop:          make dev-down"

# Pre-warm cargo cache + target dir SERIALLY before starting the 6 service
# containers — otherwise they race on the cargo registry and corrupt it.
# One-time cost (~3-5 min cold). After this, cargo-watch incrementals are fast.
.PHONY: dev-warm
dev-warm: ## Pre-build all Rust binaries serially (avoids cargo cache races)
	$(COMPOSE_DEV) build
	@echo "warming cargo cache — first build, ~3-5min..."
	$(COMPOSE_DEV) up -d postgres nats
	$(COMPOSE_DEV) run --rm --no-deps gateway cargo build --bins
	@echo "✓ cargo cache warm"

dev-down: ## Stop the dev stack (keep volumes — cargo cache survives)
	$(COMPOSE_DEV) down

dev-logs: ## Follow logs from all dev services
	$(COMPOSE_DEV) logs -f

dev-build: ## Rebuild the dev images (Rust + Vite). Run after Cargo.toml changes.
	$(COMPOSE_DEV) build

dev-restart: ## Restart one service (SVC=gateway make dev-restart)
	$(COMPOSE_DEV) restart $(SVC)

firecrawl-up: ## Start optional local Firecrawl API sidecar on :3002
	$(RUN) $(COMPOSE_FIRECRAWL) up -d firecrawl-api
	@echo "Firecrawl API: http://localhost:$${FIRECRAWL_PORT:-3002}"
	@echo "Queue UI:      http://localhost:$${FIRECRAWL_PORT:-3002}/admin/$${FIRECRAWL_BULL_AUTH_KEY:-stocks-dev}/queues"

firecrawl-enable: ## Start Firecrawl and recreate cognition with RESEARCH_PROVIDER including firecrawl
	$(RUN) env RESEARCH_PROVIDER=gdelt,bing_news,firecrawl $(COMPOSE_FIRECRAWL) up -d --force-recreate firecrawl-api cognition
	@echo "Firecrawl API: http://localhost:$${FIRECRAWL_PORT:-3002}"
	@echo "Cognition:     RESEARCH_PROVIDER=gdelt,bing_news,firecrawl"

firecrawl-down: ## Stop optional local Firecrawl sidecar without stopping the stocks dev stack
	$(COMPOSE_FIRECRAWL) stop firecrawl-api firecrawl-playwright firecrawl-redis firecrawl-rabbitmq firecrawl-postgres
	$(COMPOSE_FIRECRAWL) rm -f firecrawl-api firecrawl-playwright firecrawl-redis firecrawl-rabbitmq firecrawl-postgres

firecrawl-logs: ## Follow optional local Firecrawl logs
	$(COMPOSE_FIRECRAWL) logs -f firecrawl-api firecrawl-playwright firecrawl-rabbitmq firecrawl-postgres

seed-demo: ## Seed sample tickers + theses so the UI has content on first load
	PSQL_URL="$(PSQL_URL)" ./scripts/seed-demo.sh

# ---- demo: bring up everything and seed it, ready to open localhost:8080 ----
.PHONY: demo demo-up
demo-up: up migrate seed-demo ## Start infra, apply migrations, seed demo data
	@echo
	@echo "✓ infra up + schema + sample data"
	@echo "  next: in separate terminals, run:"
	@echo "    make run-gateway     # serves the SPA at http://localhost:8080"
	@echo "    make run-regime      # macro classifier"
	@echo "    make run-risk        # risk overlay"
	@echo "    make run-goalpost    # thesis integrity guard"
	@echo "    make run-router      # ingest fan-out"
	@echo "    make run-ingest      # market/company/news/macro ingest"

# ---- Rust ----
.PHONY: build test check fmt clippy
build: ## cargo build --release (all binaries into target/release/)
	cargo build --release

test: ## cargo test (lib + integration)
	cargo test

check: ## cargo check (fast)
	cargo check

fmt: ## cargo fmt
	cargo fmt --all

clippy: ## cargo clippy on all targets, deny warnings
	cargo clippy --all-targets -- -D warnings

# ---- Frontend (supply-chain hardened) ----
.PHONY: web-install web-audit web-scan web-build web-dev web-e2e
web-install: ## Install pinned deps with NO lifecycle scripts (from lockfile)
	BUN_INSTALL_CACHE_DIR="$(BUN_INSTALL_CACHE_DIR)" ./scripts/web-preflight.sh

web-audit: ## Vulnerability audit
	cd web && bun audit --audit-level=moderate

web-scan: ## Fail if any known-compromised (May 2026 wave) package is present
	@cd web && bun ../scripts/scan-deps.mjs

web-build: ## Build SPA into internal/web/dist (embedded by gateway via rust-embed)
	cd web && bun run build

web-dev: ## Vite dev server
	cd web && bun run dev

web-e2e: ## Playwright UI workflow tests (mocked API, no DB mutation)
	@mkdir -p "$(PLAYWRIGHT_WORKDIR)"
	cd web && TMPDIR="$(PLAYWRIGHT_WORKDIR)" ./node_modules/.bin/playwright install chromium-headless-shell
	cd web && TMPDIR="$(PLAYWRIGHT_WORKDIR)" bun run test:e2e

# ---- run (local dev; build once with `make build`, then ./target/release/<bin>) ----
# $(RUN) injects infisical when installed (see top of file). Override with
# `make RUN= run-gateway` to bypass.
.PHONY: run-gateway run-ingest run-regime run-router run-risk run-goalpost llmsmoke
run-gateway: ## Run the gateway
	$(RUN) cargo run --release --bin gateway

run-ingest: ## Run the market/company/news/macro ingestion runner
	$(RUN) cargo run --release --bin ingest

run-regime: ## Run the macro regime classifier
	$(RUN) cargo run --release --bin regime

run-router: ## Run the event router (ingest.* → route.ticker.>)
	$(RUN) cargo run --release --bin router

run-risk: ## Run the risk overlay (thesis.actionable → risk.veto/warning)
	$(RUN) cargo run --release --bin risk

run-goalpost: ## Run the goalpost detector (thesis.updated → integrity check)
	$(RUN) cargo run --release --bin goalpost

run-staler: ## Run the staleness service (past-deadline conditions → risk.warning)
	$(RUN) cargo run --release --bin staler

run-evaluator: ## Run the condition evaluator (resolves targets → satisfied/refuted)
	$(RUN) cargo run --release --bin evaluator

run-consensus: ## Run the consensus computation (SPEC §6.2, exit + lead-time anchor)
	$(RUN) cargo run --release --bin consensus

run-discovery: ## Run the discovery scanner (cheap-wide signals → discovery_candidate)
	$(RUN) cargo run --release --bin discovery

run-reflection: ## Run the reflection service (predictions → outcomes → calibration)
	$(RUN) cargo run --release --bin reflection

llmsmoke: ## One-shot LLM round-trip — picks transport from env (mock if no key)
	$(RUN) cargo run --release --bin llmsmoke -- "$(MSG)"

# ---- watch (auto-rebuild + restart on source change) ----
# Requires cargo-watch (`cargo install cargo-watch`). Each watch-* target
# re-execs the binary on every relevant file change.
.PHONY: watch-gateway watch-ingest watch-regime watch-router watch-risk watch-goalpost watch-all
watch-gateway: ## Auto-rebuild+restart gateway on src/ + web/dist/ changes
	$(RUN) cargo watch -w src -w web/dist -w Cargo.toml \
	    -x 'run --release --bin gateway'

watch-ingest: ## Auto-restart ingest on src/ changes
	$(RUN) cargo watch -w src -w Cargo.toml -x 'run --release --bin ingest'

watch-regime: ## Auto-restart regime on src/ changes
	$(RUN) cargo watch -w src -w Cargo.toml -x 'run --release --bin regime'

watch-router: ## Auto-restart router on src/ changes
	$(RUN) cargo watch -w src -w Cargo.toml -x 'run --release --bin router'

watch-risk: ## Auto-restart risk on src/ changes
	$(RUN) cargo watch -w src -w Cargo.toml -x 'run --release --bin risk'

watch-goalpost: ## Auto-restart goalpost on src/ changes
	$(RUN) cargo watch -w src -w Cargo.toml -x 'run --release --bin goalpost'

# Watch-all uses GNU parallel if available, else gives instructions for tmux.
watch-all: ## Show how to run all services in watch mode
	@echo "Run each in a separate terminal (or tmux pane):"
	@echo "  make watch-gateway   # :8080"
	@echo "  make watch-regime"
	@echo "  make watch-router"
	@echo "  make watch-risk"
	@echo "  make watch-goalpost"
	@echo "  make watch-ingest    # only when actively iterating on adapter code"

# ---- Python ----
.PHONY: py-setup py-check run-context research
py-setup: ## Create venv + install pinned python deps
	cd py && python3 -m venv .venv && .venv/bin/python -m pip install -e ".[dev]"

py-check: ## Ruff lint + pytest
	cd py && .venv/bin/ruff check src tests
	cd py && .venv/bin/pytest -q

refresh-context: ## Refresh ticker_context for one symbol (SYMBOL=NVDA make refresh-context)
	cd py && $(RUN) .venv/bin/python -m stocks.context_maintainer $(SYMBOL) $(if $(LIMIT),--limit $(LIMIT))

research: ## Run product/theme web research for one symbol (SYMBOL=AMD make research)
	cd py && $(RUN) .venv/bin/python -m stocks.research $(SYMBOL) $(if $(FORCE),--force)

source-tasks: ## Run due Python-owned source tasks once (LIMIT=5)
	cd py && $(RUN) .venv/bin/python -m stocks.source_tasks --once $(if $(LIMIT),--limit $(LIMIT))

draft-thesis: ## Draft a thesis from the latest ticker_context (SYMBOL=NVDA make draft-thesis)
	cd py && $(RUN) .venv/bin/python -m stocks.thesis_engine $(SYMBOL)

sharpen-thesis: ## Sharpen pass: LLM proposes structured conditions (THESIS_ID=<uuid> make sharpen-thesis)
	cd py && $(RUN) .venv/bin/python -m stocks.sharpen $(THESIS_ID)

challenge-thesis: ## Challenge pass: LLM finds weak spots, flags for review (THESIS_ID=<uuid>)
	cd py && $(RUN) .venv/bin/python -m stocks.challenge $(THESIS_ID)

classify-candidates: ## Classify all pending discovery candidates → proposed watchlist(s) (#55)
	cd py && $(RUN) .venv/bin/python -m stocks.classify --all

classify-candidate: ## Classify one pending candidate (CANDIDATE_ID=N make classify-candidate)
	cd py && $(RUN) .venv/bin/python -m stocks.classify --candidate-id $(CANDIDATE_ID)

smoketest: ## Walk one ticker through the whole pipeline; exit non-zero on first stage that fails (#60)
	./scripts/smoketest.sh

# Legacy alias (will be removed when context_maintainer becomes a long-running service).
run-context: refresh-context ## Alias of refresh-context (deprecated)

# ---- docker images ----
.PHONY: images
images: ## Build container images: one Rust image for all services, one Python image
	docker build -t ghcr.io/noeljackson/stocks:dev .
	docker build -f Dockerfile.py -t ghcr.io/noeljackson/stocks-py:dev .

# ---- k8s (production) ----
.PHONY: k8s-render k8s-apply
k8s-render: ## Render k8s manifests (skips with note if kubectl/kustomize absent)
	@if command -v kubectl >/dev/null 2>&1; then \
	   kubectl kustomize deploy/k8s/base; \
	elif command -v kustomize >/dev/null 2>&1; then \
	   kustomize build deploy/k8s/base; \
	else \
	   echo "k8s-render: kubectl/kustomize not installed, skipping"; \
	fi

k8s-apply: ## Apply to the current kube-context
	kubectl apply -k deploy/k8s/base

# ---- verify everything ----
.PHONY: verify
verify: check test web-scan web-audit web-e2e k8s-render ## Build + lint + scan + tests + manifest render
	@echo "VERIFY OK"
