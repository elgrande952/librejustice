"""Fail closed on known model-download build hooks in the resolved API graph.
This is a targeted guard, not a complete dependency security audit.
Cargo fetch downloads source packages only; no build script is executed by fetch.
"""
import json
import re
import subprocess
from pathlib import Path

metadata = json.loads(subprocess.check_output(['cargo', 'metadata', '--locked', '--format-version', '1']))
packages = {p['id']: p for p in metadata['packages']}
nodes = {n['id']: n for n in metadata['resolve']['nodes']}
root = next(p['id'] for p in metadata['packages'] if p['name'] == 'lj-api')
seen, pending = set(), [root]
while pending:
    current = pending.pop()
    if current in seen:
        continue
    seen.add(current)
    pending.extend(d['pkg'] for d in nodes[current]['deps'])
assert all(packages[item]['name'] != 'lj-ingest' for item in seen), 'lj-ingest tokenizer download is forbidden'
pattern = re.compile(r'huggingface\.co|hf_hub|safetensors|\.gguf|Qwen3-Embedding|tokenizer\.json', re.I)
audited = []
for item in sorted(seen):
    package = packages[item]
    for target in package['targets']:
        if 'custom-build' in target['kind']:
            path = Path(target['src_path'])
            text = path.read_text()
            assert not pattern.search(text), f'Model-related build hook needs review: {package["name"]}'
            audited.append(package['name'])
Path('smoke-evidence/build-hook-audit.json').write_text(json.dumps({
    'api_dependency_packages': len(seen), 'build_script_packages': sorted(audited),
    'scope': 'known model download terms in reachable custom-build entrypoints; not full supply-chain audit',
}, indent=2) + '\n')
print(f'Build hook guard: {len(audited)} reachable entrypoints checked; lj-ingest excluded.')
