#!/usr/bin/env bash
# Debug: compare tokscale output with estimator output for Freebuff accounts
set -euo pipefail

TMP=$(ls -dt /tmp/tokscale-* 2>/dev/null | head -1 || echo "/tmp/tokscale-debug")
if [ ! -d "$TMP" ]; then
  echo "No TMP dir found, running a quick estimate-only scan..."
  exit 1
fi

echo "=== Tokscale Freebuff files ==="
ls -la "$TMP"/freebuff-*.json 2>/dev/null | head -10

echo ""
echo "=== Tokscale entries (sample) ==="
for f in "$TMP"/freebuff-*.json; do
  n=$(basename "$f" .json)
  echo "--- $n ---"
  python3 -c "
import json
j = json.load(open('$f'))
for e in j.get('entries', [])[:3]:
    sid = e.get('sessionId', '?')
    inp = e.get('input') or e.get('inputTokens', 0)
    print(f'  sid={sid} input={inp}')
"
done

echo ""
echo "=== Estimator output ==="
python3 -c "
import json
sessions = json.load(open('/tmp/est.json'))
for s in sessions[:3]:
    print(f\"  acct={s['account']} sid={s['sessionId']} input={s['input']}\")
print(f'total: {len(sessions)}')
"