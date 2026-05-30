package router

import (
	"context"
	"fmt"
	"log/slog"

	"github.com/nats-io/nats.go/jetstream"

	"github.com/noeljackson/stocks/internal/platform/bus"
	"github.com/noeljackson/stocks/internal/platform/subjects"
)

// Service consumes INGEST/ingest.* events durably and republishes those
// carrying a ticker to route.ticker.<SYMBOL> on the TICKER stream. Market-wide
// events (no symbol) are acked and dropped — downstream services that want
// them subscribe to INGEST directly.
type Service struct {
	Bus *bus.Bus
	Log *slog.Logger
}

func New(b *bus.Bus, log *slog.Logger) *Service { return &Service{Bus: b, Log: log} }

func (s *Service) Run(ctx context.Context) error {
	// TICKER stream uses a wildcard subject because there's one published
	// subject per ticker (route.ticker.NVDA, route.ticker.MU, ...).
	if err := s.Bus.EnsureStream(ctx, subjects.StreamTicker, "route.ticker.*"); err != nil {
		return fmt.Errorf("ensure TICKER: %w", err)
	}
	stop, err := s.Bus.Consume(ctx, subjects.StreamIngest, "event-router", "ingest.*",
		func(m jetstream.Msg) error { return s.onIngest(ctx, m) })
	if err != nil {
		return err
	}
	defer stop()
	s.Log.Info("router consuming", "stream", subjects.StreamIngest, "filter", "ingest.*")
	<-ctx.Done()
	return nil
}

func (s *Service) onIngest(ctx context.Context, m jetstream.Msg) error {
	sym, ok := extractSymbol(m.Data())
	if !ok {
		return nil // market-wide; ack-and-drop
	}
	subj := routeSubject(sym)
	if err := s.Bus.Publish(ctx, subj, m.Data()); err != nil {
		// Transient: let JetStream redeliver. Don't log every retry — only the
		// final failure (MaxDeliver bounds it).
		return err
	}
	return nil
}
