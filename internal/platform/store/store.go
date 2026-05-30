// Package store is the Postgres access layer (pgx pool + typed helpers).
package store

import (
	"context"
	"encoding/json"
	"errors"
	"strconv"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgxpool"

	"github.com/noeljackson/stocks/internal/domain"
)

type DB struct{ Pool *pgxpool.Pool }

func Open(ctx context.Context, url string) (*DB, error) {
	pool, err := pgxpool.New(ctx, url)
	if err != nil {
		return nil, err
	}
	if err := pool.Ping(ctx); err != nil {
		pool.Close()
		return nil, err
	}
	return &DB{Pool: pool}, nil
}

func (d *DB) Close() {
	if d.Pool != nil {
		d.Pool.Close()
	}
}

func nullStr(s string) *string {
	if s == "" {
		return nil
	}
	return &s
}

// AppendIngestEvent stores a raw event append-only (SPEC §4 PIT corpus).
// Returns false if content_hash already existed (dedup), true if newly inserted.
func (d *DB) AppendIngestEvent(ctx context.Context, source, kind, symbol string, payload []byte, contentHash string, sourceTS *time.Time) (bool, error) {
	tag, err := d.Pool.Exec(ctx,
		`INSERT INTO ingest_event (source, kind, symbol, payload, content_hash, source_ts)
		 VALUES ($1,$2,$3,$4::jsonb,$5,$6)
		 ON CONFLICT (content_hash) DO NOTHING`,
		source, kind, nullStr(symbol), string(payload), contentHash, sourceTS)
	if err != nil {
		return false, err
	}
	return tag.RowsAffected() > 0, nil
}

// ActiveConfig returns the active config body (raw JSON) and version for a name.
func (d *DB) ActiveConfig(ctx context.Context, name string) ([]byte, int, error) {
	var body []byte
	var version int
	err := d.Pool.QueryRow(ctx,
		`SELECT body, version FROM config WHERE name=$1 AND active LIMIT 1`, name).Scan(&body, &version)
	if err != nil {
		return nil, 0, err
	}
	return body, version, nil
}

// InsertAlert records an emitted significant shift and returns its id.
func (d *DB) InsertAlert(ctx context.Context, kind, symbol string, payload []byte) (int64, error) {
	var id int64
	err := d.Pool.QueryRow(ctx,
		`INSERT INTO alert (kind, symbol, payload) VALUES ($1,$2,$3::jsonb) RETURNING id`,
		kind, nullStr(symbol), string(payload)).Scan(&id)
	return id, err
}

// UpsertMarketState writes a regime classification row (SPEC §5.4).
// as_of is the PRIMARY KEY; conflicts overwrite (re-classifications at the
// same instant should be rare but harmless). config_version is the active
// regime-config version that produced this row (stamped as "vN" text per the
// schema's text typing).
func (d *DB) UpsertMarketState(ctx context.Context, asOf time.Time, regime string, capitulation bool, indicators []byte, configVersion int) error {
	_, err := d.Pool.Exec(ctx,
		`INSERT INTO market_state (as_of, regime, capitulation, indicators, config_version)
		 VALUES ($1,$2,$3,$4::jsonb,$5)
		 ON CONFLICT (as_of) DO UPDATE SET
		   regime = EXCLUDED.regime,
		   capitulation = EXCLUDED.capitulation,
		   indicators = EXCLUDED.indicators,
		   config_version = EXCLUDED.config_version`,
		asOf, regime, capitulation, string(indicators), strconv.Itoa(configVersion))
	return err
}

// LatestMarketState returns the most recent market_state row, or (false, nil) if none.
func (d *DB) LatestMarketState(ctx context.Context) (regime string, capitulation bool, ok bool, err error) {
	err = d.Pool.QueryRow(ctx,
		`SELECT regime, capitulation FROM market_state ORDER BY as_of DESC LIMIT 1`,
	).Scan(&regime, &capitulation)
	if err != nil {
		if errors.Is(err, pgx.ErrNoRows) {
			return "", false, false, nil
		}
		return "", false, false, err
	}
	return regime, capitulation, true, nil
}

// RecentAlerts returns the most recent alerts for the UI feed.
func (d *DB) RecentAlerts(ctx context.Context, limit int) ([]domain.Alert, error) {
	rows, err := d.Pool.Query(ctx,
		`SELECT id, thesis_id::text, symbol, kind, payload, acknowledged, created_at
		   FROM alert ORDER BY created_at DESC LIMIT $1`, limit)
	if err != nil {
		return nil, err
	}
	defer rows.Close()

	var out []domain.Alert
	for rows.Next() {
		var (
			a       domain.Alert
			thesis  *string
			symbol  *string
			payload []byte
		)
		if err := rows.Scan(&a.ID, &thesis, &symbol, &a.Kind, &payload, &a.Acknowledged, &a.CreatedAt); err != nil {
			return nil, err
		}
		a.ThesisID = thesis
		if symbol != nil {
			a.Symbol = *symbol
		}
		if len(payload) > 0 {
			_ = json.Unmarshal(payload, &a.Payload)
		}
		out = append(out, a)
	}
	return out, rows.Err()
}
