// Command goalpost runs the thesis-integrity guard (SPEC §5.3).
// Subscribes to thesis.updated, diffs against immutable_original, and
// emits a risk.warning if invalidation has been weakened or needs review.
package main

import (
	"context"
	"os"
	"os/signal"
	"syscall"

	"github.com/noeljackson/stocks/internal/goalpost"
	"github.com/noeljackson/stocks/internal/platform/bus"
	"github.com/noeljackson/stocks/internal/platform/config"
	"github.com/noeljackson/stocks/internal/platform/logging"
	"github.com/noeljackson/stocks/internal/platform/store"
)

func main() {
	cfg := config.Load()
	log := logging.New("goalpost")
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

	if err := goalpost.New(db, b, log).Run(ctx); err != nil {
		log.Error("goalpost", "err", err)
		os.Exit(1)
	}
}
