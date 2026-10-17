#!/usr/bin/env bash
# Refresh the `luxe` you dogfood with.
#
# Development and the test gate run the *debug* profile, but the `luxe` on PATH is a symlink to
# `target/release/luxe` — so a fix can be green in the workspace and still absent from the binary
# you actually launch. That gap is silent and costs a debugging session every time. This builds
# release and points the symlink at it.
#
# Usage:
#   scripts/install-dev.sh [--gate] [bin-dir]
#
#   --gate      Run fmt/clippy/tests first and refuse to install if any fail.
#   [bin-dir]   Where to put the symlink (default: ~/.local/bin).

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
run_gate=false
bin_dir="${HOME}/.local/bin"

for arg in "$@"; do
	case "$arg" in
	--gate) run_gate=true ;;
	-h | --help)
		sed -n '2,14p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
		exit 0
		;;
	-*)
		echo "unknown flag: $arg" >&2
		exit 2
		;;
	*) bin_dir="$arg" ;;
	esac
done

cd "$repo_root"

if [[ "$run_gate" == true ]]; then
	echo "==> gate"
	cargo fmt --all --check
	cargo clippy --workspace --all-targets -- -D warnings
	# Ask cargo, not PATH: the nextest binary lives in ~/.cargo/bin, which a shell that reaches
	# cargo through a rustup shim in /usr/bin doesn't necessarily have.
	if cargo nextest --version >/dev/null 2>&1; then
		cargo nextest run --workspace
	else
		# `cargo test` runs the suite as threads in ONE process, and a handful of tests set `HOME`
		# to a temp dir to isolate global config. Those mutations are process-wide, so they can
		# break whichever test happens to run beside them (the git-commit tests lose the global
		# identity and fail). Install nextest for a trustworthy gate: cargo install cargo-nextest.
		echo "warning: cargo-nextest not found; falling back to cargo test (shared process," >&2
		echo "         env-mutating tests can cross-talk — install cargo-nextest for a clean run)" >&2
		cargo test --workspace
	fi
fi

echo "==> building release"
cargo build --release -p luxe-cli

binary="${repo_root}/target/release/luxe"
[[ -x "$binary" ]] || {
	echo "expected a binary at ${binary}" >&2
	exit 1
}

mkdir -p "$bin_dir"
link="${bin_dir}/luxe"
# Replace whatever is there (stale symlink, or a copy from an older install) rather than failing.
ln -sfn "$binary" "$link"

echo "==> installed"
printf '    %s -> %s\n' "$link" "$(readlink -f "$link")"
printf '    built  %s\n' "$(date -r "$binary" '+%Y-%m-%d %H:%M:%S')"
printf '    commit %s\n' "$(git -C "$repo_root" log -1 --format='%h %s' 2>/dev/null || echo 'unknown')"
# "Dirty" means the binary contains something no commit does. Tracked edits always qualify;
# untracked files only when they are source, so stray screenshots and scratch notes stay quiet.
tracked_changes="$(git -C "$repo_root" status --porcelain --untracked-files=no 2>/dev/null)"
untracked_source="$(git -C "$repo_root" ls-files --others --exclude-standard -- 'crates/*' 2>/dev/null)"
if [[ -n "$tracked_changes" || -n "$untracked_source" ]]; then
	echo '    note: built from a dirty tree (uncommitted changes are included)'
fi

case ":${PATH}:" in
*":${bin_dir}:"*) ;;
*) echo "    warning: ${bin_dir} is not on your PATH" >&2 ;;
esac
