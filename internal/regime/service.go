package regime

import (
	"context"
	"encoding/json"
	"fmt"
	"log/slog"
	"strconv"
	"sync"
	"time"

	"github.com/nats-io/nats.go/jetstream"

	"github.com/noeljackson/stocks/internal/domain"
	"github.com/noeljackson/stocks/internal/platform/bus"
	"github.com/noeljackson/stocks/internal/platform/store"
	"github.com/noeljackson/stocks/internal/platform/subjects"
)

// macroObservation is the schema produced by the FRED adapter.
type macroObservation struct {
	Series string `json:"series"`
	Date   string `json:"date"`
	Value  string `json:"value"`
}

// Service maintains the latest indicator snapshot and recomputes regime on
// every macro update. It is deterministic and stateless across restarts —
// the in-memory snapshot is rebuilt from the most recent ingest_event row per
// series at boot (if no events yet, it starts empty; classify → neutral).
type Service struct {
	DB  *store.DB
	Bus *bus.Bus
	Log *slog.Logger

	mu   sync.Mutex
	snap map[string]float64
	last domain.Regime
	lastCap bool
}

func New(db *store.DB, b *bus.Bus, log *slog.Logger) *Service {
	return &Service{DB: db, Bus: b, Log: log, snap: map[string]float64{}, last: domain.RegimeNeutral}
}

// Run wires the regime classifier:
//   - ensures the MARKET stream (publish target for regime.*) exists,
//   - warm-starts the in-memory snapshot from ingest_event,
//   - binds a durable JetStream consumer on INGEST/ingest.macro,
//   - blocks until ctx is cancelled.
func (s *Service) Run(ctx context.Context) error {
	if err := s.Bus.EnsureStream(ctx, subjects.StreamMarket, "regime.*", "discovery.*"); err != nil {
		return fmt.Errorf("ensure MARKET: %w", err)
	}
	if err := s.warmStart(ctx); err != nil {
		s.Log.Warn("warm-start failed; starting empty", "err", err)
	}
	stop, err := s.Bus.Consume(ctx, subjects.StreamIngest, "regime-classifier", subjects.IngestMacro,
		func(m jetstream.Msg) error { return s.onMacro(ctx, m.Data()) })
	if err != nil {
		return err
	}
	defer stop()
	s.Log.Info("regime classifier consuming", "stream", subjects.StreamIngest, "filter", subjects.IngestMacro)
	<-ctx.Done()
	return nil
}

// observation is the parsed-and-mapped form of a macro message.
type observation struct {
	Series, Name string
	Value        float64
	Date         string
}

// parseObservation decodes the FRED adapter payload and applies the
// series-name → indicator-name mapping. Returns ok=false for malformed
// (poison) messages; the caller should ack-and-drop those.
func parseObservation(data []byte) (observation, bool) {
	var raw macroObservation
	if err := json.Unmarshal(data, &raw); err != nil {
		return observation{}, false
	}
	if raw.Series == "" {
		return observation{}, false
	}
	v, err := strconv.ParseFloat(raw.Value, 64)
	if err != nil {
		return observation{}, false
	}
	name, ok := FREDSeriesToIndicator[raw.Series]
	if !ok {
		name = raw.Series
	}
	return observation{Series: raw.Series, Name: name, Value: v, Date: raw.Date}, true
}

// applyObservation updates the in-memory snapshot with obs and returns the
// classification + a snapshot copy + whether the regime/capitulation flipped.
// Pure (besides the locked snapshot mutation) — no DB, no NATS, no clock.
func (s *Service) applyObservation(cfg Config, obs observation) (Result, map[string]float64, bool) {
	s.mu.Lock()
	s.snap[obs.Name] = obs.Value
	snap := make(map[string]float64, len(s.snap))
	for k, v := range s.snap {
		snap[k] = v
	}
	prevReg, prevCap := s.last, s.lastCap
	s.mu.Unlock()

	res := Classify(cfg, snap)

	s.mu.Lock()
	s.last = res.Regime
	s.lastCap = res.Capitulation
	s.mu.Unlock()

	return res, snap, res.Regime != prevReg || res.Capitulation != prevCap
}

func (s *Service) onMacro(ctx context.Context, data []byte) error {
	obs, ok := parseObservation(data)
	if !ok {
		s.Log.Warn("dropping malformed macro message")
		return nil // poison: ack
	}
	cfg, ver, err := s.loadConfig(ctx)
	if err != nil {
		s.Log.Error("load regime config", "err", err)
		return err // transient: let JetStream redeliver
	}
	res, snap, changed := s.applyObservation(cfg, obs)

	asOf := time.Now().UTC()
	ind, _ := json.Marshal(snap)
	if err := s.DB.UpsertMarketState(ctx, asOf, string(res.Regime), res.Capitulation, ind, ver); err != nil {
		s.Log.Error("persist market_state", "err", err)
		return err
	}
	out, _ := json.Marshal(map[string]any{
		"as_of":          asOf,
		"regime":         res.Regime,
		"capitulation":   res.Capitulation,
		"matched":        res.Matched,
		"config_version": ver,
		"trigger":        map[string]any{"series": obs.Series, "name": obs.Name, "value": obs.Value, "date": obs.Date},
	})
	if err := s.Bus.Publish(ctx, subjects.RegimeState, out); err != nil {
		s.Log.Error("publish regime.state", "err", err)
		return err
	}
	if changed {
		s.Log.Info("regime change", "regime", res.Regime, "capitulation", res.Capitulation)
		if res.Capitulation {
			_ = s.Bus.Publish(ctx, subjects.RegimeCapitulation, out)
		}
	}
	return nil
}

// warmStart rebuilds the snapshot from the latest FRED observation per series
// already in ingest_event (so a restart doesn't lose state until the next poll).
func (s *Service) warmStart(ctx context.Context) error {
	rows, err := s.DB.Pool.Query(ctx, `
		SELECT DISTINCT ON (payload->>'series') payload
		  FROM ingest_event
		 WHERE source='fred'
		 ORDER BY payload->>'series', ingested_at DESC`)
	if err != nil {
		return err
	}
	defer rows.Close()
	for rows.Next() {
		var raw []byte
		if err := rows.Scan(&raw); err != nil {
			return err
		}
		var obs macroObservation
		if err := json.Unmarshal(raw, &obs); err != nil {
			continue
		}
		v, err := strconv.ParseFloat(obs.Value, 64)
		if err != nil {
			continue
		}
		name, ok := FREDSeriesToIndicator[obs.Series]
		if !ok {
			name = obs.Series
		}
		s.snap[name] = v
	}
	return rows.Err()
}

func (s *Service) loadConfig(ctx context.Context) (Config, int, error) {
	body, ver, err := s.DB.ActiveConfig(ctx, "regime")
	if err != nil {
		return Config{}, 0, err
	}
	var cfg Config
	if err := json.Unmarshal(body, &cfg); err != nil {
		return Config{}, 0, err
	}
	return cfg, ver, nil
}
