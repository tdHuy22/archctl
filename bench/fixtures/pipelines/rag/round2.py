"""Dạng review vòng 2 (2026-10-23) tìm ra: gán lại builder, builder là tham
số, bọc lại cùng tên, danh sách tool qua biến, lambda nhiều tham số."""
from langgraph.graph import StateGraph
from langgraph.prebuilt import ToolNode
from langsmith import traceable

from rag import llm
from rag.grade import grade
from rag.retrieval import retrieve

retrieve = traceable(retrieve)
tools = [grade, llm.summarize]

g = StateGraph(dict)
g.add_node("r", retrieve)
g.add_node("t", ToolNode(tools))
g.add_node("l", lambda state, config: llm.generate(state, config))
g.add_edge("r", "t")
g.add_edge("t", "l")
app = g.compile()
g = StateGraph(dict)
g.add_node("r", llm.summarize)


def extend(g, tools):
    g.add_node("x", ToolNode(tools))
    g.add_edge("r", "x")
