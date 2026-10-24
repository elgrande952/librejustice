"""Real HTTP probes against a local API instance with exactly one synthetic row."""
import json
import urllib.error
import urllib.request
from pathlib import Path

BASE = 'http://127.0.0.1:18300'
OUT = Path('smoke-evidence')
OUT.mkdir(exist_ok=True)

def read(path):
    with urllib.request.urlopen(BASE + path, timeout=45) as response:
        return response.status, json.load(response)

status, health = read('/internal/health')
assert status == 200 and health.get('status') == 'ok', health
status, result = read('/internal/search?q=solfablefixtureverifiable&mode=lexical&aiMode=0&limit=1')
serialized = json.dumps(result, ensure_ascii=False)
assert status == 200 and 'SfSmoke00001' in serialized, result
assert 'SYNTHETIQUE' in serialized.upper() or 'SYNTHÉTIQUE' in serialized.upper(), result
try:
    read('/api/search?q=solfablefixtureverifiable&mode=lexical&aiMode=0&limit=1')
    raise AssertionError('Public API accepted unauthenticated search')
except urllib.error.HTTPError as exc:
    assert exc.code in (401, 403), exc.code
    auth_status = exc.code
receipt = {
    'scope': 'ephemeral independent API, synthetic fixture, lexical only',
    'upstream_commit': 'ca1d06062ec4bb2cb332b0e9d41a45309fa3ad75',
    'health': health,
    'lexical_fixture_found': True,
    'public_api_without_key_http_status': auth_status,
    'synthetic_fixture_count': 1,
    'semantic_search_validated': False,
    'production_deployed': False,
    'full_corpus_installed': False,
    'frontend_validated': False,
    'model_downloaded': False,
}
(OUT / 'result.json').write_text(json.dumps(receipt, ensure_ascii=False, indent=2) + '\n')
(OUT / 'synthetic-search.json').write_text(json.dumps(result, ensure_ascii=False, indent=2) + '\n')
print(json.dumps(receipt, ensure_ascii=False, indent=2))
