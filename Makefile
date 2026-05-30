.DEFAULT_GOAL := help
SHELL := /bin/bash

# ---- config ----
COMPOSE := docker compose -f deploy/local/docker-compose.yml --env-file .env
PSQL_URL ?= $(shell grep -E '^DATABASE_URL=' .env 2>/dev/null | cut -d= -f2-)

.PHONY: help
help: ## List targets
	@grep -E '^[a-zA-Z0-9_-]+:.*?## ' $(MAKEFILE_LIST) | sort | awk 'BEGIN{FS=":.*?## "}{printf "  \033[36m%-18s\033[0m %s\n", $$1, $$2}'

# ---- local infra ----
.PHONY: up down logs migrate psql
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

# ---- Go ----
.PHONY: go-deps go-build go-vet go-test
go-deps: ## Resolve Go deps (writes go.sum with checksums)
	go get github.com/jackc/pgx/v5@latest github.com/nats-io/nats.go@latest
	go mod tidy

go-build: ## Build all Go binaries into ./bin
	mkdir -p bin
	go build -o bin/ ./cmd/...

go-vet: ## Static checks
	go vet ./...

go-test: ## Run Go tests
	go test ./...

# ---- Frontend (supply-chain hardened) ----
.PHONY: web-install web-audit web-scan web-build web-dev
web-install: ## Install pinned deps with NO lifecycle scripts (from lockfile)
	cd web && npm ci --ignore-scripts

web-audit: ## Vulnerability audit
	cd web && npm audit --audit-level=moderate

web-scan: ## Fail if any known-compromised (May 2026 wave) package is present
	@cd web && node ../scripts/scan-deps.mjs

web-build: ## Build SPA into internal/web/dist (embedded by gateway)
	cd web && npm run build

web-dev: ## Vite dev server
	cd web && npm run dev

# ---- run ----
.PHONY: run-gateway run-ingest run-regime run-router run-risk
run-gateway: ## Run the decision/alert + UI gateway
	go run ./cmd/gateway

run-ingest: ## Run the ingestion runner (EDGAR + FRED)
	go run ./cmd/ingest

run-regime: ## Run the deterministic macro regime classifier
	go run ./cmd/regime

run-router: ## Run the event router (ingest.* → route.ticker.*)
	go run ./cmd/router

run-risk: ## Run the risk overlay (thesis.actionable → risk.veto/warning)
	go run ./cmd/risk

run-goalpost: ## Run the goalpost detector (thesis.updated → integrity check)
	go run ./cmd/goalpost

# ---- Python ----
.PHONY: py-setup py-check run-context
py-setup: ## Create venv + install pinned python deps
	cd py && python3 -m venv .venv && .venv/bin/python -m pip install -e ".[dev]"

py-check: ## Ruff lint python
	cd py && .venv/bin/ruff check src

run-context: ## Run the Python context-maintainer
	cd py && .venv/bin/python -m stocks.context_maintainer

# ---- docker images ----
.PHONY: images
images: ## Build container images: one Go image for all services, one Python image
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
verify: go-vet go-build go-test web-scan web-audit k8s-render ## Build + lint + scan + tests + manifest render
	@echo "VERIFY OK"
