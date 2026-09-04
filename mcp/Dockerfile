# Build context is the repository root: the server reads the tool surface that
# `lj-dtos` serialises, and the two files ship together.
FROM python:3.14-slim
WORKDIR /srv
COPY mcp/lj_mcp.py mcp/
COPY packages/lj-dtos/data/mcp-tools.json packages/lj-dtos/data/
USER nobody
ENTRYPOINT ["python3", "mcp/lj_mcp.py"]
