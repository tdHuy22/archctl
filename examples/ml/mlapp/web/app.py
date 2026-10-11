from mlapp.llm.gateway import LLMUnavailable
from mlapp.rag.retriever import answer


def handle(question: str) -> dict:
    try:
        return {"answer": answer(question)}
    except LLMUnavailable:
        return {"error": "thử lại sau"}
    except RateLimitError:  # vi phạm: lỗi của nhà cung cấp lọt ra tầng web
        return {"error": "quá tải"}
