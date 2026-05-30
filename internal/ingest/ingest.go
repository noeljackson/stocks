// Package ingest is the adapter framework (SPEC §3 ingestion):
// fetch -> normalize -> append-only store + emit to NATS.
package ingest

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"log/slog"
	"time"

	"github.com/noeljackson/stocks/internal/platform/bus"
	"github.com/noeljackson/stocks/internal/platform/store"
)

// Event is a normalized item produced by an adapter.
type Event struct {
	Source   string     // 'edgar','fred',...
	Kind     string     // '10-K','series',...
	Symbol   string     // "" for market-wide
	Subject  string     // NATS subject to publish on
	Payload  []byte     // canonical JSON
	SourceTS *time.Time // event's own timestamp, if known
}

// ContentHash is the dedup key (stable over source+kind+symbol+payload).
func (e Event) ContentHash() string {
	h := sha256.New()
	for _, p := range []string{e.Source, e.Kind, e.Symbol} {
		h.Write([]byte(p))
		h.Write([]byte{0})
	}
	h.Write(e.Payload)
	return hex.EncodeToString(h.Sum(nil))
}

// Adapter polls an external source and returns current events.
// Returning already-seen events is fine: dedup happens at the store.
type Adapter interface {
	Name() string
	Interval() time.Duration
	Poll(ctx context.Context) ([]Event, error)
}

// Runner schedules adapters, stores append-only, and publishes new events.
type Runner struct {
	DB  *store.DB
	Bus *bus.Bus
	Log *slog.Logger
}

func (r *Runner) Run(ctx context.Context, adapters ...Adapter) {
	for _, a := range adapters {
		go r.loop(ctx, a)
	}
	<-ctx.Done()
}

func (r *Runner) loop(ctx context.Context, a Adapter) {
	log := r.Log.With("adapter", a.Name())
	t := time.NewTicker(a.Interval())
	defer t.Stop()
	for {
		r.once(ctx, a, log)
		select {
		case <-ctx.Done():
			return
		case <-t.C:
		}
	}
}

func (r *Runner) once(ctx context.Context, a Adapter, log *slog.Logger) {
	events, err := a.Poll(ctx)
	if err != nil {
		log.Error("poll failed", "err", err)
		return
	}
	var stored, published int
	for _, e := range events {
		inserted, err := r.DB.AppendIngestEvent(ctx, e.Source, e.Kind, e.Symbol, e.Payload, e.ContentHash(), e.SourceTS)
		if err != nil {
			log.Error("store failed", "err", err)
			continue
		}
		if !inserted {
			continue // already seen
		}
		stored++
		if e.Subject != "" {
			if err := r.Bus.Publish(ctx, e.Subject, e.Payload); err != nil {
				log.Error("publish failed", "subject", e.Subject, "err", err)
				continue
			}
			published++
		}
	}
	if stored > 0 {
		log.Info("ingested", "new", stored, "published", published)
	}
}
