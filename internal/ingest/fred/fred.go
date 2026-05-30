// Package fred ingests macro series via FRED's official REST API.
//
// The keyless fredgraph CSV endpoint is now behind Akamai bot protection
// (returns 504 to non-browser clients), so we require an API key. Get one
// free at https://fred.stlouisfed.org/docs/api/api_key.html and set
// FRED_API_KEY. Without it, the adapter no-ops (logs once on first poll).
package fred

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"log/slog"
	"net/http"
	"time"

	"github.com/noeljackson/stocks/internal/ingest"
	"github.com/noeljackson/stocks/internal/platform/subjects"
)

type Adapter struct {
	apiKey   string
	series   []string
	client   *http.Client
	warnedNK bool
}

// Default series: 10y, 3mo (curve), HY OAS, VIX — regime inputs (SPEC §4).
func New(apiKey string) *Adapter {
	return &Adapter{
		apiKey: apiKey,
		series: []string{"DGS10", "DGS3MO", "BAMLH0A0HYM2", "VIXCLS"},
		client: &http.Client{Timeout: 20 * time.Second},
	}
}

func (a *Adapter) Name() string            { return "fred" }
func (a *Adapter) Interval() time.Duration { return 6 * time.Hour }

func (a *Adapter) Poll(ctx context.Context) ([]ingest.Event, error) {
	if a.apiKey == "" {
		if !a.warnedNK {
			slog.Warn("fred: FRED_API_KEY not set; skipping macro ingest")
			a.warnedNK = true
		}
		return nil, nil
	}
	var out []ingest.Event
	for _, id := range a.series {
		ev, ok, err := a.pollOne(ctx, id)
		if err != nil {
			return out, fmt.Errorf("fred %s: %w", id, err)
		}
		if ok {
			out = append(out, ev)
		}
	}
	return out, nil
}

type fredResp struct {
	Observations []struct {
		Date  string `json:"date"`
		Value string `json:"value"`
	} `json:"observations"`
}

// pollOne fetches the latest non-missing observation for a series via the
// official JSON API (series/observations sorted desc, limit 1).
func (a *Adapter) pollOne(ctx context.Context, id string) (ingest.Event, bool, error) {
	url := fmt.Sprintf(
		"https://api.stlouisfed.org/fred/series/observations"+
			"?series_id=%s&api_key=%s&file_type=json&sort_order=desc&limit=1",
		id, a.apiKey,
	)
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, url, nil)
	if err != nil {
		return ingest.Event{}, false, err
	}
	resp, err := a.client.Do(req)
	if err != nil {
		return ingest.Event{}, false, err
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(io.LimitReader(resp.Body, 512))
		return ingest.Event{}, false, fmt.Errorf("status %d: %s", resp.StatusCode, body)
	}
	var r fredResp
	if err := json.NewDecoder(resp.Body).Decode(&r); err != nil {
		return ingest.Event{}, false, err
	}
	if len(r.Observations) == 0 || r.Observations[0].Value == "" || r.Observations[0].Value == "." {
		return ingest.Event{}, false, nil
	}
	obs := r.Observations[0]
	ts, _ := time.Parse("2006-01-02", obs.Date)
	payload, _ := json.Marshal(map[string]any{"series": id, "date": obs.Date, "value": obs.Value})
	return ingest.Event{
		Source: "fred", Kind: "series", Symbol: "",
		Subject: subjects.IngestMacro, Payload: payload, SourceTS: &ts,
	}, true, nil
}
