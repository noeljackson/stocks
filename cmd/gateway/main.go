// Command gateway runs the decision/alert + UI gateway HTTP server.
package main

import (
	"context"
	"errors"
	"net/http"
	"os"
	"os/signal"
	"syscall"
	"time"

	"github.com/noeljackson/stocks/internal/gateway"
	"github.com/noeljackson/stocks/internal/platform/bus"
	"github.com/noeljackson/stocks/internal/platform/config"
	"github.com/noeljackson/stocks/internal/platform/logging"
	"github.com/noeljackson/stocks/internal/platform/store"
)

func main() {
	cfg := config.Load()
	log := logging.New("gateway")
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

	srv := gateway.New(cfg, db, b, log)
	if err := srv.Start(ctx); err != nil {
		log.Error("start subscriptions", "err", err)
		os.Exit(1)
	}
	defer srv.Stop()

	httpSrv := &http.Server{
		Addr:              cfg.GatewayAddr,
		Handler:           srv.Routes(),
		ReadHeaderTimeout: 10 * time.Second,
	}
	go func() {
		<-ctx.Done()
		sctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
		defer cancel()
		_ = httpSrv.Shutdown(sctx)
	}()

	log.Info("gateway listening", "addr", cfg.GatewayAddr)
	if err := httpSrv.ListenAndServe(); err != nil && !errors.Is(err, http.ErrServerClosed) {
		log.Error("listen", "err", err)
		os.Exit(1)
	}
}
