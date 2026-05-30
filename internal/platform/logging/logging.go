// Package logging provides a structured slog logger.
package logging

import (
	"log/slog"
	"os"
)

// New returns a JSON structured logger bound to the given service name.
func New(service string) *slog.Logger {
	h := slog.NewJSONHandler(os.Stdout, &slog.HandlerOptions{Level: slog.LevelInfo})
	return slog.New(h).With("service", service)
}
