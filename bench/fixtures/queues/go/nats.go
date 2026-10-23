// Mẫu cho thước đo hàng đợi: NATS (nats.go), cả JetStream mới.
package main

import (
	"context"

	"github.com/nats-io/nats.go"
)

const Subject = "orders.new"

func natsSide(nc *nats.Conn, js JetStream, ctx context.Context, h nats.MsgHandler) {
	nc.Publish(Subject, nil)
	nc.Request("pricing.quote", nil, 0)
	nc.Subscribe("orders.*", h)
	nc.QueueSubscribe("audit.>", "workers", h)
	nc.ChanSubscribe(Subject, nil)
	js.Publish(ctx, "billing.charge", nil)
	js.Publish(context.Background(), "billing.refund", nil)
	js.PullSubscribe("billing.charge", "durable")
}
