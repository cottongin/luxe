#!/usr/bin/env bash
# Drive the TUI through a scripted sequence and capture screenshots along the way.
#
# The agent-loop `screenshot` tool launches a command, waits, types every key at once and shoots
# immediately — which is useless for anything with latency (a review capture, a provider round-trip)
# or for stepping a surface. This drives a real terminal with explicit pauses between steps, so what
# lands in the image is the state you meant to look at.
#
# Usage:
#   scripts/tui-drive.sh [options] -- <script-file>
#   scripts/tui-drive.sh [options] -s 'step; step; …'
#
# Options:
#   --cols N          terminal columns (default 170)
#   --rows N          terminal rows (default 48)
#   --theme NAME      LUXE theme override written to the throwaway config
#   --out DIR         screenshot directory (default .pi/agent-loop/shots)
#   --cmd 'CMDLINE'   what to run in the terminal (default: target/release/luxe)
#   --keep            leave the terminal open when the script ends
#   -s 'STEPS'        inline steps, separated by `;` or newlines
#
# Steps (one per line, `#` comments allowed):
#   sleep <seconds>       wait (fractions fine)
#   type <text>           type literal text (no Enter)
#   enter <text>          type text then press Return
#   key <keysym>[ xN]     press a key by xkb name (Return, Escape, Tab, Down, F5, …), N times
#   mod <mods> <keysym>   press a chord, mods among ctrl/alt/shift/logo joined by `+`
#   shot <name>           capture the window to <out>/<name>.png (+ a downscaled .jpg)
#   log <text>            echo a progress note
#
# Environment passed through to the app: anything already exported, plus LUXE_* set by the caller.
# A throwaway XDG_CONFIG_HOME is built from your real ~/.config/luxe with the startup dashboard
# disabled, so the first keystroke always reaches the composer instead of a splash screen.

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cols=170
rows=48
theme=""
out_dir="${repo_root}/.pi/agent-loop/shots"
app_cmd="${repo_root}/target/release/luxe"
keep=false
inline_steps=""
script_file=""

usage() { sed -n '2,33p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; }

while [[ $# -gt 0 ]]; do
	case "$1" in
	--cols)
		cols="$2"
		shift 2
		;;
	--rows)
		rows="$2"
		shift 2
		;;
	--theme)
		theme="$2"
		shift 2
		;;
	--out)
		out_dir="$2"
		shift 2
		;;
	--cmd)
		app_cmd="$2"
		shift 2
		;;
	--keep)
		keep=true
		shift
		;;
	-s)
		inline_steps="$2"
		shift 2
		;;
	-h | --help)
		usage
		exit 0
		;;
	--)
		shift
		script_file="${1:-}"
		shift || true
		;;
	*)
		script_file="$1"
		shift
		;;
	esac
done

for tool in alacritty wtype grim hyprctl jq; do
	command -v "$tool" >/dev/null || {
		echo "missing required tool: $tool" >&2
		exit 2
	}
done

mkdir -p "$out_dir"

# A throwaway config dir mirroring the user's (keys, providers) with the dashboard off: typing has
# to land in the composer, and the splash screen swallows the first keystrokes.
config_home="$(mktemp -d /tmp/luxe-drive-XXXXXX)"
mkdir -p "${config_home}/luxe"
if [[ -d "${XDG_CONFIG_HOME:-$HOME/.config}/luxe" ]]; then
	cp -r "${XDG_CONFIG_HOME:-$HOME/.config}/luxe/." "${config_home}/luxe/" 2>/dev/null || true
fi
python3 - "$config_home" "$theme" <<'PY'
import json, pathlib, sys
home, theme = sys.argv[1], sys.argv[2]
path = pathlib.Path(home) / "luxe" / "config.json"
config = {}
if path.exists():
    try:
        config = json.loads(path.read_text())
    except Exception:
        config = {}
config["dashboard"] = False
config["notifications"] = False
if theme:
    config["theme"] = theme
path.write_text(json.dumps(config, indent=2))
PY

title="luxe-drive-$$"
window_class="luxe-drive"
font_size="${LUXE_DRIVE_FONT_SIZE:-11}"

# Hyprland 0.56 moved `dispatch` and the config parser to Lua, so the legacy spellings below now
# fail — and under `set -e` a failed `hyprctl` took the whole drive down before a single step ran.
# Both forms are tried, and neither is fatal: the worst case is a tiled window whose grid the tile
# decided, which `refresh_geometry` reads back anyway.
hypr() {
	hyprctl dispatch "$1" >/dev/null 2>&1 && return 0
	hyprctl dispatch "$2" >/dev/null 2>&1 && return 0
	return 0
}

# Float the terminal so the compositor honors the geometry alacritty asks for; tiled, the tile
# decides the grid and `--cols/--rows` become suggestions (a 170x48 request rendered 230x74).
hyprctl keyword windowrulev2 "float, class:^(${window_class})\$" >/dev/null 2>&1 || true

cleanup() {
	if [[ "$keep" == false && -n "${term_pid:-}" ]]; then
		kill "$term_pid" 2>/dev/null || true
	fi
	rm -rf "$config_home"
}
trap cleanup EXIT

