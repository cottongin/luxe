#!/usr/bin/env bash
# Render the Open Graph card to web/public/social.png.
#
# The card is a page (`web/src/pages/social-card.astro`), not a design file, so it is built from the
# same tokens, fonts and logotype as the site and cannot drift from what it advertises. This just
# photographs it at the exact size the platforms want.
#
# Separate from `site-assets.sh` on purpose: that one turns Rust into assets and CI checks it, and
# it should not need Node, a build, and a browser to do so.
#
# Usage: scripts/social-card.sh

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
web_dir="${repo_root}/web"
out="${web_dir}/public/social.png"
port="${PORT:-4331}"

chrome=""
for candidate in chromium google-chrome-stable google-chrome chrome; do
	command -v "${candidate}" >/dev/null && chrome="${candidate}" && break
done
[[ -n "${chrome}" ]] || {
	echo "no chromium/chrome on PATH" >&2
	exit 1
}

cd "${web_dir}"
npm run build >/dev/null

npx astro preview --port "${port}" >/dev/null 2>&1 </dev/null &
preview=$!
trap 'kill "${preview}" 2>/dev/null || true' EXIT

for _ in $(seq 1 30); do
	sleep 1
	if curl -sf -o /dev/null "http://localhost:${port}/social-card"; then
		break
	fi
done
curl -sf -o /dev/null "http://localhost:${port}/social-card" || {
	echo "preview never came up on :${port}" >&2
	exit 1
}

# `--virtual-time-budget` waits for the webfonts and the wordmark rather than shooting a blank frame.
"${chrome}" --headless --disable-gpu --hide-scrollbars \
	--force-device-scale-factor=1 --window-size=1200,630 \
	--virtual-time-budget=5000 \
	--screenshot="${out}" "http://localhost:${port}/social-card" 2>/dev/null

printf 'wrote %s (%s bytes)\n' "${out}" "$(stat -c%s "${out}")"
