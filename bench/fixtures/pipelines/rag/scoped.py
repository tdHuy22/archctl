"""Phạm vi tên, lớp bọc, bí danh: các dạng review 2026-10-23 tìm ra."""
from langgraph.graph import StateGraph
from langgraph.prebuilt import ToolNode
from langgraph.types import Command as Go
from functools import partial

from rag import llm
from rag.grade import grade as check
from rag.retrieval import retrieve
from rag.retrieval import retrieve as fetch


class Helper:
    def retrieve(self):
        return None


def build_a():
    workflow = StateGraph(dict)
    workflow.add_node("retrieve", retrieve)
    workflow.add_node("generate", lambda s: llm.generate(s))
    workflow.add_node("tools", ToolNode([fetch, check]))
    workflow.add_node("wrapped", partial(llm.agent, temperature=0))
    workflow.add_edge("retrieve", "generate")
    workflow.add_edge("generate", "tools")
    workflow.add_edge("tools", "wrapped")
    return workflow


def build_b():
    workflow = StateGraph(dict)
    workflow.add_node("retrieve", llm.generate)
    workflow.add_node("generate", llm.summarize)
    workflow.add_edge("retrieve", "generate")
    edge = workflow.add_edge
    edge("generate", "retrieve")
    return workflow


def route(state):
    return Go(goto="retrieve")


def route_positional(state):
    return Go(None, {}, None, "retrieve")


def helper(g):
    g.add_edge("retrieve", "generate")
