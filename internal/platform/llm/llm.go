// Package llm is the swappable LLM provider abstraction (SPEC §3 invariant,
// §6 Integration). v1 uses a subscription transport (user's decision); the
// interface keeps that reversible to the API without touching call sites.
package llm

import "context"

type Message struct {
	Role    string `json:"role"` // "user" | "assistant"
	Content string `json:"content"`
}

type Request struct {
	Model    string    `json:"model"`
	System   string    `json:"system,omitempty"`
	Messages []Message `json:"messages"`
	// JSONSchema, when set, instructs the provider to return schema-valid JSON
	// (structured output for theses/context).
	JSONSchema []byte `json:"-"`
}

type Response struct {
	Content string `json:"content"`
}

type Provider interface {
	Complete(ctx context.Context, req Request) (Response, error)
}

// New returns a provider by name. Real transports (subscription, anthropic)
// are implemented behind this same interface; "mock" is the dev default.
func New(name string) Provider {
	switch name {
	case "anthropic":
		// TODO: Anthropic API client (prompt caching + batch + model tiering).
		return &mock{}
	case "subscription":
		// TODO: subscription transport (user's elected v1 access path).
		return &mock{}
	default:
		return &mock{}
	}
}

type mock struct{}

func (m *mock) Complete(_ context.Context, _ Request) (Response, error) {
	return Response{Content: `{"mock":true}`}, nil
}
