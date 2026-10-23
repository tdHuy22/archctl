"""Đồ thị LangGraph dạng thường gặp: tên bước bằng chuỗi, hàm nhập từ module khác."""
from typing import Literal

from langgraph.graph import END, START, StateGraph
from langgraph.prebuilt import ToolNode, tools_condition

import rag.grade as grading
from rag import llm
from rag.retrieval import retrieve as find_docs
from functools import partial


def route(state) -> Literal["generate", "rewrite", "__end__"]:
    return "generate"


def rewrite(state):
    return state


PATHS = {"yes": "generate", "no": END}

builder = StateGraph(dict)
builder.add_node("retrieve", find_docs)
builder.add_node("generate", llm.generate)
builder.add_node(grading.grade)
builder.add_node("rewrite", rewrite)
builder.add_node("tools", ToolNode([]))
builder.add_node("agent", partial(llm.agent, temperature=0))
builder.add_node(
    "summarize",
    action=llm.summarize,
)
builder.add_edge(START, "retrieve")
builder.add_edge("retrieve", "grade")
builder.add_conditional_edges("grade", route)
builder.add_conditional_edges("rewrite", llm.pick, PATHS)
builder.add_conditional_edges("agent", tools_condition)
builder.add_conditional_edges(source="generate", path=llm.pick, path_map=["summarize", END])
builder.add_edge(["tools", "rewrite"], "agent")
builder.add_edge(("summarize"), END)
graph = builder.compile()
