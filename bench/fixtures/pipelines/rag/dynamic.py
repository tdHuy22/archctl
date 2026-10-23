"""Dạng dựng lúc chạy: archctl phải báo, không đoán."""
from langgraph.graph import StateGraph
from langgraph.types import Command, Send

from rag.retrieval import retrieve
from rag import llm

STEPS = {"a": retrieve, "b": llm.generate}

g = StateGraph(dict)
for name, fn in STEPS.items():
    g.add_node(name, fn)
g.add_edge("a", "b")

h = StateGraph(dict)
h.add_node("first", retrieve)
h.add_node("second", make_step())
h.add_node(f"step_{1}", llm.generate)
h.add_edge("first", "second")
h.add_edge("first", "missing")
h.add_conditional_edges("first", llm.pick)
h.add_conditional_edges("first", llm.pick, {k: k for k in "ab"})


def build(step):
    s = StateGraph(dict)
    s.add_node("x", step)
    return s


def node(state):
    return Command(goto="first")


def fan(state):
    return [Send("first", x) for x in state]


seq = StateGraph(dict).add_sequence([retrieve, ("gen", llm.generate), llm.summarize])
