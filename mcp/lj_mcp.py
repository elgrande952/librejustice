#!/usr/bin/env python3
"""LibreJustice MCP server over stdio, backed by the public REST API.

The tool surface is read from `mcp-tools.json`, which the hosted server
serialises from its own definitions, so the two servers describe their tools
with the same words. Each tool maps onto one route of `/api`, which answers in
the shape those definitions declare.

`LIBREJUSTICE_API_KEY` is created from https://librejustice.fr/profile. A
directory can enumerate the tools without one; a call needs it.
"""

import json
import os
import sys
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

PROTOCOL_VERSION = "2024-11-05"
SERVER_NAME = "librejustice"
VERSION = "0.1.0"

SURFACE = Path(__file__).resolve().parent.parent / "packages/lj-dtos/data/mcp-tools.json"

_surface = json.loads(SURFACE.read_text(encoding="utf-8"))

# Repeatable filters of `search_decisions`, whose query keys are camelCase.
SEARCH_LISTS = {
    "jurisdiction_type": "jurisdictionType",
    "jurisdiction_code": "jurisdictionCode",
    "legal_domain": "legalDomain",
    "legal_instrument": "legalInstrument",
    "legal_article": "legalArticle",
    "solution": "solution",
    "procedure": "procedure",
    "office": "office",
    "seat": "seat",
    "publication": "publication",
}
SEARCH_SCALARS = {
    "query": "q",
    "date_from": "dateFrom",
    "date_to": "dateTo",
    "mode": "mode",
    "sort": "sort",
    "limit": "limit",
}
TEXTS_SCALARS = {
    "query": "q",
    "code": "code",
    "jurisdiction": "jurisdiction",
    "date": "date",
    "limit": "limit",
}


def as_text(value):
    if isinstance(value, bool):
        return "1" if value else "0"
    return str(value)


def scalars(args, mapping):
    return [
        (key, as_text(args[name]))
        for name, key in mapping.items()
        if args.get(name) not in (None, "")
    ]


def search_decisions(args):
    params = scalars(args, SEARCH_SCALARS)
    for name, key in SEARCH_LISTS.items():
        for value in args.get(name) or []:
            params.append((key, as_text(value)))
    # The tool reranks by default, the route does not: the flag has to be sent.
    if args.get("ai_rerank", True):
        params.append(("aiMode", "1"))
    return "/api/search", params


def search_legal_texts(args):
    return "/api/search-textes", scalars(args, TEXTS_SCALARS)


def get_decision(args):
    url = args.get("url") or ""
    _, sep, tail = url.rpartition("/decision/")
    if not sep or not tail:
        raise ValueError(f"not a decision url (expected …/decision/{{id}}): {url or '(missing)'}")
    return f"/api/decision/{urllib.parse.quote(tail.strip('/'), safe='')}", []


def get_legal_text(args):
    date = [("date", args["date"])] if args.get("date") else []
    return legal_text_path(args), date


def legal_text_path(args):
    """The four doors of `get_legal_text`, in the order it reads them."""
    url = args.get("url") or ""
    ident = args.get("textId") or ""
    if url.startswith("LEGIARTI") or ident:
        return f"/api/article/{urllib.parse.quote(ident or url, safe='')}"
    if url:
        _, sep, tail = url.rpartition("/texte/")
        if not sep or not tail:
            raise ValueError(
                f"not a legal text url (expected …/texte/{{code}}[/{{article}}]): {url}"
            )
        return "/api/texte/" + tail.strip("/")
    code = args.get("code")
    if not code:
        raise ValueError(
            "missing tool argument: url. Address the text by url "
            "(/texte/{code}/{article}), by the pair {code, article}, or by a LEGIARTI id."
        )
    path = "/api/texte/" + urllib.parse.quote(code, safe="")
    article = args.get("article")
    return f"{path}/{urllib.parse.quote(article, safe='')}" if article else path


# A tool absent here is absent from `tools/list`: `list_my_activity` reports what
# the site and the hosted connector recorded, which this process cannot read.
ROUTES = {
    "search_decisions": search_decisions,
    "search_legal_texts": search_legal_texts,
    "get_decision": get_decision,
    "get_legal_text": get_legal_text,
}


def dispatch(request, settings):
    method = request.get("method", "")
    if method == "initialize":
        return {
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {"tools": {}},
            "serverInfo": {"name": SERVER_NAME, "version": VERSION},
            "instructions": _surface["instructions"],
        }
    if method == "ping":
        return {}
    if method == "tools/list":
        return {"tools": [t for t in _surface["tools"] if t["name"] in ROUTES]}
    if method == "tools/call":
        return call(request.get("params") or {}, settings)
    raise ValueError(f"unknown method: {method}")


def call(params, settings):
    name = params.get("name", "")
    route = ROUTES.get(name)
    if route is None:
        raise ValueError(f"unknown tool: {name}")
    path, query = route(params.get("arguments") or {})
    payload = fetch(path, query, settings)
    return {
        "content": [{"type": "text", "text": json.dumps(payload, ensure_ascii=False)}],
        "structuredContent": payload,
        "isError": False,
    }


class NoRedirect(urllib.request.HTTPRedirectHandler):
    """`urllib` strips only the content headers on a redirect, so it would carry
    the key to whatever host answers. The API serves its routes directly."""

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


OPENER = urllib.request.build_opener(NoRedirect)


def fetch(path, query, settings):
    if not settings["api_key"]:
        raise ValueError(
            "no API key: set LIBREJUSTICE_API_KEY, created from https://librejustice.fr/profile"
        )
    url = settings["base_url"] + path
    if query:
        url += "?" + urllib.parse.urlencode(query)
    asked = urllib.request.Request(
        url,
        headers={
            "authorization": f"Bearer {settings['api_key']}",
            "accept": "application/json",
            "user-agent": f"lj-mcp/{VERSION}",
        },
    )
    try:
        with OPENER.open(asked, timeout=120) as response:
            return json.load(response)
    except urllib.error.HTTPError as error:
        raise ValueError(f"{error.code}: {error.read().decode('utf-8', 'replace')}") from error


def main():
    if sys.stdin.isatty():
        sys.exit(
            "lj-mcp speaks JSON-RPC on stdin: set LIBREJUSTICE_API_KEY and let a client drive it"
        )
    settings = {
        "api_key": os.environ.get("LIBREJUSTICE_API_KEY", ""),
        "base_url": os.environ.get("LIBREJUSTICE_API_BASE_URL", "https://librejustice.fr").rstrip(
            "/"
        ),
    }
    for raw in sys.stdin:
        line = raw.strip()
        if not line:
            continue
        request = json.loads(line)
        if "id" not in request:
            continue
        try:
            body = {"jsonrpc": "2.0", "id": request["id"], "result": dispatch(request, settings)}
        except Exception as error:
            body = {
                "jsonrpc": "2.0",
                "id": request["id"],
                "error": {"code": -32603, "message": str(error)},
            }
        print(json.dumps(body, ensure_ascii=False), flush=True)


if __name__ == "__main__":
    main()
