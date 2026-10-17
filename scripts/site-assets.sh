#!/usr/bin/env bash
# Regenerate every asset the website derives from this repo.
#
# Nothing under the output directory is authored by hand: the logotype comes from the art constants,
# the color tokens from the theme JSON, and the screenshots from the production view path via the
# fixture runner. That is the point — a screenshot taken by hand is a claim about a build nobody can
# reproduce, and it starts lying the first time a border changes.
#
# Run it after anything that changes how Luxe looks, and commit the result. CI runs it with
# `--check`, which regenerates into a scratch directory and fails if the committed assets differ.
#
# The *demos* are not here and cannot be. They are recordings of real sessions against a real
# provider (`scripts/site-record.sh`), so no two runs produce the same bytes and nothing CI does
# can verify one. They live beside these, under `web/src/assets/captures/`, with a manifest saying
# what each was shot against. Everything in this directory is derived and checked; everything in
# that one is a take.
#
# Usage:
#   scripts/site-assets.sh [--out DIR] [--check]
#
#   --out DIR   where to write (default: web/src/assets/generated)
#   --check     regenerate to a temp dir and diff against --out; write nothing
#
# The guide's screenshots are generated the same way, into docs/guide/images/, and checked with the
# rest.

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
out_dir="${repo_root}/web/src/assets/generated"
check=false

while [[ $# -gt 0 ]]; do
	case "$1" in
	--out)
		out_dir="$2"
		shift 2
		;;
	--check)
		check=true
		shift
		;;
	-h | --help)
		sed -n '2,17p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
		exit 0
		;;
	*)
		echo "unknown argument: $1" >&2
		exit 2
		;;
	esac
done

# The scenarios the site shows, and the terminal each is drawn at. Guided Review earns a wider box
# because it is a three-pane surface and a narrow capture sells it as a diff viewer. The dashboard
# is first because it is the first thing anyone sees, and it went uncaptured for far too long.
#
# The GitHub review surfaces are the landing page's, each twice: at the width a laptop shows and at
# the width a phone can read, which lays the same review out as one pane. So is peek, once wide
# enough to sit beside the conversation and once narrow enough to take its place, at the same
# aspect (172x50 against 110x32) so switching between the two does not jolt the page. The optional
# fourth field names the file when one scenario is drawn at two sizes, and the fifth opens a state
# over it, by the names `fixture_svg --overlay` takes (`/github` and `/pr` live there).
#
#   <fixture scenario>:<cols>:<rows>[:<name>[:<overlay>]]
SHOTS=(
	"dashboard:150:36"
	"chat-tools:150:40"
	"chat-streaming:150:36"
	"chat-thinking:150:34"
	"chat-question:150:34"
	"guided-review-demo:180:44"
	"changes-wide:240:40"
	"dock-wide:170:42"
	"session-picker:150:36"
	"guided-review-pull-request:150:42"
	"guided-review-pull-request:72:30:guided-review-pull-request.narrow"
	"guided-review-pull-request-post:150:42"
	"guided-review-pull-request-post:72:30:guided-review-pull-request-post.narrow"
	"changes-wide:172:50:peek-beside"
	"changes-narrow:110:32:peek-over"
	"chat-idle:140:34:github-overview:github"
	"chat-idle:72:30:github-overview.narrow:github-detail"
	"chat-idle:140:34:github-conversation:github-conversation"
	"chat-idle:72:30:github-conversation.narrow:github-conversation"
)

# The guide's screenshots, drawn narrower than the site's: the docs column is about 830px, and a
# 100-column capture keeps the type readable in it. Same fields; the name is required.
GUIDE_SHOTS=(
	"dashboard:100:26:quickstart-dashboard"
	"setup:110:34:quickstart-setup"
	"dashboard-typing:100:28:interface-launcher-typing"
	"dashboard-minimal:100:20:interface-launcher-minimal"
	"settings-tools:120:36:configuration-settings"
	"configuration:100:26:quickstart-configuration"
	"chat-tool-run:100:22:quickstart-turn"
	"guided-review-demo:110:34:quickstart-review"
	"guided-review-open-pull-request:100:30:quickstart-pull-request"
	"changes-wide:140:36:interface-peek"
	"guided-review-demo:140:40:review-guide"
	"guided-review-finish:110:26:review-finish"
	"guided-review-pull-request:140:40:review-pull-request"
	"chat-idle:140:34:github-overview:github"
	"chat-idle:140:34:github-search:github-search"
	"chat-idle:140:34:github-conversation:github-conversation"
	"chat-question:100:22:agent-question"
	"chat-turn:100:36:interface-turn"
	"chat-permission:100:24:security-permission-prompt"
	"chat-palette:100:30:commands-palette"
	"chat-idle:100:22:mentions-picker:mention-files-status"
	"session-picker:100:28:sessions-picker"
	"chat-plan-ready:100:26:loop-plan-ready"
)
guide_dir="${repo_root}/docs/guide/images"

# Luxe's own theme, the one the site is set in. The colors are baked into each file rather than left
# as custom properties, so a capture stays correct wherever it is embedded — a README, someone's
# blog. The theme preview is the one picture drawn in every theme, below.
THEMES=("luxe")

