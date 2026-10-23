"""Nhóm task, TaskFlow truyền qua đối số, `>>` lồng, các lời gọi nối khác."""
import logging

from airflow.decorators import dag, task
from airflow.models.baseoperator import chain as link
from airflow.models.baseoperator import chain_linear, cross_downstream
from airflow.operators.bash import BashOperator
from airflow.operators.python import PythonOperator
from airflow.utils.task_group import TaskGroup

from etl.steps import clean, pull, push


@dag()
def grouped():
    start = PythonOperator(task_id="start", python_callable=pull)
    with TaskGroup("work") as work:
        a = PythonOperator(task_id="a", python_callable=clean)
        b = PythonOperator.partial(task_id="b", python_callable=push).expand(op_args=[[1], [2]])
    start >> work
    load = push(clean(pull()))
    x = clean.override(task_id="c2")(load)
    link(start, a)
    chain_linear(start, [a, b])
    cross_downstream(to_tasks=[a], from_tasks=[start])
    sh = BashOperator(task_id="sh", bash_command="python etl/run.py --token x")
    sh >> b
    other = dag.get_task("a")
    other >> b
    logging.info("%s", start >> b)
    y = z = start >> a
    start \
        >> b
    start >> a if True else b
    a.set_dependency("a", "b")
    up = a.set_upstream
    return x
