// Package edgar ingests SEC filings via the EDGAR submissions JSON API (free).
package edgar

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"strings"
	"time"

	"github.com/noeljackson/stocks/internal/ingest"
	"github.com/noeljackson/stocks/internal/platform/subjects"
)

const maxFilings = 10

type Adapter struct {
	ua     string
	ciks   map[string]string // ticker -> 10-digit CIK; TODO: source from DB Tier-1
	client *http.Client
}

func New(ua string) *Adapter {
	return &Adapter{
		ua: ua,
		ciks: map[string]string{
			"NVDA": "0001045810",
			"MU":   "0000723125",
		},
		client: &http.Client{Timeout: 20 * time.Second},
	}
}

func (a *Adapter) Name() string            { return "edgar" }
func (a *Adapter) Interval() time.Duration { return 30 * time.Minute }

type submissions struct {
	Filings struct {
		Recent struct {
			AccessionNumber []string `json:"accessionNumber"`
			FilingDate      []string `json:"filingDate"`
			Form            []string `json:"form"`
			PrimaryDocument []string `json:"primaryDocument"`
		} `json:"recent"`
	} `json:"filings"`
}

func (a *Adapter) Poll(ctx context.Context) ([]ingest.Event, error) {
	var out []ingest.Event
	for ticker, cik := range a.ciks {
		evs, err := a.pollOne(ctx, ticker, cik)
		if err != nil {
			return out, fmt.Errorf("edgar %s: %w", ticker, err)
		}
		out = append(out, evs...)
	}
	return out, nil
}

func (a *Adapter) pollOne(ctx context.Context, ticker, cik string) ([]ingest.Event, error) {
	url := fmt.Sprintf("https://data.sec.gov/submissions/CIK%s.json", cik)
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, url, nil)
	if err != nil {
		return nil, err
	}
	req.Header.Set("User-Agent", a.ua) // SEC requires a descriptive UA
	req.Header.Set("Accept", "application/json")
	resp, err := a.client.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("status %d", resp.StatusCode)
	}
	body, err := io.ReadAll(io.LimitReader(resp.Body, 16<<20))
	if err != nil {
		return nil, err
	}
	var s submissions
	if err := json.Unmarshal(body, &s); err != nil {
		return nil, err
	}

	r := s.Filings.Recent
	n := len(r.AccessionNumber)
	if n > maxFilings {
		n = maxFilings
	}
	out := make([]ingest.Event, 0, n)
	for i := 0; i < n; i++ {
		filed, _ := time.Parse("2006-01-02", r.FilingDate[i])
		ts := filed
		doc := ""
		if i < len(r.PrimaryDocument) {
			doc = r.PrimaryDocument[i]
		}
		accNoDashes := strings.ReplaceAll(r.AccessionNumber[i], "-", "")
		docURL := fmt.Sprintf("https://www.sec.gov/Archives/edgar/data/%s/%s/%s",
			strings.TrimLeft(cik, "0"), accNoDashes, doc)
		payload, _ := json.Marshal(map[string]any{
			"ticker": ticker, "cik": cik, "form": r.Form[i],
			"accession": r.AccessionNumber[i], "filing_date": r.FilingDate[i],
			"primary_document": doc, "url": docURL,
		})
		out = append(out, ingest.Event{
			Source: "edgar", Kind: r.Form[i], Symbol: ticker,
			Subject: subjects.IngestFiling, Payload: payload, SourceTS: &ts,
		})
	}
	return out, nil
}
