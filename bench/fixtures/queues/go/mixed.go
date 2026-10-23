// Mẫu cho thước đo hàng đợi: NATS cùng amqp091 và go-redis trong một file.
package main

import (
	"github.com/nats-io/nats.go"
	amqp "github.com/rabbitmq/amqp091-go"
	"github.com/redis/go-redis/v9"
)

type Bus struct {
	conn *nats.Conn
}

func mixed(b *Bus, ch *amqp.Channel, rdb *redis.Client, msg amqp.Publishing) {
	nc, _ := nats.Connect("nats://x")
	js, _ := nc.JetStream()
	nc.Publish("orders.new", nil)
	b.conn.Subscribe("orders.*", nil)
	js.Publish("audit.log", nil)
	ch.Publish("", "jobs", false, false, msg)
	rdb.Publish(ctx, "events", "x")
	rdb.Subscribe(ctx, "events")
}

func more(a, b *nats.Conn, s *Bus, client *redis.Client) {
	a.Publish("x.one", nil)
	b.Publish("x.two", nil)
	s.bus.Publish("orders.shipped", nil)
	client.Publish(ctx, "events", "x")
}

func price(s *Svc) {
	s.conn.Request("pricing.quote", nil, 0)
}
