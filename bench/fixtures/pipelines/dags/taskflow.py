"""DAG Airflow dạng TaskFlow: hàm được gọi là hàm của task."""
from airflow.decorators import dag, task
from airflow.utils.task_group import TaskGroup

from etl.steps import clean, pull, push


@task
def report(x):
    return x


@dag()
def flow():
    a = pull()
    b = clean.override(task_id="clean_2")()
    c = push.partial(x=1).expand(y=[1, 2])
    a >> b >> c
    b >> report(b)
    with TaskGroup("grp") as grp:
        pass
    grp >> c
    unknown_task >> c
    lam = lambda: 1
    lam >> c


flow()
