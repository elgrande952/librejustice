"""What the mapping onto `/api` has to hold: one route per tool, four doors on `get_legal_text`, and the tool spellings translated to the query keys."""

import json
from pathlib import Path

import pytest

import lj_mcp

OPENAPI = Path(lj_mcp.__file__).resolve().parent.parent / "packages/lj-web/public/openapi.json"

DOCUMENTED = {
    "/api/search": "search_decisions",
    "/api/search-textes": "search_legal_texts",
    "/api/decision/{id}": "get_decision",
    "/api/texte/{code}": "get_legal_text",
    "/api/texte/{code}/section/{cid}": "get_legal_text",
    "/api/texte/{code}/{article}": "get_legal_text",
    "/api/article/{uid}": "get_legal_text",
}


def test_the_served_tools_are_the_ones_mapped():
    listed = {t["name"] for t in lj_mcp.dispatch({"method": "tools/list"}, {})["tools"]}
    assert listed == set(lj_mcp.ROUTES)
    assert "list_my_activity" not in listed


def test_the_instructions_come_from_the_shared_surface():
    initialized = lj_mcp.dispatch({"method": "initialize"}, {})
    assert initialized["instructions"] == lj_mcp._surface["instructions"]
    assert initialized["serverInfo"]["name"] == "librejustice"


def test_a_search_translates_its_scalars_and_repeats_its_filters():
    path, query = lj_mcp.search_decisions(
        {
            "query": "trouble de voisinage",
            "date_from": "2020-01-01",
            "jurisdiction_type": ["CA", "TJ"],
            "legal_article": ["code-civil/1240"],
            "limit": 5,
        }
    )
    assert path == "/api/search"
    assert query == [
        ("q", "trouble de voisinage"),
        ("dateFrom", "2020-01-01"),
        ("limit", "5"),
        ("jurisdictionType", "CA"),
        ("jurisdictionType", "TJ"),
        ("legalArticle", "code-civil/1240"),
        ("aiMode", "1"),
    ]


def test_the_rerank_is_on_unless_it_is_refused():
    _, on = lj_mcp.search_decisions({"query": "x"})
    _, off = lj_mcp.search_decisions({"query": "x", "ai_rerank": False})
    assert ("aiMode", "1") in on
    assert not [v for k, v in off if k == "aiMode"]


def test_a_decision_is_addressed_by_the_url_of_a_hit():
    path, query = lj_mcp.get_decision({"url": "https://librejustice.fr/decision/HrsWxKX8KYyC"})
    assert (path, query) == ("/api/decision/HrsWxKX8KYyC", [])
    with pytest.raises(ValueError, match="not a decision url"):
        lj_mcp.get_decision({"url": "/texte/code-civil/1240"})


@pytest.mark.parametrize(
    ("args", "expected"),
    [
        ({"url": "/texte/code-civil/1240"}, "/api/texte/code-civil/1240"),
        ({"url": "https://librejustice.fr/texte/code-civil"}, "/api/texte/code-civil"),
        (
            {"url": "/texte/code-civil/section/LEGISCTA000006117617"},
            "/api/texte/code-civil/section/LEGISCTA000006117617",
        ),
        ({"code": "code civil", "article": "L. 121-1"}, "/api/texte/code%20civil/L.%20121-1"),
        ({"code": "cgi"}, "/api/texte/cgi"),
        ({"textId": "LEGIARTI000006419282"}, "/api/article/LEGIARTI000006419282"),
        ({"url": "LEGIARTI000006419282"}, "/api/article/LEGIARTI000006419282"),
    ],
)
def test_the_four_doors_of_a_legal_text(args, expected):
    assert lj_mcp.legal_text_path(args) == expected


def test_a_legal_text_needs_a_designation():
    with pytest.raises(ValueError, match="missing tool argument: url"):
        lj_mcp.legal_text_path({"date": "2020-01-01"})


def test_a_date_rides_along_whichever_door_was_used():
    asked = lj_mcp.get_legal_text({"code": "code-civil", "article": "1240", "date": "2015-06-01"})
    assert asked == ("/api/texte/code-civil/1240", [("date", "2015-06-01")])


def test_a_call_without_a_key_names_where_to_get_one():
    with pytest.raises(ValueError, match="librejustice.fr/profile"):
        lj_mcp.call(
            {"name": "search_decisions", "arguments": {"query": "x"}},
            {"api_key": "", "base_url": "https://librejustice.fr"},
        )


def test_an_unknown_tool_is_refused_before_the_network():
    with pytest.raises(ValueError, match="unknown tool: list_my_activity"):
        lj_mcp.call({"name": "list_my_activity"}, {"api_key": "k", "base_url": "x"})


def test_the_rest_contract_answers_with_the_tool_output_schemas():
    paths = json.loads(OPENAPI.read_text(encoding="utf-8"))["paths"]
    schemas = {t["name"]: t.get("outputSchema") for t in lj_mcp._surface["tools"]}
    assert set(paths) == set(DOCUMENTED)
    assert set(DOCUMENTED.values()) == set(lj_mcp.ROUTES)
    for path, tool in DOCUMENTED.items():
        served = paths[path]["get"]["responses"]["200"]["content"]["application/json"]["schema"]
        assert served == schemas[tool], path
