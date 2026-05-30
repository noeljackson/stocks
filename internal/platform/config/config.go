// Package config loads runtime configuration from the environment.
package config

import "os"

type Config struct {
	DatabaseURL  string
	NATSURL      string
	GatewayAddr  string
	LLMProvider  string
	ModelDeep    string
	ModelRoutine string
	ModelTriage  string
	SECUserAgent string
	FREDAPIKey   string
}

func get(k, def string) string {
	if v := os.Getenv(k); v != "" {
		return v
	}
	return def
}

// Load reads config from the environment, falling back to local-dev defaults.
func Load() Config {
	return Config{
		DatabaseURL:  get("DATABASE_URL", "postgres://stocks:stocks_dev_only@localhost:5432/stocks?sslmode=disable"),
		NATSURL:      get("NATS_URL", "nats://localhost:4222"),
		GatewayAddr:  get("GATEWAY_ADDR", ":8080"),
		LLMProvider:  get("LLM_PROVIDER", "mock"),
		ModelDeep:    get("LLM_MODEL_DEEP", "claude-opus-4-8"),
		ModelRoutine: get("LLM_MODEL_ROUTINE", "claude-sonnet-4-6"),
		ModelTriage:  get("LLM_MODEL_TRIAGE", "claude-haiku-4-5"),
		SECUserAgent: get("SEC_EDGAR_UA", "stocks-research n@noeljackson.com"),
		FREDAPIKey:   get("FRED_API_KEY", ""),
	}
}
