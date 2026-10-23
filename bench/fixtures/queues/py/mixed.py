# Mẫu cho thước đo hàng đợi: file import nhiều thư viện dùng chung tên phương
# thức. `publish`, `subscribe`, `request` chỉ là NATS khi biến chắc là client NATS.
import aio_pika
import nats
import redis
import requests
from kafka import KafkaConsumer


async def mixed(r, consumer, ch, url):
    nc = await nats.connect()
    js = nc.jetstream()
    bus = await nats.connect(servers=["nats://x"])
    await nc.publish("orders.new", b"x")
    await bus.subscribe("orders.*")
    await js.pull_subscribe("audit.log", "d")
    r.publish("cache-events", "hi")
    r.subscribe("cache-events")
    consumer.subscribe(["orders"])
    requests.request("GET", url)
    await nc.request("pricing.quote", b"")
    await ch.default_exchange.publish(aio_pika.Message(b"x"), routing_key="jobs")


class Handler:
    async def handle(self, bus: nats.NATS, r2):
        await bus.publish("orders.paid", b"x")
        await self.client.publish("orders.shipped", b"x")
        rds = redis.Redis()
        rds.publish("cache-events", "x")


async def pricing(client):
    await client.request("pricing.quote", b"")
