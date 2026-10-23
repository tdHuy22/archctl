# Mẫu cho thước đo hàng đợi: NATS (nats-py), aio-pika, kombu.
import os

import aio_pika
import nats
from kombu import Consumer, Exchange, Queue

SUBJECT = "orders.new"
media = Exchange("media", "direct")
video = Queue("video", exchange=media, routing_key="video")


async def nats_side(h):
    nc = await nats.connect()
    await nc.publish(SUBJECT, b"x")
    await nc.publish(subject="billing.charge", payload=b"")
    await nc.request("pricing.quote", b"")
    await nc.publish(os.getenv("NATS_SUBJECT"), b"")
    await nc.subscribe("orders.*", cb=h)
    await nc.subscribe("audit.>", queue="workers")
    await nc.subscribe(SUBJECT)
    js = nc.jetstream()
    await js.pull_subscribe("billing.charge", "durable")


async def aio_side(conn, cb):
    ch = await conn.channel()
    await ch.default_exchange.publish(aio_pika.Message(b"x"), routing_key="hello")
    logs = await ch.declare_exchange("logs", aio_pika.ExchangeType.FANOUT)
    await logs.publish(aio_pika.Message(b"y"), routing_key="")
    queue = await ch.declare_queue("hello")
    await queue.consume(cb)
    tmp = await ch.declare_queue(exclusive=True)
    await tmp.bind(logs)
    await tmp.bind("logs")
    async with queue.iterator() as it:
        pass


def kombu_side(conn, producer, body):
    producer.publish(body, exchange=media, routing_key="video")
    producer.publish(body, routing_key="jobs")
    producer.publish(body, exchange=Exchange("audit"), routing_key="")
    with Consumer(conn, queues=[video, Queue("jobs"), Queue("tasks", exchange=Exchange("work"))]):
        pass
    conn.Consumer(queues=[Queue("emails")])
