.DEFAULT_GOAL := help
SHELL := /bin/bash

# ---- config ----
COMPOSE := docker compose -f deploy/local/docker-compose.yml --env-file .env
PSQL_URL ?= $(shell grep -E '^DATABASE_URL=' .env 2>/dev/null | cut -d= -f2-)

.PHONY: help
help: ## List targets
	@grep -E '^[a-zA-Z0-9_-]+:.*?## ' $(MAKEFILE_LIST) | sort | awk 'BEGIN{FS=":.*?## "}{printf "  \033[36m%-18s\033[0m %s\n", $$1, $$2}'

# ---- local infra ----
.PHONY: up down logs migrate psql nuke
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
.PHONY: web-install web-audit web-scan web-build web-dev
web-install: ## Install pinned deps with NO lifecycle scripts (from lockfile)
	cd web && npm ci --ignore-scripts

web-audit: ## Vulnerability audit
	cd web && npm audit --audit-level=moderate

web-scan: ## Fail if any known-compromised (May 2026 wave) package is present
	@cd web && node ../scripts/scan-deps.mjs

web-build: ## Build SPA into internal/web/dist (embedded by gateway via rust-embed)
	cd web && npm run build

web-dev: ## Vite dev server
	cd web && npm run dev

# ---- run (local dev; build once with `make build`, then ./target/release/<bin>) ----
.PHONY: run-gateway run-ingest run-regime run-router run-risk run-goalpost
run-gateway: ## Run the gateway (build first with `make build`)
	cargo run --release --bin gateway

run-ingest: ## Run the ingestion runner (EDGAR + FRED)
	cargo run --release --bin ingest

run-regime: ## Run the macro regime classifier
	cargo run --release --bin regime

run-router: ## Run the event router (ingest.* → route.ticker.*)
	cargo run --release --bin router

run-risk: ## Run the risk overlay (thesis.actionable → risk.veto/warning)
	cargo run --release --bin risk

run-goalpost: ## Run the goalpost detector (thesis.updated → integrity check)
	cargo run --release --bin goalpost

# ---- Python ----
.PHONY: py-setup py-check run-context
py-setup: ## Create venv + install pinned python deps
	cd py && python3 -m venv .venv && .venv/bin/python -m pip install -e ".[dev]"

py-check: ## Ruff lint + pytest
	cd py && .venv/bin/ruff check src tests
	cd py && .venv/bin/pytest -q

run-context: ## Run the Python context-maintainer
	cd py && .venv/bin/python -m stocks.context_maintainer

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
verify: check test web-scan web-audit k8s-render ## Build + lint + scan + tests + manifest render
	@echo "VERIFY OK"
