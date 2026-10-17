#!/usr/bin/env bash
# Measure each tool against the shell command it replaces, for the site's Tools page.
#
#   scripts/tool-bench.sh NAME=REPO:CASES ...
#
# Each argument is a checkout to measure in and the cases to run there (scripts/tool-bench/);
# Luxe's own checkout is always measured last, with the cases built into the example. The first
# repository is the one the page draws its bars from.
#
# A recording, not a generated asset: the numbers depend on the trees they run in, build output
# and virtualenvs included, so nothing checks them. Run it after a build, and commit the JSON,
# which names each repository's commit. Needs network for the web_fetch row.
set -euo pipefail
cd "$(dirname "$0")/.."
root=$PWD
cargo build -q --release -p luxe-cli --example tool_bench
bench="$root/target/release/examples/tool_bench"
out=$(mktemp -d)
names=()
for spec in "$@"; do
	name=${spec%%=*}
	rest=${spec#*=}
	repo=${rest%%:*}
	cases=${rest#*:}
	(cd "$repo" && TOOL_BENCH_CASES="$root/$cases" "$bench" "$out/$name.json")
	names+=("$name")
done
"$bench" "$out/Luxe.json"
names+=("Luxe")
python3 - "$out" "${names[@]}" <<'PY' > web/src/data/tool-bench.json
import json, sys
out, names = sys.argv[1], sys.argv[2:]
repos = []
for name in names:
    data = json.load(open(f"{out}/{name}.json"))
    repos.append({"name": name, "commit": data["commit"], "rows": data["rows"]})
print(json.dumps({"repositories": repos}, indent=2))
PY
rm -rf "$out"
