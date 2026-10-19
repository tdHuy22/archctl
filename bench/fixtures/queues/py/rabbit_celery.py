import pika
from celery import Celery, shared_task

app = Celery("shop")


def main(ch):
    ch.basic_publish(exchange="", routing_key="jobs", body=b"x")
    ch.basic_publish("logs", "", b"x")
    ch.basic_publish(exchange="topic_logs", routing_key="a.b", body=b"")
    ch.basic_consume(queue="jobs", on_message_callback=print)
    r = ch.queue_declare(queue="named")
    ch.basic_consume(queue=r.method.queue, on_message_callback=print)
    anon = ch.queue_declare(queue="", exclusive=True)
    ch.basic_consume(queue=anon.method.queue, on_message_callback=print)
    ch.queue_bind(exchange="logs", queue=anon.method.queue)
    app.send_task("shop.tasks.charge", args=[1])
    app.send_task(name="custom.bill")


@shared_task
def refund(x):
    pass


@app.task(name="custom.bill", bind=True)
def bill(self):
    pass


@app.task
@other_decorator
def ship():
    pass
