"""Đồ thị LangGraph của RAG: bước tìm tài liệu rồi bước gọi cổng LLM. Hai bước
không import nhau; archctl đọc cạnh từ `add_edge` và xét `allow Rag -> LLM`."""
from langgraph.graph import END, START, StateGraph

from mlapp.llm.gateway import complete
from mlapp.rag.retriever import answer

builder = StateGraph(dict)
builder.add_node("retrieve", answer)
builder.add_node("generate", complete)
builder.add_edge(START, "retrieve")
builder.add_edge("retrieve", "generate")
builder.add_edge("generate", END)
graph = builder.compile()
