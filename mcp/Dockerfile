# Build context is the repository root: the server reads the tool surface that
# `lj-mcp-tools` serialises, and the two files ship together.
FROM python:3.14-slim
WORKDIR /srv
COPY mcp/lj_mcp.py mcp/
COPY packages/lj-mcp-tools/data/mcp-tools.json packages/lj-mcp-tools/data/
USER nobody
ENTRYPOINT ["python3", "mcp/lj_mcp.py"]