# The throwaway XDG_CONFIG_HOME is for Luxe alone. gh finds its hosts.yml under the same variable,
# so without this every GitHub surface in the drive reads "not signed in" while gh is fine
export GH_CONFIG_DIR="${GH_CONFIG_DIR:-${XDG_CONFIG_HOME:-$HOME/.config}/gh}"

XDG_CONFIG_HOME="$config_home" alacritty \
	--class "$window_class,$window_class" \
	--title "$title" \
	-o "window.dimensions.columns=$cols" \
	-o "window.dimensions.lines=$rows" \
	-o "font.size=$font_size" \
	-e bash -lc "cd '$repo_root' && $app_cmd" &
term_pid=$!

# Wait for the window to map before typing at it.
geometry=""
for _ in $(seq 1 60); do
	geometry="$(hyprctl clients -j | jq -r --arg t "$title" '.[] | select(.title==$t) | "\(.at[0]),\(.at[1]) \(.size[0])x\(.size[1])"' | head -1)"
	[[ -n "$geometry" ]] && break
	sleep 0.2
done
if [[ -z "$geometry" ]]; then
	echo "terminal window '$title' never appeared" >&2
	exit 1
fi
# Focus it so keystrokes land here and not in whatever had focus.
hypr "hl.dsp.focus({ window = \"title:$title\" })" "focuswindow title:$title"

refresh_geometry() {
	geometry="$(hyprctl clients -j | jq -r --arg t "$title" '.[] | select(.title==$t) | "\(.at[0]),\(.at[1]) \(.size[0])x\(.size[1])"' | head -1)"
}

# Wait without going idle.
#
# A drive is mostly waiting on a provider, which to the compositor looks like nobody is at the
# keyboard — long enough and the screensaver/lock takes the screen, the captures become pictures of
# hyprlock, and every subsequent scripted keystroke is typed into a password field. So long waits
# are broken up and punctuated with a bare modifier press: real input to the compositor, a no-op to
# the app under test.
idle_safe_sleep() {
	local remaining="$1" chunk
	while awk -v r="$remaining" 'BEGIN { exit !(r > 0) }'; do
		chunk="$(awk -v r="$remaining" 'BEGIN { print (r > 30 ? 30 : r) }')"
		sleep "$chunk"
		remaining="$(awk -v r="$remaining" -v c="$chunk" 'BEGIN { print r - c }')"
		if awk -v r="$remaining" 'BEGIN { exit !(r > 0) }'; then
			wtype -M shift -m shift
		fi
	done
}

shot() {
	local name="$1"
	refresh_geometry
	local png="${out_dir}/${name}.png"
	# A locked or blanked screen draws no frames, and grim waits for one forever
	if ! timeout 15 grim -g "$geometry" "$png"; then
		echo "tui-drive: no capture within 15s; the screen is probably locked or asleep" >&2
		exit 1
	fi
	if command -v magick >/dev/null; then
		magick "$png" -resize 1280x -quality 88 "${out_dir}/${name}.jpg"
	fi
	echo "shot ${out_dir}/${name}.png"
}

run_step() {
	local line="$1"
	line="${line%%$'\r'}"
	# Leading space is how an inline `-s 'a; b'` arrives, and it used to make every step after
	# the first an unknown one.
	line="${line#"${line%%[![:space:]]*}"}"
	[[ -z "${line// /}" || "$line" == \#* ]] && return 0
	local verb="${line%% *}"
	local rest="${line#"$verb"}"
	rest="${rest# }"
	case "$verb" in
	sleep) idle_safe_sleep "$rest" ;;
	log) echo "-- $rest" ;;
	type) wtype -- "$rest" ;;
	enter)
		[[ -n "$rest" ]] && wtype -- "$rest"
		sleep 0.15
		wtype -k Return
		;;
	key)
		local keysym count
		keysym="${rest%% *}"
		count="${rest#"$keysym"}"
		count="${count// /}"
		[[ "$count" =~ ^x[0-9]+$ ]] && count="${count#x}" || count=1
		for _ in $(seq 1 "$count"); do
			wtype -k "$keysym"
			sleep 0.08
		done
		;;
	mod)
		local mods keysym
		mods="${rest%% *}"
		keysym="${rest#"$mods"}"
		keysym="${keysym# }"
		local args=()
		IFS='+' read -ra parts <<<"$mods"
		for part in "${parts[@]}"; do args+=(-M "$part"); done
		args+=(-k "$keysym")
		for part in "${parts[@]}"; do args+=(-m "$part"); done
		wtype "${args[@]}"
		;;
	shot) shot "$rest" ;;
	*)
		echo "unknown step: $line" >&2
		return 1
		;;
	esac
	return 0
}

steps() {
	if [[ -n "$inline_steps" ]]; then
		printf '%s\n' "${inline_steps//;/$'\n'}"
	elif [[ -n "$script_file" ]]; then
		cat "$script_file"
	else
		cat # stdin
	fi
}

while IFS= read -r line; do
	run_step "$line"
done < <(steps)

if [[ "$keep" == true ]]; then
	echo "terminal left open (pid $term_pid)"
fi
