#!/usr/bin/env python3
"""Real HTTP acceptance checks; credentials are never printed."""
import json, os, urllib.error, urllib.parse, urllib.request

BASE=os.environ.get("LJ_TEST_URL","http://127.0.0.1:18300").rstrip("/")
TOKEN=os.environ["LJ_ACCESS_TOKEN"]

def call(path,auth=True):
    headers={"Authorization":"Bearer "+TOKEN} if auth else {}
    req=urllib.request.Request(BASE+path,headers=headers)
    try:
        with urllib.request.urlopen(req,timeout=45) as r:
            return r.status,json.load(r)
    except urllib.error.HTTPError as e:
        try: payload=json.load(e)
        except Exception: payload=None
        return e.code,payload

status,health=call("/health",False)
assert status==200 and health["embeddings"]=="disabled" and health["mode"]=="lexical_only"
assert call("/search?q=test",False)[0]==401
assert call("/corpus",False)[0]==401
status,corpus=call("/corpus")
assert status==200 and corpus["decisions"]==10 and corpus["complete"] is False
status,search=call("/search?"+urllib.parse.urlencode({"q":"obligation de sécurité","limit":10}))
assert status==200 and search["hits"] and search["queryMode"]=="lexical" and search["reranked"] is False,(status,search)
first=search["hits"][0]
assert first.get("officialUrl","").startswith("https://www.courdecassation.fr/")
assert all(hit.get("dateLecture") and hit.get("docketNumbers") and hit.get("titleHtml") for hit in search["hits"])
status,decision=call("/decision/"+first["id"])
assert status==200 and decision["id"]==first["id"]
assert decision["dateLecture"]==first["dateLecture"] and decision["docketNumbers"]==first["docketNumbers"]
assert "SYNTHETIQUE" not in json.dumps(decision,ensure_ascii=False)
for suffix in ["&mode=semantic","&mode=auto","&mode=reference","&aiMode=true"]:
    assert call("/search?q=test"+suffix)[0]==422
assert call("/search?q=test&limit=21")[0]==422
assert call("/search?"+urllib.parse.urlencode({"q":"x"*513}))[0]==422
assert call("/internal/search?q=test")[0]==404
assert call("/admin")[0]==404
print(json.dumps({"health":"passed","authentication":"passed","corpus_count":corpus["decisions"],"corpus_complete":False,"lexical_search_hits":len(search["hits"]),"decision_read":"passed","semantic_auto_reference_ai":"rejected_422","upstream_admin_internal":"not_exposed","embeddings":"disabled","model_downloads":False},ensure_ascii=False))
