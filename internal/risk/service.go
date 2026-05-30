package risk

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

// actionable mirrors the JSON shape published by the (forthcoming) thesis
// engine on thesis.actionable. v0 uses the minimal subset the risk overlay
// needs — the engine is free to add fields without breaking us.
type actionable struct {
	ThesisID      string  `json:"thesis_id"`
	Symbol        string  `json:"symbol"`
	Cluster       string  `json:"cluster"`
	Instrument    string  `json:"instrument"`
	DeltaNotional float64 `json:"delta_notional"`
	PremiumAtRisk float64 `json:"premium_at_risk"`
}

// portfolioDefaults provides the cash%/drawdown% that v0 can't yet compute
// from a real broker integration. Override via env if you're paper-trading
// against a known balance (TODO: wire into Service.PortfolioOverride).
var portfolioDefaults = Portfolio{TotalValue: 100_000, CashPct: 50, DrawdownPct: 0}

type Service struct {
	DB                *store.DB
	Bus               *bus.Bus
	Log               *slog.Logger
	PortfolioOverride *Portfolio // optional; nil → use computed/default
}

func New(db *store.DB, b *bus.Bus, log *slog.Logger) *Service {
	return &Service{DB: db, Bus: b, Log: log}
}

func (s *Service) Run(ctx context.Context) error {
	if err := s.Bus.EnsureStream(ctx, subjects.StreamDecisions, "risk.*", "decision.*"); err != nil {
		return fmt.Errorf("ensure DECISIONS: %w", err)
	}
	stop, err := s.Bus.Consume(ctx, subjects.StreamThesis, "risk-overlay", "thesis.actionable",
		func(m jetstream.Msg) error { return s.onActionable(ctx, m) })
	if err != nil {
		return err
	}
	defer stop()
	s.Log.Info("risk overlay consuming", "stream", subjects.StreamThesis, "filter", "thesis.actionable")
	<-ctx.Done()
	return nil
}

func (s *Service) onActionable(ctx context.Context, m jetstream.Msg) error {
	var a actionable
	if err := json.Unmarshal(m.Data(), &a); err != nil {
		s.Log.Warn("dropping malformed thesis.actionable", "err", err)
		return nil // ack-and-drop
	}
	cfg, ver, err := s.loadConfig(ctx)
	if err != nil {
		s.Log.Error("load risk config", "err", err)
		return err
	}
	positions, err := s.loadOpenPositions(ctx)
	if err != nil {
		s.Log.Error("load positions", "err", err)
		return err
	}
	port := portfolioDefaults
	if s.PortfolioOverride != nil {
		port = *s.PortfolioOverride
	}

	decision := Evaluate(toIntent(a), positions, port, cfg)
	if !decision.Veto && len(decision.Warnings) == 0 {
		return nil // clean: nothing to publish
	}

	payload, _ := json.Marshal(map[string]any{
		"thesis_id":      a.ThesisID,
		"symbol":         a.Symbol,
		"veto":           decision.Veto,
		"reasons":        decision.Reasons,
		"warnings":       decision.Warnings,
		"size_mult":      decision.SizeMult,
		"config_version": ver,
		"at":             time.Now().UTC(),
	})
	subj := subjects.RiskWarning
	if decision.Veto {
		subj = subjects.RiskVeto
	}
	if err := s.Bus.Publish(ctx, subj, payload); err != nil {
		s.Log.Error("publish risk verdict", "subject", subj, "err", err)
		return err
	}
	s.Log.Info("risk verdict", "subject", subj, "symbol", a.Symbol,
		"veto", decision.Veto, "reasons", decision.Reasons, "warnings", decision.Warnings)
	return nil
}

func (s *Service) loadConfig(ctx context.Context) (Config, int, error) {
	body, ver, err := s.DB.ActiveConfig(ctx, "risk")
	if err != nil {
		return Config{}, 0, err
	}
	var c Config
	if err := json.Unmarshal(body, &c); err != nil {
		return Config{}, 0, err
	}
	return c, ver, nil
}

func (s *Service) loadOpenPositions(ctx context.Context) ([]Position, error) {
	rows, err := s.DB.Pool.Query(ctx, `
		SELECT p.symbol, COALESCE(t.cluster_id, ''), p.instrument,
		       COALESCE(p.delta_notional, 0), COALESCE(p.premium_at_risk, 0)
		  FROM position p
		  LEFT JOIN ticker t ON t.symbol = p.symbol
		 WHERE p.closed_at IS NULL`)
	if err != nil {
		if errors.Is(err, pgx.ErrNoRows) {
			return nil, nil
		}
		return nil, err
	}
	defer rows.Close()
	var out []Position
	for rows.Next() {
		var p Position
		if err := rows.Scan(&p.Symbol, &p.Cluster, &p.Instrument, &p.DeltaNotional, &p.PremiumAtRisk); err != nil {
			return nil, err
		}
		out = append(out, p)
	}
	return out, rows.Err()
}

func toIntent(a actionable) Intent {
	return Intent{
		Symbol:        a.Symbol,
		Cluster:       a.Cluster,
		Instrument:    a.Instrument,
		DeltaNotional: a.DeltaNotional,
		PremiumAtRisk: a.PremiumAtRisk,
	}
}
