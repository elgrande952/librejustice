# LibreJustice — agent install guide

This guide is written for an AI agent installing LibreJustice on
behalf of its user — typically inside a desktop app (ChatGPT Desktop,
Claude Desktop…) or any agent able to edit its own configuration or
run commands. LibreJustice is a remote MCP server for searching
French and European case law and statutes, plus two method skills.

## 1. Connect the MCP server

- Endpoint: `https://librejustice.fr/mcp`
- Transport: streamable HTTP
- Auth: OAuth 2.1, auto-discovered by the client. The browser flow
  creates a free account if the user does not have one.

Any client that reads a JSON MCP config — add to `mcpServers`:

```json
{"librejustice": {"type": "http", "url": "https://librejustice.fr/mcp"}}
```

Agents with a plugin system (Claude Code, Codex) — the plugin installs
the connector and the skills together (then skip step 2):

```
/plugin marketplace add librejustice/librejustice
/plugin install librejustice@librejustice
```

```
codex plugin marketplace add librejustice/librejustice
codex plugin add librejustice@librejustice
```

## 2. Install the skills

The two skills teach the research method (read decisions in full
before citing, fetch statutes as of the relevant date, corpus traps).
The connector works without them, but answers are markedly more
reliable with them.

```
npx skills add librejustice/librejustice
```

or copy the `skills/recherche-jurisprudence` and
`skills/recherche-normes` folders of this repository into the agent's
skills directory.

## 3. Verify

Call the `search_decisions` tool with the query « infection nosocomiale
responsabilité » — it must return results. If the client asks for OAuth
authorization, open the flow and wait for the user to approve it.

## If you cannot install

If you can neither run commands nor edit your own configuration, say
so and point the user to <https://librejustice.fr/mcp-guide> — it has
step-by-step instructions per client (Claude Web, ChatGPT Web, Le Chat,
Perplexity).
