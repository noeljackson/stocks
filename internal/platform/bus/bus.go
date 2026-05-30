// Package bus wraps NATS + JetStream (SPEC §3: durable, replayable streams).
//
// Publish is JetStream-only (durable). Subscribers should use Consume, which
// creates a JetStream durable consumer — at-least-once delivery, replays on
// restart, cursor is server-side. Plain NC is exposed for the rare case
// (health-check / fire-and-forget) but production paths should go via Consume.
package bus

import (
	"context"
	"fmt"

	"github.com/nats-io/nats.go"
	"github.com/nats-io/nats.go/jetstream"
)

type Bus struct {
	NC *nats.Conn
	JS jetstream.JetStream
}

// Connect dials NATS and initializes the JetStream context.
func Connect(url string) (*Bus, error) {
	nc, err := nats.Connect(url, nats.MaxReconnects(-1), nats.Name("stocks"))
	if err != nil {
		return nil, err
	}
	js, err := jetstream.New(nc)
	if err != nil {
		nc.Close()
		return nil, err
	}
	return &Bus{NC: nc, JS: js}, nil
}

func (b *Bus) Close() {
	if b.NC != nil {
		b.NC.Close()
	}
}

// EnsureStream idempotently creates/updates a file-backed stream over subjects.
// Producers and consumers should both call this for the stream they touch —
// it's idempotent and avoids startup races.
func (b *Bus) EnsureStream(ctx context.Context, name string, subjects ...string) error {
	_, err := b.JS.CreateOrUpdateStream(ctx, jetstream.StreamConfig{
		Name:     name,
		Subjects: subjects,
		Storage:  jetstream.FileStorage,
	})
	return err
}

// Publish persists a message to JetStream. Errors if no stream covers subject.
func (b *Bus) Publish(ctx context.Context, subject string, data []byte) error {
	_, err := b.JS.Publish(ctx, subject, data)
	return err
}

// Consume creates (or updates) a durable JetStream consumer on the named
// stream, filtered to filterSubject, and starts dispatching to handler. The
// returned stop function drains the consumer and unblocks.
//
// Acking policy: handler returning nil → Ack; non-nil error → Nak (redeliver).
// Durable names should be stable per service (e.g. "gateway-thesis-alerts"),
// since they're the server-side cursor identity.
func (b *Bus) Consume(ctx context.Context, stream, durable, filterSubject string, handler func(jetstream.Msg) error) (func(), error) {
	s, err := b.JS.Stream(ctx, stream)
	if err != nil {
		return nil, fmt.Errorf("stream %s: %w", stream, err)
	}
	cons, err := s.CreateOrUpdateConsumer(ctx, jetstream.ConsumerConfig{
		Durable:       durable,
		FilterSubject: filterSubject,
		AckPolicy:     jetstream.AckExplicitPolicy,
		MaxDeliver:    5,
	})
	if err != nil {
		return nil, fmt.Errorf("consumer %s/%s: %w", stream, durable, err)
	}
	cctx, err := cons.Consume(func(m jetstream.Msg) {
		if err := handler(m); err != nil {
			_ = m.Nak()
			return
		}
		_ = m.Ack()
	})
	if err != nil {
		return nil, fmt.Errorf("consume %s/%s: %w", stream, durable, err)
	}
	return cctx.Stop, nil
}
