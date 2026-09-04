# LibreJustice MCP server, local

The hosted connector at `https://librejustice.fr/mcp` speaks OAuth, which needs a browser once. This server speaks stdio and needs an API key instead, created for free from [your profile](https://librejustice.fr/profile). It calls the public REST API, which answers in the shape the tool definitions declare, so the answers are the connector's answers.

## Run it

```bash
LIBREJUSTICE_API_KEY=ljk_your_key python3 mcp/lj_mcp.py
```

Nothing to install, the standard library is enough. Or in a container, built from the repository root:

```bash
podman build -f mcp/Dockerfile -t lj-mcp .
LIBREJUSTICE_API_KEY=ljk_your_key podman run -i --rm -e LIBREJUSTICE_API_KEY lj-mcp
```

## Wire it to a client

```json
{
  "mcpServers": {
    "librejustice": {
      "command": "python3",
      "args": ["/path/to/librejustice/mcp/lj_mcp.py"],
      "env": { "LIBREJUSTICE_API_KEY": "ljk_your_key" }
    }
  }
}
```

`LIBREJUSTICE_API_BASE_URL` points elsewhere than `https://librejustice.fr` when you run your own instance.

The tests need no checkout of the rest of the workspace:

```bash
uv run --with pytest pytest mcp -q
```

## What it serves

Four tools: `search_decisions`, `search_legal_texts`, `get_decision`, `get_legal_text`. Their names, descriptions and schemas are read from `packages/lj-dtos/data/mcp-tools.json`, which the hosted server serialises from its own definitions, so a client, or a directory, enumerates the tools without presenting a key. A call needs one, and lands on `/api`: the budget is attached to the account the key names, shared with searches made on the site.

The connector's fifth tool, `list_my_activity`, is not served here. It reports the searches, bookmarks and readings the site and the connector recorded for the account, which this process neither writes nor reads.
