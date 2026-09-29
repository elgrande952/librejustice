# LibreJustice plugin

Free and open source, [librejustice.fr](https://librejustice.fr) gives your agent **~3.8 million French-language and European decisions and ~3.9 million legal articles**, linked article by article. Four MCP tools and two skills that frame their use, from finding the applicable text to the verified citation.

## Installation

Claude Code:

```
/plugin marketplace add librejustice/librejustice
/plugin install librejustice@librejustice
```

Codex:

```
codex plugin marketplace add librejustice/librejustice
codex plugin add librejustice@librejustice
```

On first use, the agent opens the browser to connect (or create) a librejustice.fr account. OAuth 2.1 with dynamic client registration: no API key to configure, and the account is free.

## Contents

| Component | Role |
| -- | -- |
| MCP server `librejustice` | `https://librejustice.fr/mcp`: `search_decisions`, `get_decision`, `get_norm`, `search_norms` |
| Skill `recherche-jurisprudence` | find, verify and cite decisions from France's administrative and judicial courts, every one of them linked |
| Skill `recherche-normes` | the text as it applied on the relevant date: versioned French codes, treaties, EU law, foreign codes, collective agreements, BOFiP |

Four tools cover both case law and legislation, rather than one tool per fonds: every connected tool takes room in the agent's context and enters its choice at each turn.

The MCP server also works on its own. On claude.ai it is added from the [connector directory](https://claude.ai/directory/connectors/librejustice). Elsewhere (ChatGPT, Le Chat, Perplexity...), add it as a custom connector with the URL `https://librejustice.fr/mcp`.
