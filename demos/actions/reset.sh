#!/usr/bin/env bash
# Reset the demo fixture and re-date its clock to "now".
#
# The producer stamps real wall-clock timestamps into everything it
# writes; grafting those onto a fixture dated months ago makes every
# age/ETA the pane and `dagr stats` report nonsense.
# This restores the pristine fixture (when in a git checkout) and then
# shifts EVERY timestamp by the same delta that puts `generated_at` at
# the current minute — offsets between timestamps are preserved.
#
#   demos/actions/reset.sh    # then run the demo per README
set -euo pipefail
cd "$(dirname "$0")"

git checkout -- run.json 2>/dev/null || true

python3 - <<'PY'
import json, re
from datetime import datetime, timezone

FMT = "%Y-%m-%dT%H:%M:%SZ"
with open("run.json") as fh:
    raw = fh.read()
anchor = datetime.strptime(json.loads(raw)["generated_at"], FMT).replace(tzinfo=timezone.utc)
delta = datetime.now(timezone.utc).replace(second=0, microsecond=0) - anchor

def shift(m):
    t = datetime.strptime(m.group(0), FMT).replace(tzinfo=timezone.utc)
    return (t + delta).strftime(FMT)

out = re.sub(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z", shift, raw)
with open("run.json", "w") as fh:
    fh.write(out)
print(f"run.json re-dated: generated_at -> {json.loads(out)['generated_at']}")
PY
