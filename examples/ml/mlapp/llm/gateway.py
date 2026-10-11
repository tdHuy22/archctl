"""Cổng duy nhất gọi nhà cung cấp LLM: thử lại, đổi nhà cung cấp, ghi chi phí ở đây."""
from openai import APITimeoutError, OpenAI, RateLimitError

client = OpenAI()


class LLMUnavailable(Exception):
    pass


def complete(prompt: str) -> str:
    try:
        return client.responses.create(model="gpt-x", input=prompt).output_text
    except (RateLimitError, APITimeoutError) as err:
        raise LLMUnavailable() from err
