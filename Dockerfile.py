# syntax=docker/dockerfile:1
# Python services (context maintainer, etc.).

FROM python:3.14-slim
WORKDIR /app
COPY py/pyproject.toml ./
COPY py/src ./src
RUN pip install --no-cache-dir .
USER 65532:65532
ENTRYPOINT ["python", "-m", "stocks.context_maintainer"]
