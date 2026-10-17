#!/usr/bin/env bash
# Regenerate the embedded models.dev snapshot.
#
# Luxe resolves model metadata from four layers (defaults -> snapshot -> cache -> config). The
# snapshot is the offline/first-run floor: it's what a fresh install with no network knows. The
# runtime refresh keeps a live install current, so this only needs to run at release cadence —
# CI does it weekly and opens a PR when the bytes change.
#
# The projection keeps only the fields `luxe_providers::catalog::schema` reads, then gzips, which
# takes the 3.2 MB API response down to ~150 KB in the binary.
#
# Usage:
#   scripts/update-models-snapshot.sh [--url URL]

set -euo pipefail

URL="${MODELS_DEV_URL:-https://models.dev/api.json}"
while [[ $# -gt 0 ]]; do
	case "$1" in
	--url)
		URL="$2"
		shift 2
		;;
	*)
		echo "unknown argument: $1" >&2
		exit 2
		;;
	esac
done

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="$ROOT/crates/luxe-providers/data/models-dev-snapshot.json.gz"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

echo "fetching $URL"
curl --fail --silent --show-error --location --max-time 60 --compressed "$URL" -o "$TMP/api.json"

python3 - "$TMP/api.json" "$TMP/snapshot.json" <<'PY'
import json, sys

MODEL_FIELDS = (
    "name", "reasoning", "tool_call", "release_date", "status",
    "limit", "cost", "modalities", "reasoning_options",
)
COST_FIELDS = ("input", "output", "cache_read", "cache_write")

def clean(value):
    """Drop nulls and empties so the snapshot matches what the schema re-serializes.

    Upstream does ship the occasional null inside a string array (sarvam-*), which the strict
    Rust schema would reject; stripping here keeps the rest of that entry usable.
    """
    if isinstance(value, dict):
        out = {k: clean(v) for k, v in value.items() if v is not None}
        return {k: v for k, v in out.items() if v not in ({}, [], None)}
    if isinstance(value, list):
        return [clean(v) for v in value if v is not None]
    return value

def project_model(model):
    out = {}
    for field in MODEL_FIELDS:
        value = model.get(field)
        if value in (None, {}, []):
            continue
        if field == "cost":
            value = {k: value[k] for k in COST_FIELDS if value.get(k) is not None}
        elif field == "modalities":
            # Only input modalities are read; output is always ["text"] for chat models.
            value = {k: value[k] for k in ("input",) if value.get(k)}
        elif field == "reasoning_options":
            value = [clean(option) for option in value]
        out[field] = clean(value)
    return {k: v for k, v in out.items() if v not in ({}, [], None)}

source = json.load(open(sys.argv[1]))
snapshot = {}
for provider_id, provider in sorted(source.items()):
    models = {
        model_id: project_model(model)
        for model_id, model in sorted(provider.get("models", {}).items())
    }
    if not models:
        continue
    block = {"models": models}
    if provider.get("name"):
        block["name"] = provider["name"]
    snapshot[provider_id] = block

json.dump(snapshot, open(sys.argv[2], "w"), separators=(",", ":"), sort_keys=True)
print(
    f"projected {len(snapshot)} providers / "
    f"{sum(len(p['models']) for p in snapshot.values())} models",
    file=sys.stderr,
)
PY

# -n so the gzip header carries no name/mtime: identical input yields identical bytes, which
# keeps the weekly CI job from opening a PR that changes nothing.
gzip -9 -n -c "$TMP/snapshot.json" >"$TMP/snapshot.json.gz"

mkdir -p "$(dirname "$OUT")"
if [[ -f "$OUT" ]] && cmp -s "$TMP/snapshot.json.gz" "$OUT"; then
	echo "snapshot unchanged ($(wc -c <"$OUT") bytes)"
	exit 0
fi
mv "$TMP/snapshot.json.gz" "$OUT"
echo "wrote $OUT ($(wc -c <"$OUT") bytes)"
