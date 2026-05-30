// Package gateway is the decision/alert + UI gateway (SPEC §3 + §11):
// embeds the SPA, serves REST for actions and SSE for the live feed, bridges
// NATS thesis/risk events into alerts.
package gateway

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"io/fs"
	"log/slog"
	"net/http"
	"strings"
	"sync"
	"time"

	"github.com/nats-io/nats.go/jetstream"

	"github.com/noeljackson/stocks/internal/platform/bus"
	"github.com/noeljackson/stocks/internal/platform/config"
	"github.com/noeljackson/stocks/internal/platform/store"
	"github.com/noeljackson/stocks/internal/platform/subjects"
	"github.com/noeljackson/stocks/internal/web"
)

type Server struct {
	cfg  config.Config
	db   *store.DB
	bus  *bus.Bus
	log  *slog.Logger
	hub  *hub
	stop []func() // durable-consumer drain handles
}

func New(cfg config.Config, db *store.DB, b *bus.Bus, log *slog.Logger) *Server {
	return &Server{cfg: cfg, db: db, bus: b, log: log, hub: newHub()}
}

// Start ensures the streams the gateway depends on exist, then binds durable
// JetStream consumers that persist alerts and feed the SSE hub.
func (s *Server) Start(ctx context.Context) error {
	if err := s.bus.EnsureStream(ctx, subjects.StreamThesis, "thesis.*"); err != nil {
		return fmt.Errorf("ensure THESIS: %w", err)
	}
	if err := s.bus.EnsureStream(ctx, subjects.StreamDecisions, "risk.*", "decision.*"); err != nil {
		return fmt.Errorf("ensure DECISIONS: %w", err)
	}
	thesisStop, err := s.bus.Consume(ctx, subjects.StreamThesis, "gateway-thesis-alerts", "thesis.*", s.onEvent("state_transition"))
	if err != nil {
		return err
	}
	riskStop, err := s.bus.Consume(ctx, subjects.StreamDecisions, "gateway-risk-alerts", "risk.*", s.onEvent("risk"))
	if err != nil {
		thesisStop()
		return err
	}
	s.stop = append(s.stop, thesisStop, riskStop)
	return nil
}

// Stop drains all durable consumers.
func (s *Server) Stop() {
	for _, f := range s.stop {
		f()
	}
}

func (s *Server) onEvent(kind string) func(jetstream.Msg) error {
	return func(m jetstream.Msg) error {
		ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
		defer cancel()
		data := m.Data()
		subj := m.Subject()
		if _, err := s.db.InsertAlert(ctx, kind, "", data); err != nil {
			s.log.Error("insert alert", "err", err)
			return err // Nak → JetStream redelivers
		}
		env, _ := json.Marshal(map[string]any{
			"subject": subj, "kind": kind, "payload": json.RawMessage(data),
		})
		s.hub.broadcast(env)
		return nil
	}
}

func (s *Server) Routes() http.Handler {
	mux := http.NewServeMux()
	mux.HandleFunc("GET /healthz", func(w http.ResponseWriter, _ *http.Request) { _, _ = w.Write([]byte("ok")) })
	mux.HandleFunc("GET /api/alerts", s.handleAlerts)
	mux.HandleFunc("GET /api/stream", s.handleStream)
	mux.HandleFunc("POST /api/decisions", s.handleDecision)
	mux.Handle("/", s.spa())
	return mux
}

func (s *Server) handleAlerts(w http.ResponseWriter, r *http.Request) {
	alerts, err := s.db.RecentAlerts(r.Context(), 100)
	if err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	writeJSON(w, alerts)
}

func (s *Server) handleStream(w http.ResponseWriter, r *http.Request) {
	fl, ok := w.(http.Flusher)
	if !ok {
		http.Error(w, "stream unsupported", http.StatusInternalServerError)
		return
	}
	w.Header().Set("Content-Type", "text/event-stream")
	w.Header().Set("Cache-Control", "no-cache")
	w.Header().Set("Connection", "keep-alive")

	ch := s.hub.add()
	defer s.hub.remove(ch)
	fmt.Fprint(w, ": connected\n\n")
	fl.Flush()

	ka := time.NewTicker(25 * time.Second)
	defer ka.Stop()
	for {
		select {
		case <-r.Context().Done():
			return
		case <-ka.C:
			fmt.Fprint(w, ": keepalive\n\n")
			fl.Flush()
		case msg := <-ch:
			fmt.Fprintf(w, "data: %s\n\n", msg)
			fl.Flush()
		}
	}
}

type decisionReq struct {
	ThesisID   string          `json:"thesis_id"`
	Action     string          `json:"action"`
	UserChoice string          `json:"user_choice"`
	Sizing     json.RawMessage `json:"sizing"`
}

func (s *Server) handleDecision(w http.ResponseWriter, r *http.Request) {
	var req decisionReq
	if err := json.NewDecoder(io.LimitReader(r.Body, 1<<20)).Decode(&req); err != nil {
		http.Error(w, "bad json", http.StatusBadRequest)
		return
	}
	sizing := "null"
	if len(req.Sizing) > 0 {
		sizing = string(req.Sizing)
	}
	_, err := s.db.Pool.Exec(r.Context(),
		`INSERT INTO decision (thesis_id, action, user_choice, sizing)
		 VALUES (NULLIF($1,'')::uuid, $2, $3, $4::jsonb)`,
		req.ThesisID, req.Action, req.UserChoice, sizing)
	if err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	env, _ := json.Marshal(req)
	_ = s.bus.Publish(r.Context(), subjects.DecisionRecorded, env)
	w.WriteHeader(http.StatusNoContent)
}

func (s *Server) spa() http.Handler {
	sub, err := fs.Sub(web.Dist, "dist")
	if err != nil {
		panic(err)
	}
	fileServer := http.FileServer(http.FS(sub))
	index, _ := fs.ReadFile(sub, "index.html")
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		p := strings.TrimPrefix(r.URL.Path, "/")
		if p == "" {
			p = "index.html"
		}
		if _, statErr := fs.Stat(sub, p); statErr != nil {
			w.Header().Set("Content-Type", "text/html; charset=utf-8")
			_, _ = w.Write(index) // SPA client-routing fallback
			return
		}
		fileServer.ServeHTTP(w, r)
	})
}

func writeJSON(w http.ResponseWriter, v any) {
	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(v)
}

// ---- SSE fan-out hub ----

type hub struct {
	mu      sync.Mutex
	clients map[chan []byte]struct{}
}

func newHub() *hub { return &hub{clients: make(map[chan []byte]struct{})} }

func (h *hub) add() chan []byte {
	ch := make(chan []byte, 16)
	h.mu.Lock()
	h.clients[ch] = struct{}{}
	h.mu.Unlock()
	return ch
}

func (h *hub) remove(ch chan []byte) {
	h.mu.Lock()
	if _, ok := h.clients[ch]; ok {
		delete(h.clients, ch)
		close(ch)
	}
	h.mu.Unlock()
}

func (h *hub) broadcast(b []byte) {
	h.mu.Lock()
	defer h.mu.Unlock()
	for ch := range h.clients {
		select {
		case ch <- b:
		default: // drop for slow clients
		}
	}
}
