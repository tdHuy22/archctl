"""DAG Airflow dạng operator: `python_callable`, `>>`, `<<`, danh sách, chain."""
from datetime import datetime

from airflow import DAG
from airflow.models.baseoperator import chain, cross_downstream
from airflow.operators.bash import BashOperator
from airflow.operators.python import BranchPythonOperator, PythonOperator
from itertools import chain as ichain

from etl import steps
from etl.steps import pull


def local_check():
    return True


with DAG("etl", start_date=datetime(2026, 1, 1)) as dag:
    extract = PythonOperator(task_id="extract", python_callable=pull)
    transform = PythonOperator(task_id="transform", python_callable=steps.clean)
    load = PythonOperator(
        task_id="load",
        python_callable=steps.push,
        op_kwargs={"password": "hunter2-fixture"},
    )
    notify = BashOperator(task_id="notify", bash_command="echo done")
    check = BranchPythonOperator(task_id="check", python_callable=local_check)
    tasks = [transform, load]

    extract >> transform >> load
    load << extract
    extract >> [transform, check] >> load
    check.set_downstream(load)
    load.set_upstream([extract, check])
    chain(extract, [transform, check], [load, notify])
    cross_downstream([extract, check], [load, notify])
    ichain(extract, load)
    extract >> tasks
    notify >> load
    mask = 0x1F >> 2
    tasks[0] >> load
    assert extract >> load


def wire():
    return extract >> load
