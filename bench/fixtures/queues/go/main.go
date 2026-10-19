// Mẫu cho thước đo hàng đợi: các dạng repo thật trong bộ đo không có.
package main

import (
	"os"

	"github.com/IBM/sarama"
	amqp "github.com/rabbitmq/amqp091-go"
	"github.com/redis/go-redis/v9"
	kafka "github.com/segmentio/kafka-go"
)

const Topic = "orders"

const (
	Audit        = "audit"
	Typed string = "typed"
)

var envTopic = os.Getenv("PAY_TOPIC")

func send(ch *amqp.Channel, rdb *redis.Client, p sarama.SyncProducer) {
	m := &sarama.ProducerMessage{Topic: Topic, Value: nil}
	_ = m
	w := &kafka.Writer{Addr: nil, Topic: Audit}
	_ = w
	msg := kafka.Message{Topic: Typed}
	_ = msg
	_ = kafka.Message{Value: nil}
	ch.Publish("", "jobs", false, false, amqp.Publishing{})
	ch.PublishWithContext(nil, "logs", "", false, false, amqp.Publishing{})
	q, err := ch.QueueDeclare(
		"named", false, false, false, false, nil)
	_ = err
	ch.Publish("", q.Name, false, false, amqp.Publishing{})
	rdb.Publish(nil, "news", "m")
	rdb.Publish(nil, envTopic, "m")
}

func receive(ch *amqp.Channel, rdb *redis.Client, c sarama.Consumer, g sarama.ConsumerGroup) {
	c.ConsumePartition(Topic, 0, 0)
	g.Consume(nil, []string{"pay", Audit}, nil)
	r := kafka.NewReader(kafka.ReaderConfig{Topic: "orders", GroupTopics: []string{"a", "b"}})
	_ = r
	anon, _ := ch.QueueDeclare("", false, false, true, false, nil)
	ch.Consume(anon.Name, "", true, false, false, false, nil)
	ch.Consume("jobs", "", true, false, false, false, nil)
	ch.QueueBind(anon.Name, "", "logs", false, nil)
	rdb.Subscribe(nil, "news", os.Getenv("CH"))
	rdb.PSubscribe(nil, "news.*")
}

func (b *Bus) Publish(ctx any, ch string, m any) error { return nil }

func shadow(topic string, p sarama.SyncProducer) {
	m := &sarama.ProducerMessage{Topic: topic}
	_ = m
	for _, Audit := range []string{"x"} {
		_ = &sarama.ProducerMessage{Topic: Audit}
	}
	msg := &sarama.ProducerMessage{}
	msg.Topic = "payments"
	a, b := "one", "two"
	_ = &sarama.ProducerMessage{Topic: a}
	_ = b
}

func reader() {
	cfg := kafka.ReaderConfig{Brokers: nil}
	cfg.Topic = "orders"
	var e struct{ Topic string }
	e.Topic = "payments"
	_ = cfg
}

func multiline() {
	r := kafka.NewReader(kafka.ReaderConfig{GroupTopics: []string{
		Topic,
		"pay",
	}})
	_ = r
}
