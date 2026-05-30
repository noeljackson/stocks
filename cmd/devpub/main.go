// Command devpub publishes a synthetic event for local smoke testing.
//
// Usage: devpub <subject> <json-payload>
// Example: devpub thesis.actionable '{"ticker":"NVDA","conviction":0.72}'
package main

import (
	"fmt"
	"os"

	"github.com/nats-io/nats.go"

	"github.com/noeljackson/stocks/internal/platform/config"
)

func main() {
	if len(os.Args) != 3 {
		fmt.Fprintln(os.Stderr, "usage: devpub <subject> <json-payload>")
		os.Exit(2)
	}
	subject, payload := os.Args[1], os.Args[2]
	cfg := config.Load()
	nc, err := nats.Connect(cfg.NATSURL)
	if err != nil {
		fmt.Fprintln(os.Stderr, "nats connect:", err)
		os.Exit(1)
	}
	defer nc.Close()
	if err := nc.Publish(subject, []byte(payload)); err != nil {
		fmt.Fprintln(os.Stderr, "publish:", err)
		os.Exit(1)
	}
	if err := nc.Flush(); err != nil {
		fmt.Fprintln(os.Stderr, "flush:", err)
		os.Exit(1)
	}
	fmt.Println("published", subject, len(payload), "bytes")
}
