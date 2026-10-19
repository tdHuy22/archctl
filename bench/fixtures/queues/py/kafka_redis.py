# Mẫu cho thước đo hàng đợi: các dạng repo thật trong bộ đo không có.
from confluent_kafka import Producer, Consumer
from kafka import KafkaConsumer
import os
import redis

TOPIC = "orders"
AUDIT: str = "audit"
ENV_TOPIC = os.getenv("PAY_TOPIC", "pay")
TWICE = "a"
TWICE = "b"


class Bus:
    def __init__(self):
        self.topic = "events"

    def send(self, p, v, t):
        p.produce(TOPIC, v)
        p.produce(topic=AUDIT, value=v)
        p.produce(self.topic, v)
        p.produce(f"orders-{t}", v)
        p.produce(f"plain", v)
        p.produce(TWICE, v)
        p.produce(os.environ["RAW_TOPIC"], v)
        p.produce(ENV_TOPIC, v)
        get_producer().produce(
            "late", v)

    def receive(self, c, r):
        c.subscribe([TOPIC, "audit", ENV_TOPIC])
        c.subscribe(topics=("x", "y"))
        c.subscribe(pattern="^orders.*")
        KafkaConsumer("k1", "k2", bootstrap_servers="b:9092")
        r.publish("news", "m")
        r.publish(channel="alerts", message="m")
        ps = r.pubsub()
        ps.subscribe("news", "alerts")
        ps.psubscribe("news.*")


def send_task(self, message):
    pass


# Phạm vi: tham số, biến vòng lặp, keyword xuống dòng trùng tên hằng số.
SHADOW = "audit"


def shadowed(p, SHADOW, scoped):
    p.produce(SHADOW, 1)
    for LOOPED in ["payments"]:
        p.produce(LOOPED, 1)
    cfg = dict(
        scoped="x"
    )
    p.produce(scoped, cfg)


LOOPED = "audit"


def by_reference(p, r):
    send = p.produce
    getattr(p, "produce")("payments", 1)
    r.pubsub().subscribe(payments=print)
    r.pubsub().psubscribe(**{"a.*": print})


def generic_send(ws, producer, t):
    ws.send(t)
    producer.send(t, 1)
    p2 = KafkaProducer()
    p2.send(t, 1)


class Annotated:
    def __init__(self):
        self.client: KafkaProducer = KafkaProducer()

    def f(self, t):
        self.client.send(t, 1)
        other = make()
        other.send(os.getenv("ORDERS_TOPIC"), 1)
