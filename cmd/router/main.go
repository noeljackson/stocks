// Command router fans ingest events to per-ticker subjects (SPEC §3).
package main

import (
	"context"
	"os"
	"os/signal"
	"syscall"

	"github.com/noeljackson/stocks/internal/platform/bus"
	"github.com/noeljackson/stocks/internal/platform/config"
	"github.com/noeljackson/stocks/internal/platform/logging"
	"github.com/noeljackson/stocks/internal/router"
)

func main() {
	cfg := config.Load()
	log := logging.New("router")
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()

	b, err := bus.Connect(cfg.NATSURL)
	if err != nil {
		log.Error("nats connect", "err", err)
		os.Exit(1)
	}
	defer b.Close()

	if err := router.New(b, log).Run(ctx); err != nil {
		log.Error("router", "err", err)
		os.Exit(1)
	}
}