# The figures the landing page quotes, derived rather than typed.
#
# Only the ones that change with the thing they describe: the theme count moves when a theme ships,
# and the catalog moves when the models.dev snapshot is refreshed, which regenerates this in the
# same pull request. A test total used to live here too, and it made every pull request that added a
# test fail this check until someone regenerated a number the page barely needed.
facts() {
	python3 -c "import gzip,json,sys
catalog = json.load(gzip.open('${repo_root}/crates/luxe-providers/data/models-dev-snapshot.json.gz'))
print(json.dumps({
	'themes': len(json.load(open('${1}/themes.json'))),
	'providers': len(catalog),
	'models': sum(len(p.get('models', {})) for p in catalog.values()),
}, indent=2))"
}

fixture() { cargo run --quiet --release -p luxe-tui --features fixture --example fixture_svg -- "$@"; }
wordmark() { cargo run --quiet --release -p luxe-tui --example wordmark_svg -- "$@"; }
tokens() { cargo run --quiet --release -p luxe-tui --example theme_css; }
catalog() { cargo run --quiet --release -p luxe-tui --example theme_css -- --json; }
shiki() { cargo run --quiet --release -p luxe-tui --example shiki_theme -- "$@"; }

# The README's wordmark. GitHub shows an image on its own, where the CSS variables the site paints
# the letters with do not exist and every letter would fall back to black, so this copy carries
# the theme's own colors and its background: the same on GitHub's light pages as on its dark ones.
banner() {
	python3 - "$1/wordmark.svg" "$1/tokens.css" <<'PY'
import re, sys
svg, css = (open(path).read() for path in sys.argv[1:])
color = lambda name: re.search(rf"--luxe-{name}:\s*(#[0-9a-f]{{6}})", css).group(1)
svg = re.sub(r"var\(--luxe-(wordmark-\d), currentColor\)", lambda m: color(m.group(1)), svg)
width, height = (int(n) for n in re.search(r'viewBox="0 0 (\d+) (\d+)"', svg).groups())
pad_x, pad_y = 120, 72
box = (width + 2 * pad_x, height + 2 * pad_y)
svg = re.sub(
    r'viewBox="[^"]*" width="\d+" height="\d+"',
    f'viewBox="{-pad_x} {-pad_y} {box[0]} {box[1]}" width="{box[0]}" height="{box[1]}"',
    svg,
)
background = f'<rect x="{-pad_x}" y="{-pad_y}" width="{box[0]}" height="{box[1]}" rx="16" fill="{color("background")}"/>'
print(svg.replace("</title>", "</title>\n  " + background, 1), end="")
PY
}

generate() {
	local target="$1" guide_target="$2"
	mkdir -p "${target}" "${guide_target}"

	wordmark wordmark >"${target}/wordmark.svg"
	wordmark mark >"${target}/mark.svg"
	wordmark tile >"${target}/tile.svg"
	tokens >"${target}/tokens.css"
	banner "${target}" >"${target}/wordmark-banner.svg"
	catalog >"${target}/themes.json"
	facts "${target}" >"${target}/facts.json"
	shiki luxe >"${target}/shiki-luxe.json"

	local shot theme scenario cols rows name
	for shot in "${SHOTS[@]}"; do
		IFS=: read -r scenario cols rows name overlay <<<"${shot}"
		for theme in "${THEMES[@]}"; do
			fixture --theme "${theme}" --scenario "${scenario}" \
				--cols "${cols}" --rows "${rows}" ${overlay:+--overlay "${overlay}"} \
				>"${target}/${name:-${scenario}}.${theme}.svg"
		done
	done

	# The theme preview: the dashboard, once, in every built-in theme. Each color in it is a CSS
	# variable and `theme-preview.json` says what each one is per theme, so the page can repaint the
	# real UI as a reader points at a theme instead of shipping twelve copies of one picture. The
	# shipped theme leads, so its colors are the fallbacks when no palette has been applied.
	local palette
	palette="$(python3 -c "
import json, sys
names = sorted(json.load(open(sys.argv[1])))
names.remove('luxe')
print(','.join(['luxe'] + names))
" "${target}/themes.json")"
	fixture --scenario dashboard --cols 150 --rows 36 --palette "${palette}" \
		--palette-json "${target}/theme-preview.json" >"${target}/theme-preview.svg"

	for shot in "${GUIDE_SHOTS[@]}"; do
		IFS=: read -r scenario cols rows name overlay <<<"${shot}"
		fixture --theme luxe --scenario "${scenario}" --cols "${cols}" --rows "${rows}" \
			${overlay:+--overlay "${overlay}"} >"${guide_target}/${name}.svg"
	done

}

# One release build up front. Every generator is a separate `cargo run`, and letting each discover
# that for itself turns a five-second job into a minute of cargo deciding it has nothing to do.
cargo build --quiet --release -p luxe-tui --features fixture \
	--example fixture_svg --example wordmark_svg --example theme_css --example shiki_theme

if [[ "${check}" == true ]]; then
	scratch="$(mktemp -d)"
	trap 'rm -rf "${scratch}"' EXIT
	generate "${scratch}/site" "${scratch}/guide"
	if diff -ru "${out_dir}" "${scratch}/site" && diff -ru "${guide_dir}" "${scratch}/guide"; then
		echo "site assets are up to date"
	else
		echo >&2
		echo "site assets are stale — run scripts/site-assets.sh and commit the result" >&2
		exit 1
	fi
	exit 0
fi

generate "${out_dir}" "${guide_dir}"

printf '%s\n' "wrote $(find "${out_dir}" -type f | wc -l) files to ${out_dir}"
du -sh "${out_dir}" | cut -f1 | xargs -I{} echo "total {} on disk, and they gzip to about a tenth of that"
