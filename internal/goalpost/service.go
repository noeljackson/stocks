package goalpost

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"log/slog"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/nats-io/nats.go/jetstream"

	"github.com/noeljackson/stocks/internal/platform/bus"
	"github.com/noeljackson/stocks/internal/platform/store"
	"github.com/noeljackson/stocks/internal/platform/subjects"
)

// updatedEvent is the minimal JSON shape the (forthcoming) thesis engine
// will publish on thesis.updated. We need the thesis_id and the new version
// number; everything else (immutable_original, current invalidation_conditions)
// is loaded from the DB so the diff is always against authoritative state.
type updatedEvent struct {
	ThesisID string `json:"thesis_id"`
	Version  int    `json:"version"`
	Rationale string `json:"rationale,omitempty"`
}

type Service struct {
	DB  *store.DB
	Bus *bus.Bus
	Log *slog.Logger
}

func New(db *store.DB, b *bus.Bus, log *slog.Logger) *Service {
	return &Service{DB: db, Bus: b, Log: log}
}

func (s *Service) Run(ctx context.Context) error {
	if err := s.Bus.EnsureStream(ctx, subjects.StreamThesis, "thesis.*"); err != nil {
		return fmt.Errorf("ensure THESIS: %w", err)
	}
	stop, err := s.Bus.Consume(ctx, subjects.StreamThesis, "goalpost-detector", subjects.ThesisUpdated,
		func(m jetstream.Msg) error { return s.onUpdated(ctx, m) })
	if err != nil {
		return err
	}
	defer stop()
	s.Log.Info("goalpost detector consuming", "stream", subjects.StreamThesis, "filter", subjects.ThesisUpdated)
	<-ctx.Done()
	return nil
}

func (s *Service) onUpdated(ctx context.Context, m jetstream.Msg) error {
	var ev updatedEvent
	if err := json.Unmarshal(m.Data(), &ev); err != nil {
		s.Log.Warn("dropping malformed thesis.updated", "err", err)
		return nil
	}
	if ev.ThesisID == "" {
		s.Log.Warn("thesis.updated missing thesis_id; dropping")
		return nil
	}

	orig, curr, err := s.loadConditions(ctx, ev.ThesisID)
	if err != nil {
		s.Log.Error("load conditions", "thesis_id", ev.ThesisID, "err", err)
		return err
	}
	r := Analyze(orig, curr)

	// Persist the verdict to thesis_version_history (append-only).
	diffBody, _ := json.Marshal(map[string]any{
		"dropped": r.Dropped, "loosened": r.Loosened,
		"added": r.Added, "needs_review": r.NeedsReview,
		"reasons": r.Reasons,
	})
	if err := s.recordVersion(ctx, ev.ThesisID, ev.Version, diffBody, ev.Rationale, r.Weakened); err != nil {
		s.Log.Error("record version", "err", err)
		return err
	}

	if r.Weakened || r.NeedsReview {
		payload, _ := json.Marshal(map[string]any{
			"thesis_id":    ev.ThesisID,
			"version":      ev.Version,
			"kind":         "goalpost_moved",
			"weakened":     r.Weakened,
			"needs_review": r.NeedsReview,
			"dropped":      r.Dropped,
			"loosened":     r.Loosened,
			"added":        r.Added,
			"reasons":      r.Reasons,
			"at":           time.Now().UTC(),
		})
		// Publish to risk.warning — it flows through the DECISIONS stream and
		// surfaces on the gateway's SSE feed alongside other risk verdicts.
		// thesis.invalidated is reserved for the real lifecycle transition.
		if err := s.Bus.Publish(ctx, subjects.RiskWarning, payload); err != nil {
			return err
		}
		s.Log.Warn("goalpost moved", "thesis_id", ev.ThesisID, "version", ev.Version,
			"weakened", r.Weakened, "needs_review", r.NeedsReview,
			"dropped", r.Dropped, "loosened", r.Loosened)
	} else {
		s.Log.Info("goalpost clean", "thesis_id", ev.ThesisID, "version", ev.Version)
	}
	return nil
}

func (s *Service) loadConditions(ctx context.Context, thesisID string) (orig, curr Conditions, err error) {
	var origJSON, currJSON []byte
	err = s.DB.Pool.QueryRow(ctx, `
		SELECT COALESCE(immutable_original -> 'invalidation_conditions', '[]'::jsonb),
		       invalidation_conditions
		  FROM thesis WHERE thesis_id = $1`, thesisID).Scan(&origJSON, &currJSON)
	if err != nil {
		if errors.Is(err, pgx.ErrNoRows) {
			return nil, nil, fmt.Errorf("thesis %s not found", thesisID)
		}
		return nil, nil, err
	}
	if err := json.Unmarshal(origJSON, &orig); err != nil {
		return nil, nil, fmt.Errorf("decode original: %w", err)
	}
	if err := json.Unmarshal(currJSON, &curr); err != nil {
		return nil, nil, fmt.Errorf("decode current: %w", err)
	}
	return orig, curr, nil
}

func (s *Service) recordVersion(ctx context.Context, thesisID string, version int, diff []byte, rationale string, weakens bool) error {
	_, err := s.DB.Pool.Exec(ctx, `
		INSERT INTO thesis_version_history (thesis_id, version, diff, rationale, weakens_invalidation)
		VALUES ($1, $2, $3::jsonb, NULLIF($4,''), $5)`,
		thesisID, version, string(diff), rationale, weakens)
	return err
}
