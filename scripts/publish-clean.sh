#!/usr/bin/env bash
# Materialize a clean, publishable copy of luxe — without this repo's development
# artifacts (PROJECT.md, PROGRESS.md, docs/plan/, .pi/, …) or its git history.
#
# The exclusions are declared as `export-ignore` in .gitattributes, so `git archive`
# does the filtering for us. The result is a fresh tree suitable for `git init` in a
# new public repository. luxe never requires the excluded files at runtime.
#
# Usage:
#   scripts/publish-clean.sh <target-dir> [ref]
#
#   <target-dir>   Empty/nonexistent directory to populate (required).
#   [ref]          Git ref to export (default: HEAD).
#
# Example:
#   scripts/publish-clean.sh /tmp/luxe-public
#   cd /tmp/luxe-public && git init && git add -A && git commit -m "Initial public release"

set -euo pipefail

target="${1:-}"
ref="${2:-HEAD}"

if [[ -z "$target" ]]; then
	echo "usage: $0 <target-dir> [ref]" >&2
	exit 2
fi

if [[ -e "$target" && -n "$(ls -A "$target" 2>/dev/null)" ]]; then
	echo "error: target '$target' exists and is not empty" >&2
	exit 1
fi

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

mkdir -p "$target"
# --worktree-attributes honors the working-tree .gitattributes export-ignore rules,
# so the exclusions apply whether or not .gitattributes has been committed yet.
git archive --worktree-attributes "$ref" | tar -x -C "$target"

# `git archive` still emits directory entries for fully-excluded trees; drop them so the
# published tree has no empty `docs/plan/` stub (git couldn't track it anyway).
find "$target" -mindepth 1 -type d -empty -delete

# Verify rather than trust: gitattributes patterns are easy to get subtly wrong (a trailing
# slash matches nothing, so `docs/plan/` silently exports the whole directory). Publishing the
# planning spine or `.pi/memory/` to a public repo is not a recoverable mistake.
leaked=()
for path in PROJECT.md PROGRESS.md PLAN.md ROADMAP.md docs/plan docs/eval .pi \
	luxe-review-findings.md shine-text.md scripts/tool-ergonomics.sh; do
	if [[ -f "$target/$path" ]] ||
		[[ -d "$target/$path" && -n "$(find "$target/$path" -type f -print -quit)" ]]; then
		leaked+=("$path")
	fi
done
leaked+=($(cd "$target" && find . -path './docs/HANDOFF-*' -print))
if ((${#leaked[@]})); then
	echo "error: development artifacts leaked into '$target': ${leaked[*]}" >&2
	echo "       check the export-ignore patterns in .gitattributes (use 'dir/**', not 'dir/')" >&2
	exit 1
fi

# Then read what did ship. Pointers into the excluded tree dangle in public, a home path or a
# private key says something about this machine, and the denylist holds the names that must never
# leave it. The denylist lives in .git/info, outside the tree, because a file listing private names
# would publish them; one PCRE per line, `#` for comments.
denylist="${PUBLISH_DENYLIST:-$repo_root/.git/info/publish-denylist}"
# A citation names a file that did not ship; a bare `docs/plan/` in backticks points at the
# directory. Either dangles. A generic path in an example (`docs/plan/*.md`) is fine.
cited=$(git ls-files -z docs/plan docs/eval | xargs -0 -n1 | sed 's/[.]/\\./g' | paste -sd'|')
patterns=(
	"(${cited})"
	'`docs/(plan|eval)/`'
	"$(printf '%s' "$HOME" | sed 's/[][\\.*^$+?(){}|/]/\\&/g')(/|\b)"
	'^-----BEGIN [A-Z ]*PRIVATE KEY-----$'
	'sk-ant-api\d\d-[A-Za-z0-9_-]{80,}'
	'sk-(proj|svcacct)-[A-Za-z0-9_-]{80,}'
	'\bghp_[A-Za-z0-9]{36}\b'
	'github_pat_[A-Za-z0-9_]{80,}'
)
if [[ -f "$denylist" ]]; then
	while IFS= read -r line; do
		[[ -z "$line" || "$line" == \#* ]] || patterns+=("$line")
	done <"$denylist"
else
	echo "warning: no denylist at $denylist; scanning for the built-in patterns only" >&2
fi
hits=$(cd "$target" && for pattern in "${patterns[@]}"; do
	grep -rIPn --exclude-dir=node_modules --exclude=publish-clean.sh --exclude=.gitattributes --exclude=.gitignore -e "$pattern" . || true
done | sort -u)
if [[ -n "$hits" ]]; then
	echo "error: the exported tree mentions something it must not ($(wc -l <<<"$hits") lines):" >&2
	echo "$hits" | cut -c1-200 >&2
	exit 1
fi

echo "Clean tree written to: $target"
echo "Excluded: PROJECT.md PROGRESS.md PLAN.md ROADMAP.md docs/plan/ docs/eval/ docs/HANDOFF-*.md .pi/"
echo
echo "Next: cd $target && git init && git add -A && git commit -m 'Initial public release'"
