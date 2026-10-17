#!/usr/bin/env bash
# site-check — build the site, serve it the way Cloudflare does, and assert it renders.
#
# `astro build` proves the files exist; it never opens one. The demos shipped as empty boxes because
# the CSP blocked the player's WebAssembly, and no step in the pipeline was looking at a rendered
# page. This closes that gap.
#
# It serves through `wrangler pages dev` rather than `astro preview` on purpose: `_headers` — which
# is where the CSP lives — is applied by Pages and by nothing else, so a preview server would test
# a configuration that never reaches production.
set -euo pipefail

cd "$(dirname "$0")/.."
port="${PORT:-8788}"
serve_log="${TMPDIR:-/tmp}/site-check-serve.log"
skip_build=0
[[ "${1:-}" == "--no-build" ]] && skip_build=1

command -v "${CHROME:-google-chrome-stable}" >/dev/null 2>&1 || {
	echo "site-check: no headless Chrome (set CHROME=...), skipping" >&2
	exit 0
}

# Is anything listening there? Bash's own TCP redirection, because `ss` is Linux-only and `lsof` is
# not guaranteed anywhere.
port_is_open() {
	(exec 3<>"/dev/tcp/127.0.0.1/$1") 2>/dev/null
}

# Stop the server and everything it spawned.
#
# Signals the process *group*: wrangler spawns workerd and esbuild, and killing only the parent
# leaves the port held. Idempotent, because the interrupt traps below run it and then exit, which
# runs it again.
stop_server() {
	[[ -n "${serve_pgid:-}" ]] || return 0
	kill -TERM -- -"${serve_pgid}" 2>/dev/null || true
	for _ in $(seq 1 30); do
		kill -0 -- -"${serve_pgid}" 2>/dev/null || return 0
		sleep 0.1
	done
	# workerd holds the port until it is gone, and the next run cannot tell a stale server from a
	# fresh one — so a group that ignored the polite signal does not get to stay.
	kill -KILL -- -"${serve_pgid}" 2>/dev/null || true
}

# The readiness probe only asks for `/`, and a stale workerd answers that with a 200 from whatever
# `dist` it was started on. A run that found the port held would therefore validate a build nobody
# made — quietly, and with a green check. Refusing here is also what lets the probe below trust the
# answer it gets: the port was free when this run started, and this run is what bound it.
if port_is_open "${port}"; then
	echo "site-check: 127.0.0.1:${port} is already in use — stop that server, or set PORT=…" >&2
	exit 1
fi

if ((skip_build == 0)); then
	(cd web && npm run build)
fi

# `set -m` gives the background job a process group of its own, with the pgid `$!` reports. Without
# it a script's background job shares the shell's group, so `kill -- -$!` names a group that does
# not exist, the fallback kills the one process not holding anything, and workerd survives on the
# port. That is not only an interrupt path: it leaked on every clean run too.
set -m
(cd web && exec npx wrangler pages dev dist --port "${port}" --ip 127.0.0.1) >"${serve_log}" 2>&1 &
serve_pgid=$!
set +m

# EXIT alone is not enough: bash runs no EXIT trap when an *untrapped* signal kills it, so
# `timeout -s TERM` — which is how this runs unattended — skipped the cleanup entirely. Each of
# these exits with the shell's usual 128 + signal.
trap stop_server EXIT
trap 'stop_server; exit 130' INT
trap 'stop_server; exit 143' TERM
trap 'stop_server; exit 129' HUP

# A server that died is an answer, and without asking we spend the whole window discovering it.
gave_up=1
for _ in $(seq 1 60); do
	if ! kill -0 -- -"${serve_pgid}" 2>/dev/null; then
		echo "site-check: the dev server exited before it was ready" >&2
		tail -20 "${serve_log}" >&2
		exit 1
	fi
	if curl -fsS -o /dev/null "http://127.0.0.1:${port}/" 2>/dev/null; then
		gave_up=0
		break
	fi
	sleep 0.5
done
if ((gave_up == 1)); then
	echo "site-check: the dev server never answered on 127.0.0.1:${port}" >&2
	tail -20 "${serve_log}" >&2
	exit 1
fi

node scripts/site-smoke.mjs "http://127.0.0.1:${port}"
