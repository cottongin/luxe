#!/usr/bin/env bash
# Record a demo for the website by driving the real luxe in a real terminal.
#
# These used to be scripted through the reducer with no terminal involved, which made them exactly
# reproducible and exactly as much work as writing the app twice. This runs the shipped binary
# against a real provider in Ghostty and records what a person would have seen — so a demo is a
# take, and re-shooting it after a visual change is re-running one command rather than editing a
# thousand lines of Rust.
#
# What that costs, stated plainly: a recording is no longer verifiable by CI, so nothing checks
# that it still matches the build. `web/src/assets/captures/manifest.json` records what each take
# was shot against, and re-recording is the only way to refresh one.
#
# Usage:
#   scripts/site-record.sh [options] <script-file>
#
# Options:
#   --name NAME       output basename (default: the script's own)
#   --cols N          terminal columns (default 92 — see below)
#   --rows N          terminal rows (default 28)
#   --font-size N     Ghostty font size (default 13)
#   --theme NAME      luxe theme (default luxe)
#   --project DIR     where to run luxe (default: a scratch copy of web/demo)
#   --project-src DIR the project template to copy
#   --project-name N  what the scratch copy is called, which is the name every surface shows
#   --max-idle SECS   cap on a gap between frames after editing (default 1.2)
#   --speed N         play the finished take N times faster (default 1.0)
#   --timeout SECS    abandon a take that has not finished (default 900)
#   --out DIR         where the finished cast lands (default web/src/assets/captures)
#   --raw             keep the unedited recording beside the finished one
#   --keep-project    leave the scratch project behind for inspection
#   --monitor NAME    which output the recording window opens on, when --watch is set
#   --watch           put the recording window on screen instead of hiding it
#   --seed 'CMD'      shell run before the first commit, so its result is the project's history
#   --setup 'CMD'     shell run after it, so its result is an uncommitted change
#   --model P/ID      which model answers (default: LUXE_RECORD_MODEL, else whatever is remembered)
#   --thinking LEVEL  reasoning level for the take
#   --glyphs STYLE    glyph fidelity: auto, nerd, nerd_wide, unicode, ascii
#   --bin DIR         put DIR first on luxe's PATH (relative to web/demo-scripts), for stand-ins
#
# Both are how a demo gets a starting state without spending a turn on it: a changes-and-commit
# demo needs a dirty tree, and paying a provider to manufacture one so the demo can then talk about
# it is a minute of recording nobody watches.
#
# Which side of the commit matters. A defect introduced *after* it is a defect `git checkout` can
# undo, and a model asked to fix one will notice — one take ended with "restoring working tree to
# HEAD", which is a correct answer to a question the demo did not mean to ask. `--seed` puts it in
# history instead, where fixing it means editing the code.
#
# 92 columns is the whole mobile strategy. A demo is drawn at a fixed size per character, so the
# column count *is* the type size — and the review surface at 180 columns is three pixels a
# character on a phone, which is the same as not being there. 92 carries a two-pane review and
# still reads at 390px wide.
#
# The keystrokes go into luxe's pty, never to the compositor. `wtype` was the obvious way to drive
# a TUI and it works right up until focus moves, after which every remaining key — `Ctrl+C`
# included — is typed into whatever window took it. `scripts/site-drive.py` writes to the pty
# instead, which cannot reach another window and does not need the recording to hold focus.
#
# The step language is documented in that script.
#
# A demo declares its own options in `#:` directives at the top of its script, so re-shooting the
# set is one command and the size a demo needs lives with the demo rather than in somebody's shell
# history. A flag on the command line overrides its directive.
#
#   #: cols 150
#   #: speed 1.6
#   #: setup sed -i 's/x/y/' src/filter.js

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
name=""
cols=92
rows=28
font_size=13
theme="luxe"
project=""
project_src="${repo_root}/web/demo"
# The directory name is the project name in the statusline, the dock and the dashboard, so it is
# part of the shot rather than an implementation detail of the scratch copy.
project_name="todo"
max_idle=1.2
speed=1.0
take_timeout=900
out_dir="${repo_root}/web/src/assets/captures"
keep_raw=false
keep_project=false
script_file=""
monitor="${LUXE_RECORD_MONITOR:-}"
watch=false
setup_cmd=""
seed_cmd=""
model="${LUXE_RECORD_MODEL:-}"
thinking="${LUXE_RECORD_THINKING:-}"
glyphs=""
bin_dir=""

