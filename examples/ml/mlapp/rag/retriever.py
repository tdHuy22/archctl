import openai  # vi phạm: gọi thẳng nhà cung cấp, bỏ qua cổng LLM

from mlapp.llm.gateway import complete


def embed(text: str):
    return openai.embeddings.create(model="emb-x", input=text)


def answer(question: str) -> str:
    return complete(question)
