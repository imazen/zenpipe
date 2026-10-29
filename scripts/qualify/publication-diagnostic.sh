#!/usr/bin/env bash
# First compile gate after a fresh serialized grant. Never suppress the native
# compiler diagnostic behind the public-API snapshot wrapper's summary.
set -euo pipefail
results=${1:?absolute new diagnostic directory required}
[[ "$results" = /* && ! -e "$results" && ! -e .git && ! -e .jj ]] || exit 2
mkdir -p "$results"
set +e
cargo check -p zencodecs --no-default-features \
    --features all,jp2-decode,metadata,checked-transcode-async --locked \
    --message-format=json > "$results/compiler.jsonl" 2> "$results/compiler.stderr.log"
rc=$?
set -e
printf '%s\n' "$rc" > "$results/exit-code.txt"
python3 - "$results/compiler.jsonl" > "$results/diagnostics.log" <<'PY'
from pathlib import Path
import json,sys
for line in Path(sys.argv[1]).read_text().splitlines():
    message = json.loads(line)
    if message.get('reason') == 'compiler-message':
        print(message['message'].get('rendered') or message['message']['message'])
PY
cat "$results/compiler.stderr.log" "$results/diagnostics.log"
exit "$rc"