usage() { sed -n '2,40p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; }

while [[ $# -gt 0 ]]; do
	case "$1" in
	--name) name="$2" && shift 2 ;;
	--cols) cols="$2" && shift 2 ;;
	--rows) rows="$2" && shift 2 ;;
	--font-size) font_size="$2" && shift 2 ;;
	--theme) theme="$2" && shift 2 ;;
	--project) project="$2" && shift 2 ;;
	--project-src) project_src="$2" && shift 2 ;;
	--project-name) project_name="$2" && shift 2 ;;
	--max-idle) max_idle="$2" && shift 2 ;;
	--speed) speed="$2" && shift 2 ;;
	--timeout) take_timeout="$2" && shift 2 ;;
	--out) out_dir="$2" && shift 2 ;;
	--raw) keep_raw=true && shift ;;
	--keep-project) keep_project=true && shift ;;
	--monitor) monitor="$2" && shift 2 ;;
	--watch) watch=true && shift ;;
	--setup) setup_cmd="$2" && shift 2 ;;
	--seed) seed_cmd="$2" && shift 2 ;;
	--model) model="$2" && shift 2 ;;
	--thinking) thinking="$2" && shift 2 ;;
	--glyphs) glyphs="$2" && shift 2 ;;
	--bin) bin_dir="$2" && shift 2 ;;
	-h | --help)
		usage
		exit 0
		;;
	-*)
		echo "unknown flag: $1" >&2
		exit 2
		;;
	*) script_file="$1" && shift ;;
	esac
done

[[ -n "$script_file" ]] || {
	usage
	exit 2
}
[[ -f "$script_file" ]] || {
	echo "no such script: $script_file" >&2
	exit 2
}
[[ -n "$name" ]] || name="$(basename "${script_file%.*}")"

# Directives first, so an explicit flag still wins: everything above has already been parsed, so
# only a setting still at its default is taken from the file.
while IFS= read -r directive; do
	key="${directive%% *}"
	value="${directive#"$key"}"
	value="${value# }"
	case "$key" in
	cols) [[ "$cols" == 92 ]] && cols="$value" ;;
	rows) [[ "$rows" == 28 ]] && rows="$value" ;;
	speed) [[ "$speed" == 1.0 ]] && speed="$value" ;;
	max-idle) [[ "$max_idle" == 1.2 ]] && max_idle="$value" ;;
	setup) [[ -z "$setup_cmd" ]] && setup_cmd="$value" ;;
	seed) [[ -z "$seed_cmd" ]] && seed_cmd="$value" ;;
	model) [[ -z "$model" ]] && model="$value" ;;
	thinking) [[ -z "$thinking" ]] && thinking="$value" ;;
	glyphs) [[ -z "$glyphs" ]] && glyphs="$value" ;;
	bin) [[ -z "$bin_dir" ]] && bin_dir="$value" ;;
	project-name) [[ "$project_name" == todo ]] && project_name="$value" ;;
	*)
		echo "unknown directive in ${script_file}: ${key}" >&2
		exit 2
		;;
	esac
done < <(sed -n 's/^#: *//p' "$script_file")

for tool in ghostty python3 git; do
	command -v "$tool" >/dev/null || {
		echo "missing required tool: $tool" >&2
		exit 2
	}
done

luxe_bin="${repo_root}/target/release/luxe"
[[ -x "$luxe_bin" ]] || {
	echo "no release binary at ${luxe_bin} — run scripts/install-dev.sh first" >&2
	exit 2
}

mkdir -p "$out_dir"
work="$(mktemp -d /tmp/luxe-record-XXXXXX)"
raw_cast="${work}/raw.cast"
marks_file="${work}/marks"
drive_log="${work}/drive.log"

# === The terminal ===

