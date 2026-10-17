#!/usr/bin/env bash
# Build and run the whole suite for Windows, from Linux, in about twenty seconds.
#
# CI's `windows-latest` job is the authority, but it is a thirteen-minute round trip and it lies by
# omission: GitHub's image puts Git-for-Windows' `usr/bin` on `PATH`, so `sh`, `bash` and `grep` are
# all present there and none of the "there is no POSIX shell" failures ever surface. A default Git
# for Windows install puts only `Git\cmd` on `PATH` — `git.exe` and nothing else.
#
# So this runs the *stricter* environment: cross-compile with clang-cl (`cargo-xwin` fetches the MSVC
# CRT and Windows SDK headers), execute under wine, and hand it MinGit and nothing more. What passes
# here passes on a real Windows box; what CI reports green may still be broken there.
#
# Wine is an approximation, not Windows. It gets `FlushFileBuffers`, path semantics, `PATH` order and
# process spawning right, which is where our bugs live; it does not get ACLs, antivirus, or console
# rendering right. Treat a green run here as "worth pushing", never as "shipped".
#
#   Requires: clang-cl + lld-link + llvm-lib (Arch: clang lld), wine, cargo-xwin
#             (`cargo install cargo-xwin`), cargo-nextest.
#
# Usage:
#   scripts/win-check.sh [options] [-- <extra nextest args>]
#
# Options:
#   --no-git         don't set up MinGit (the git-backed tests will fail; useful for isolating)
#   --setup          fetch MinGit and prime the wine prefix, then exit
#   --clippy         run clippy for the Windows target instead of the tests
#   --build          build only
#   -h, --help       this text
#
# Examples:
#   scripts/win-check.sh
#   scripts/win-check.sh -- -p luxe-tools -E 'test(glob)'
set -euo pipefail

TARGET=x86_64-pc-windows-msvc
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# 19 GB of Windows objects: keep them on real disk, not on a tmpfs /tmp.
WIN_TARGET_DIR="$ROOT/target/win"
MINGIT_DIR="${LUXE_MINGIT_DIR:-$ROOT/target/win-mingit}"
MINGIT_VERSION="${LUXE_MINGIT_VERSION:-2.51.0}"
MINGIT_URL="https://github.com/git-for-windows/git/releases/download/v${MINGIT_VERSION}.windows.1/MinGit-${MINGIT_VERSION}-64-bit.zip"

mode=test
want_git=1
extra=()
while [[ $# -gt 0 ]]; do
	case "$1" in
	--no-git)
		want_git=0
		shift
		;;
	--setup)
		mode=setup
		shift
		;;
	--clippy)
		mode=clippy
		shift
		;;
	--build)
		mode=build
		shift
		;;
	-h | --help)
		# The header comment IS the help text, so read it rather than hardcoding a line range —
		# which silently truncated the moment a paragraph was added above it.
		awk 'NR > 1 && /^#/ { sub(/^# ?/, ""); print; next } NR > 1 { exit }' "${BASH_SOURCE[0]}"
		exit 0
		;;
	--)
		shift
		extra=("$@")
		break
		;;
	*)
		echo "unknown option: $1 (see --help)" >&2
		exit 2
		;;
	esac
done

have() { command -v "$1" >/dev/null 2>&1; }

missing=()
for tool in clang-cl lld-link llvm-lib wine cargo; do
	have "$tool" || missing+=("$tool")
done
have cargo-xwin || cargo xwin --version >/dev/null 2>&1 || missing+=("cargo-xwin (cargo install cargo-xwin)")
if [[ ${#missing[@]} -gt 0 ]]; then
	printf 'missing: %s\n' "${missing[*]}" >&2
	exit 1
fi

# The Windows standard library, which is the prerequisite nothing else checks for. Without it the
# build fails hundreds of crates deep with `can't find crate for core`, whose own suggestion is to
# run a command that a distribution's Rust package does not have. Say the useful thing instead.
if have rustup; then
	if ! rustup target list --installed 2>/dev/null | grep -qx "$TARGET"; then
		cat >&2 <<-EOF
			missing: the $TARGET standard library

			  rustup target add $TARGET

			If mise manages your Rust, declare it instead so a toolchain update keeps it:
			  rust = { version = "stable", targets = ["$TARGET"] }
		EOF
		exit 1
	fi
else
	# No rustup at all: a distribution package ships one target and cannot add another, so the
	# build below is going to fail and the reason will not be obvious from its output.
	cat >&2 <<-EOF
		missing: rustup, which is the only way to install the $TARGET standard library

		A distribution's \`rust\` package ships one target and no way to add another.
		  mise use -g rust@stable      # mise manages rustup underneath
		  # or: https://rustup.rs
	EOF
	exit 1
fi

# MinGit, unzipped. Only `cmd/` goes on the wine PATH: that is what the Git for Windows installer
# adds by default, and the whole point is to *not* hand ourselves the unix tools a real user lacks.
setup_mingit() {
	# `-f`, not `-x`: the archive carries no unix modes, so nothing unpacks executable.
	if [[ -f "$MINGIT_DIR/cmd/git.exe" ]]; then
		return
	fi
	echo "fetching MinGit $MINGIT_VERSION -> $MINGIT_DIR"
	mkdir -p "$MINGIT_DIR"
	local zip="$MINGIT_DIR/../mingit.zip"
	curl -fsSL -o "$zip" "$MINGIT_URL"
	(cd "$MINGIT_DIR" && unzip -q -o "$zip")
	rm -f "$zip"
	[[ -f "$MINGIT_DIR/cmd/git.exe" ]] || {
		echo "MinGit unpacked but git.exe is missing" >&2
		exit 1
	}
}

if [[ $want_git -eq 1 ]]; then
	setup_mingit
fi

cd "$ROOT"
# `cargo xwin env` exports the clang-cl/lld-link toolchain *and* sets the target runner to wine, so
# `cargo nextest run --target …` just works from here.
eval "$(cargo xwin env --target "$TARGET")"
export CARGO_TARGET_DIR="$WIN_TARGET_DIR"
export WINEDEBUG="${WINEDEBUG:--all}"
export WINEPREFIX="${WINEPREFIX:-$HOME/.wine-luxe}"
# NOTE: wine initializes a display driver even for console programs, and on a GPU whose mesa build
# can't answer its EGL probe it prints a handful of `libEGL warning:` lines into the captured stderr
# of every test. Cosmetic — ignore them; they are wine's, not ours.
if [[ $want_git -eq 1 ]]; then
	export WINEPATH="$MINGIT_DIR/cmd"
fi

case "$mode" in
setup)
	# A cold prefix prints its first-run noise into whatever runs next; get it over with.
	wine cmd /c ver >/dev/null 2>&1 || true
	echo "wine prefix: $WINEPREFIX"
	echo "mingit:      ${WINEPATH:-<none>}"
	;;
build) cargo build --workspace --all-targets --target "$TARGET" "${extra[@]}" ;;
clippy) cargo clippy --workspace --all-targets --target "$TARGET" "${extra[@]}" -- -D warnings ;;
test) cargo nextest run --workspace --target "$TARGET" --no-fail-fast "${extra[@]}" ;;
esac
