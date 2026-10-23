"""TaskFlow: thân `@task` và hàm `python_callable` không nối đồ thị; `;`, `case`."""
from airflow import DAG
from airflow.decorators import dag, task
from airflow.operators.python import PythonOperator

from etl.steps import clean, pull, push


@dag()
def flow():
    @task
    def inner(x):
        return push(clean(x))
    a = pull(); b = inner(a)
    c = clean(b)


with DAG("d"):
    def _run():
        data = pull()
        return push(data)
    t = PythonOperator(task_id="t", python_callable=_run); u = PythonOperator(task_id="u", python_callable=clean); t >> u
    match 1:
        case 1: u >> t
