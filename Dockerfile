# syntax=docker/dockerfile:1
#
# Single Go image: builds the Svelte SPA, embeds it into the gateway binary
# via go:embed, then builds ALL Go binaries (gateway, ingest, regime, router,
# risk, devpub) into one distroless image. Each k8s pod / compose service
# picks its own entrypoint with `command:` — same image, same supply-chain
# surface, one pull per node.

FROM node:26-alpine AS web
WORKDIR /app/web
COPY web/package.json web/package-lock.json web/.npmrc ./
RUN npm ci --ignore-scripts
COPY web/ ./
RUN npm run build            # → /app/internal/web/dist (vite outDir)

FROM golang:1.26 AS build
WORKDIR /app
COPY go.mod go.sum ./
RUN go mod download
COPY . .
COPY --from=web /app/internal/web/dist ./internal/web/dist
# Build every Go binary in cmd/ under /out, one per command directory.
RUN CGO_ENABLED=0 go build -trimpath -o /out/ ./cmd/...

FROM gcr.io/distroless/static-debian12:nonroot
COPY --from=build /out/* /
USER nonroot
EXPOSE 8080
# No ENTRYPOINT — set `command:` per pod (e.g. ["/gateway"], ["/ingest"]).
