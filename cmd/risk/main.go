// Command risk runs the deterministic risk overlay (SPEC §7).
package main

import (
	"context"
	"os"
	"os/signal"
	"syscall"

	"github.com/noeljackson/stocks/internal/platform/bus"
	"github.com/noeljackson/stocks/internal/platform/config"
	"github.com/noeljackson/stocks/internal/platform/logging"
	"github.com/noeljackson/stocks/internal/platform/store"
	"github.com/noeljackson/stocks/internal/risk"
)

func main() {
	cfg := config.Load()
	log := logging.New("risk")
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

	if err := risk.New(db, b, log).Run(ctx); err != nil {
		log.Error("risk", "err", err)
		os.Exit(1)
	}
}