# Ghostty wears the luxe palette. The app paints nearly every cell itself, but not the ones it has
# not reached yet — and a demo whose first frame is somebody else's background color announces that
# it was staged.
theme_json="${repo_root}/crates/luxe-core/src/themes/${theme}.json"
[[ -f "$theme_json" ]] || {
	echo "no such theme: ${theme_json}" >&2
	exit 2
}
read -r term_bg term_fg < <(python3 -c "
import json, sys
palette = json.load(open(sys.argv[1]))['vars']
print(palette['bg'], palette['fg'])
" "$theme_json")

ghostty_config="${work}/ghostty.conf"
cat >"$ghostty_config" <<CONF
font-family = JetBrainsMono Nerd Font Mono
font-size = ${font_size}
window-width = ${cols}
window-height = ${rows}
window-padding-x = 0
window-padding-y = 0
window-decoration = none
window-inherit-font-size = false
background = ${term_bg}
foreground = ${term_fg}
confirm-close-surface = false
resize-overlay = never
mouse-hide-while-typing = true
cursor-style-blink = false
shell-integration = none
CONF

# === The project under the demo ===

# A scratch copy, every time. A demo that edits a file has to start from the same file next time,
# and a template that accumulates the last five takes is a template that eventually shows them.
if [[ -z "$project" ]]; then
	[[ -d "$project_src" ]] || {
		echo "no demo project at ${project_src} — pass --project" >&2
		exit 2
	}
	project="${work}/${project_name}"
	cp -r "$project_src" "$project"
	if [[ -n "$seed_cmd" ]]; then
		(cd "$project" && DEMO_SCRIPTS="${repo_root}/web/demo-scripts" eval "$seed_cmd")
	fi
	git -C "$project" init --quiet --initial-branch=main
	# Configured on the repository, not passed per command. A `git commit` with no identity anywhere
	# cannot proceed, so an agent asked to commit correctly stops and asks who it is — which is good
	# behavior and a poor demo, and it is the recorder's doing rather than something to talk the
	# model out of in the prompt.
	git -C "$project" config user.name "luxe"
	git -C "$project" config user.email "demo@luxe.dev"
	git -C "$project" add -A
	git -C "$project" commit --quiet -m "Initial commit"
fi

# After the commit, so whatever it does shows up as a change rather than as the baseline.
#
# `DEMO_SCRIPTS` because the command runs in the scratch project, which is somewhere under /tmp —
# so a change big enough to live in its own file has no relative path to reach itself by.
if [[ -n "$setup_cmd" ]]; then
	(cd "$project" && DEMO_SCRIPTS="${repo_root}/web/demo-scripts" eval "$setup_cmd")
fi

# === luxe's own config ===

# Copied from the real one so the credentials and the model are the ones being demonstrated, then
# overridden where a demo needs a fixed answer. Never the real directory: a take must not be able
# to write to it, and a theme set for a recording must not outlive it.
config_home="${work}/config"
mkdir -p "${config_home}/luxe"
real_config="${XDG_CONFIG_HOME:-$HOME/.config}/luxe"
if [[ -d "$real_config" ]]; then
	# Not `mcp.json`: the operator's servers would sit in the picker's MCP tab of every take, and
	# connect to real services on the way.
	find "$real_config" -maxdepth 1 -name '*.json' ! -name 'auth.json' ! -name 'mcp.json' \
		-exec cp {} "${config_home}/luxe/" \;
	# Linked, not copied. An OAuth credential rotates: the provider hands back a new refresh token
	# and invalidates the one just used, and luxe writes it to whichever auth file it was given. A
	# copy in a scratch directory therefore takes the rotation with it when the directory is
	# deleted, and every later take authenticates with a token the provider has already retired —
	# which is a signed-out account after one recording, not a recording problem you notice at the
	# time. The real file is the credential store; a take uses it exactly as a normal run would.
	[[ -e "${real_config}/auth.json" ]] &&
		ln -s "${real_config}/auth.json" "${config_home}/luxe/auth.json"
fi
python3 - "$config_home" "$theme" "$glyphs" <<'PY'
import json, pathlib, sys

home, theme, glyphs = sys.argv[1], sys.argv[2], sys.argv[3]
path = pathlib.Path(home) / "luxe" / "config.json"
config = json.loads(path.read_text()) if path.exists() else {}
config["theme"] = theme
if glyphs:
    config["glyphs"] = glyphs
# Straight into a session. The dashboard is a real surface and it has its own demo if it ever wants
# one, but as a preamble it costs every take three seconds of a screen the demo is not about.
config["dashboard"] = False
# A desktop notification landing mid-take is somebody else's window in the shot.
config["notifications"] = False
# The update check is one request that can only make a recording slower or wrong.
config.setdefault("update", {})["check"] = False
path.write_text(json.dumps(config, indent=2))
PY

# A scratch state directory, always. The remembered model outranks `default_model`, and it lives
# outside the config directory — so without this a take silently inherits whichever model the
# operator last picked, which is how a demo meant for a local model got recorded against a frontier
# one. Isolating it also stops a recording from writing back to real state.
state_home="${work}/state"
mkdir -p "${state_home}/luxe"
python3 - "$state_home" "$model" <<'PY'
import json, pathlib, sys

home, model = sys.argv[1], sys.argv[2]
state = {"schema_version": 1}
if model:
    provider, _, identifier = model.partition("/")
    if not identifier:
        sys.exit(f"--model wants provider/id, got {model!r}")
    state["model"] = {"provider": provider, "id": identifier}
path = pathlib.Path(home) / "luxe" / "state.json"
path.write_text(json.dumps(state, indent=2))
path.chmod(0o600)
PY

# `--thinking` on the command line, not in the state file. A model's own configured level in
# `models.json` outranks the remembered one, so a level written to state is silently ignored for
# exactly the models that declare a default — which is the set worth overriding.
luxe_args=()
[[ -n "$thinking" ]] && luxe_args+=(--thinking "$thinking")

# A stand-in for a program the demo project has no real counterpart for, like `gh` for a repository
# that doesn't exist. First on PATH, so luxe and every command the model runs find it.
record_path="$PATH"
if [[ -n "$bin_dir" ]]; then
	[[ "$bin_dir" == /* ]] || bin_dir="${repo_root}/web/demo-scripts/${bin_dir}"
	[[ -d "$bin_dir" ]] || {
		echo "no such bin directory: ${bin_dir}" >&2
		exit 2
	}
	record_path="${bin_dir}:${PATH}"
fi

# === Launch ===

cleanup() {
	# `|| true`: by the time this runs the terminal has usually already gone, and under `set -e` a
	# failed kill in the exit trap turns a finished recording into a non-zero exit.
	if [[ -n "${term_pid:-}" ]]; then
		kill "$term_pid" 2>/dev/null || true
	fi
	if [[ "$keep_project" == true ]]; then
		echo "project left at ${project}"
	else
		rm -rf "$work"
	fi
	return 0
}
trap cleanup EXIT

# By default the window is hidden on a silent special workspace, and that is the important setting
# rather than a convenience.
#
# A take does not need to be seen: the recording comes from the pty the driver owns, and a hidden
# GTK window still answers the device queries luxe asks. What an on-screen window *does* do is sit
# in the way — it takes focus, it lands on whichever monitor the pointer last touched, and the
# operator cannot use their machine without wondering whether they just spoiled a minute of
# recording. `silent` keeps the workspace from being switched to.
#
# `--watch` puts it back on screen for the times you need to see what a script is actually doing.
# A class of its own, per run.
#
# `hyprctl keyword windowrulev2` *adds* a rule for the rest of the compositor's session; there is no
# unset. So a run that once asked for `float, size, center, monitor DP-2` leaves those rules behind,
# and every later run of the hidden variant matches them too — the window floats onto whichever
# monitor last had the pointer instead of going quietly to its workspace. A unique class means only
# this run's rules can ever match this run's window.
#
# The segment cannot start with a digit: Ghostty validates the class as a GTK application id and
# silently ignores an invalid one, which puts the window back under the desktop's own class where
# no rule reaches it.
window_class="dev.luxe.record.r$$"
rule() { hyprctl keyword windowrulev2 "$1, class:^(${window_class})\$" >/dev/null 2>&1 || true; }

if [[ "$watch" == true ]]; then
	# Ghostty's own `window-width`/`window-height` are in cells and Hyprland overrides them for a
	# new float, so the size is asked for again in pixels. The cell is JetBrains Mono's own box, a
	# 0.6em advance by a 1.32em line, at the 20px a 13pt face measures on this display.
	read -r window_px_w window_px_h < <(python3 -c "
import sys
cols, rows, size = (int(a) for a in sys.argv[1:4])
print(round(cols * 12.02 * size / 13), round(rows * 26.45 * size / 13))
" "$cols" "$rows" "$font_size")
	rule "float"
	rule "size ${window_px_w} ${window_px_h}"
	rule "center"
	[[ -n "$monitor" ]] && rule "monitor ${monitor}"
else
	rule "workspace special:luxe-record silent"
	rule "noanim"
fi

echo "==> recording ${name} (${cols}x${rows}, ${theme})"

XDG_CONFIG_HOME="$config_home" \
	XDG_STATE_HOME="$state_home" \
	TERM=xterm-256color \
	LUXE_UPDATE_CHECK=0 \
	LUXE_WIRE_LOG="${work}/wire.jsonl" \
	PATH="$record_path" \
	ghostty \
	--config-default-files=false \
	--config-file="$ghostty_config" \
	--gtk-single-instance=false \
	--class="${window_class}" \
	--title="luxe-record-$$" \
	-e python3 "${repo_root}/scripts/site-drive.py" \
	--script "$script_file" \
	--out "$raw_cast" \
	--marks "$marks_file" \
	--log "$drive_log" \
	--cols "$cols" --rows "$rows" \
	--cwd "$project" \
	--title "luxe — ${name}" \
	-- "$luxe_bin" "${luxe_args[@]}" &
term_pid=$!

# A take that never finishes is a script with a step that never lands. Bounded, so a bad script
# costs a wait rather than a window nobody notices is still open.
waited=0
while kill -0 "$term_pid" 2>/dev/null; do
	sleep 1
	waited=$((waited + 1))
	if ((waited > take_timeout)); then
		echo "take exceeded ${take_timeout}s — abandoning" >&2
		kill "$term_pid" 2>/dev/null || true
		break
	fi
done
wait "$term_pid" 2>/dev/null || true

# The driver's notes, always — a take that went wrong is exactly the one whose window is gone.
if [[ -f "$drive_log" ]]; then
	cp "$drive_log" "${out_dir}/${name}.log"
fi
# And what the take actually said and sent: the session it wrote and every request and reply on
# the wire. A card that went red for no reason the cast can show is answered by these, and the
# scratch project they lived in is about to be deleted. Kept beside the driver's notes, and like
# them never committed (`luxe wire` reads the second).
if [[ -f "${work}/wire.jsonl" ]]; then
	cp "${work}/wire.jsonl" "${out_dir}/${name}.wire.jsonl"
fi
session_file="$(ls -t "${project}"/.luxe/sessions/*/*.jsonl 2>/dev/null | head -1 || true)"
if [[ -n "$session_file" ]]; then
	cp "$session_file" "${out_dir}/${name}.session.jsonl"
fi

[[ -s "$raw_cast" ]] || {
	echo "nothing was recorded — see ${out_dir}/${name}.log" >&2
	exit 1
}

# === Cut ===

python3 "${repo_root}/scripts/cast-edit.py" "$raw_cast" \
	--out "${out_dir}/${name}.cast" \
	--marks "$marks_file" \
	--max-idle "$max_idle" \
	--speed "$speed" \
	--title "luxe — ${name}"

[[ "$keep_raw" == true ]] && cp "$raw_cast" "${out_dir}/${name}.raw.cast"

python3 - "$out_dir" "$name" "$cols" "$rows" "$theme" "$script_file" "$repo_root" <<'PY'
import datetime, json, pathlib, subprocess, sys

out_dir, name, cols, rows, theme, script, root = sys.argv[1:8]
path = pathlib.Path(out_dir) / "manifest.json"
manifest = json.loads(path.read_text()) if path.exists() else {}


def relative(target: str) -> str:
    """Names the script inside the repository, or absolutely for a one-off from elsewhere."""
    try:
        return str(pathlib.Path(target).resolve().relative_to(pathlib.Path(root).resolve()))
    except ValueError:
        return str(pathlib.Path(target).resolve())


def git(*args: str) -> str:
    try:
        done = subprocess.run(["git", "-C", root, *args], capture_output=True, text=True, check=True)
        return done.stdout.strip()
    except (subprocess.CalledProcessError, OSError):
        return "unknown"


manifest[name] = {
    "recorded": datetime.datetime.now().astimezone().strftime("%Y-%m-%d"),
    "commit": git("rev-parse", "--short", "HEAD"),
    "dirty": bool(git("status", "--porcelain")),
    "cols": int(cols),
    "rows": int(rows),
    "theme": theme,
    "script": relative(script),
}
path.write_text(json.dumps(dict(sorted(manifest.items())), indent=2) + "\n")
PY

echo "==> ${out_dir}/${name}.cast"
