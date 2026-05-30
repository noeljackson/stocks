// Command ingest runs the ingestion adapters (EDGAR + FRED for the MVP).
package main

import (
	"context"
	"os"
	"os/signal"
	"syscall"

	"github.com/noeljackson/stocks/internal/ingest"
	"github.com/noeljackson/stocks/internal/ingest/edgar"
	"github.com/noeljackson/stocks/internal/ingest/fred"
	"github.com/noeljackson/stocks/internal/platform/bus"
	"github.com/noeljackson/stocks/internal/platform/config"
	"github.com/noeljackson/stocks/internal/platform/logging"
	"github.com/noeljackson/stocks/internal/platform/store"
	"github.com/noeljackson/stocks/internal/platform/subjects"
)

func main() {
	cfg := config.Load()
	log := logging.New("ingest")
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()

	db, err := store.Open(ctx, cfg.DatabaseURL)
	if err != nil {
		log.Error("db open", "err", err)
		os.Exit(1)
	}
	defer db.Close()

	b, err := bus.Connect(cfg.NATSURL)
	if err != nil {
		log.Error("nats connect", "err", err)
		os.Exit(1)
	}
	defer b.Close()

	if err := b.EnsureStream(ctx, subjects.StreamIngest, "ingest.*"); err != nil {
		log.Error("ensure stream", "err", err)
		os.Exit(1)
	}

	r := &ingest.Runner{DB: db, Bus: b, Log: log}
	log.Info("ingestion started")
	r.Run(ctx, edgar.New(cfg.SECUserAgent), fred.New(cfg.FREDAPIKey))
}
