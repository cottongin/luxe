#!/usr/bin/env bash
# Build the website's webfonts, subset to what the site actually draws.
#
# Three families: Instrument Serif for display, JetBrains Mono for everything a terminal would set,
# and a sliver of Adwaita Mono for the handful of marks JetBrains Mono does not draw. All three are
# SIL Open Font License 1.1, and none declares a Reserved Font Name — checked, because a
# subset is a "Modified Version" under the OFL and an RFN would forbid keeping the name. The license
# requires the copyright notice and license text to travel with the files, which `web/public/fonts/
# LICENSES.md` does.
#
# Why subset at all: the terminal face is a Nerd Font, and the unsubset file is several MB. The site
# draws a couple of hundred distinct characters, a dozen of them private-use icons. Shipping the
# other several thousand would be most of the page weight, for glyphs no visitor will ever see.
#
# The codepoint list is derived from the generated assets rather than guessed — run
# `scripts/site-assets.sh` first, since this reads its output.
#
# That derivation is also the failure mode: the assets are regenerated far more often than the
# fonts, and a character that appears in a new capture without a rebuild here renders as tofu with
# its own codepoint printed inside it. It happened — thirty-one glyphs, every Nerd Font icon and
# both Powerline caps among them, for nine days. So the build records what it covered in
# `coverage.txt` and `--check` holds the assets to it, which is a diff rather than a font parse and
# therefore cheap enough for CI to run on every push.
#
# `coverage.txt` is read back out of the *built* files, not from the list that was requested.
# `pyftsubset` silently drops a codepoint the source font does not have, so a manifest of the
# request would assert coverage the font does not deliver — which is the bug, restated. Anything
# neither face has is listed with its reason in `fallback-ok.txt` and left to the reader's own font,
# on the web through the CSS stack and in a terminal through fontconfig.
#
# **Why two mono faces.** JetBrains Mono is the one people already run, so it is what the site and
# every capture are set in. It lacks six marks luxe draws (`ℹ ↳ ↺ ⏸ ✔ ✖`), and a missing glyph on
# the web is whatever the reader's system substitutes, at whatever width. Adwaita Mono has all six
# on the same 0.6em cell, so those codepoints, and only those, come from it; the CSS stack names it
# second and the browser falls through per glyph.
#
# Requires: fonttools + brotli (`pip install fonttools brotli`), plus JetBrains Mono and Adwaita
# Mono Nerd Font Mono installed (or `JETBRAINS_DIR` / `ADWAITA_DIR`). `--check` needs none of it —
# bash and python3 only.
#
# Usage: scripts/site-fonts.sh [--check]

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
out_dir="${repo_root}/web/public/fonts"
assets_dir="${repo_root}/web/src/assets/generated"
# Recordings draw glyphs too, and they are not derived from the Rust source — so a capture that
# catches an icon no fixture ever drew would ship it as tofu, silently, to every reader without the
# face installed. Both directories feed the subset.
captures_dir="${repo_root}/web/src/assets/captures"
# The guide's screenshots are drawn on the site too, and so is the guide's own text, which names
# the symbols luxe draws: set in a face that lacks them, a browser reaches for color emoji.
guide_images_dir="${repo_root}/docs/guide/images"
guide_dir="${repo_root}/docs/guide"
coverage_file="${out_dir}/coverage.txt"
fallback_file="${out_dir}/fallback-ok.txt"
work_dir="$(mktemp -d)"
trap 'rm -rf "${work_dir}"' EXIT

check=false
case "${1:-}" in
--check) check=true ;;
-h | --help)
	sed -n '2,39p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
	exit 0
	;;
'') ;;
*)
	echo "unknown argument: $1" >&2
	exit 2
	;;
esac

# What the generated assets actually draw, read out of them. One definition, used by the build and
# by the check, or the two can disagree about what coverage means.
asset_codepoints() {
	python3 - "${assets_dir}" "${captures_dir}" "${guide_images_dir}" "${guide_dir}" <<'PY'
import glob, json, re, sys

used = set()
for directory in sys.argv[1:]:
    for path in glob.glob(f"{directory}/*.svg"):
        body = re.sub(r"<[^>]+>", "", open(path, encoding="utf-8").read())
        used.update(body.replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&"))
    for path in glob.glob(f"{directory}/*.md"):
        used.update(open(path, encoding="utf-8").read())
    for path in glob.glob(f"{directory}/*.cast"):
        for line in open(path, encoding="utf-8"):
            try:
                event = json.loads(line)
            except ValueError:
                continue
            if isinstance(event, list) and len(event) == 3:
                used.update(event[2])

# Control bytes are escape sequences, and variation selectors pick a presentation; neither is drawn.
glyphs = sorted(
    c for c in used if ord(c) >= 0x20 and ord(c) != 0x7F and not 0xFE00 <= ord(c) <= 0xFE0F
)
print(",".join(f"U+{ord(c):04X}" for c in glyphs))
PY
}

if [[ "${check}" == true ]]; then
	[[ -f "${coverage_file}" ]] || {
		echo "no ${coverage_file} — run scripts/site-fonts.sh" >&2
		exit 1
	}
	missing="$(
		comm -23 \
			<(asset_codepoints | tr ',' '\n' | sort -u) \
			<(cat "${coverage_file}" "${fallback_file}" |
				sed 's/#.*//' | tr -d '[:blank:]' | grep . | sort -u)
	)"
	if [[ -n "${missing}" ]]; then
		echo "the webfonts do not cover what the assets draw:" >&2
		echo "${missing}" | tr '\n' ' ' >&2
		echo >&2
		echo "each of these renders as tofu — run scripts/site-fonts.sh and commit the result" >&2
		exit 1
	fi
	echo "webfonts cover every character the assets draw"
	exit 0
fi

command -v pyftsubset >/dev/null || {
	echo "pyftsubset not found — pip install fonttools brotli" >&2
	exit 1
}

# Every style has to come from one directory, and the file names are the upstream ones, so a release
# unpacked anywhere works with the directory variable pointed at it.
find_face() {
	local override="$1" file="$2" candidate
	shift 2
	if [[ -n "${override}" ]]; then
		[[ -f "${override}/${file}" ]] && echo "${override}"
		return 0
	fi
	for candidate in "$@"; do
		[[ -f "${candidate}/${file}" ]] && echo "${candidate}" && return 0
	done
	return 0
}
font_dirs=("${HOME}/.local/share/fonts" /usr/share/fonts/TTF /usr/share/fonts/truetype "${HOME}/Library/Fonts")
jetbrains_dir="$(find_face "${JETBRAINS_DIR:-}" JetBrainsMonoNerdFontMono-Regular.ttf \
	"${HOME}/.local/share/fonts/JetBrainsMonoNFM" "${font_dirs[@]}")"
[[ -n "${jetbrains_dir}" ]] || {
	echo "JetBrains Mono Nerd Font Mono not found (set JETBRAINS_DIR to the directory holding" >&2
	echo "JetBrainsMonoNerdFontMono-*.ttf — https://github.com/ryanoasis/nerd-fonts/releases)" >&2
	exit 1
}
adwaita_dir="$(find_face "${ADWAITA_DIR:-}" AdwaitaMonoNerdFontMono-Regular.ttf "${font_dirs[@]}")"
[[ -n "${adwaita_dir}" ]] || {
	echo "Adwaita Mono Nerd Font Mono not found (set ADWAITA_DIR to the directory holding" >&2
	echo "AdwaitaMonoNerdFontMono-*.ttf — https://github.com/ryanoasis/nerd-fonts/releases)" >&2
	exit 1
}

# The woff2 encoder needs brotli, so ask the encoder — not the ambient `python3`. `pyftsubset` is
# commonly installed into its own virtualenv (pipx, `uv tool`), where importing brotli here proves
# nothing and refuses a setup that works.
pyftsubset "${jetbrains_dir}/JetBrainsMonoNerdFontMono-Regular.ttf" \
	--output-file="${work_dir}/probe.woff2" \
	--flavor=woff2 --unicodes='U+0041' 2>/dev/null
[[ -s "${work_dir}/probe.woff2" ]] || {
	echo "pyftsubset cannot write woff2 — its interpreter needs brotli (pip install brotli)" >&2
	exit 1
}

mkdir -p "${out_dir}"

codepoints="$(asset_codepoints)"
[[ -n "${codepoints}" ]] || {
	echo "no codepoints found — run scripts/site-assets.sh first" >&2
	exit 1
}

subset() {
	local src="$1" dst="$2" unicodes="$3"
	pyftsubset "${src}" \
		--output-file="${dst}" \
		--flavor=woff2 \
		--unicodes="${unicodes}" \
		--layout-features= \
		--no-hinting \
		--desubroutinize \
		--name-IDs='0,1,2,3,4,5,6,13,14' \
		--notdef-outline 2>/dev/null
	printf '  %-34s %6s\n' "$(basename "${dst}")" "$(stat -c%s "${dst}")"
}

# The terminal face. Regular and Bold are what luxe's cells ask for; Italic is there because reasoning
# and a few labels are set in it, and a synthesized slant of an upright mono reads as a rendering bug.
pyftsubset_python="$(head -1 "$(command -v pyftsubset)" | sed 's/^#!//')"
cmap() {
	${pyftsubset_python} - "$1" <<'PY'
import sys

from fontTools.ttLib import TTFont

for codepoint in sorted(TTFont(sys.argv[1]).getBestCmap()):
    print(f"U+{codepoint:04X}")
PY
}

echo "terminal face — JetBrains Mono Nerd Font Mono, the assets' own characters plus printable ASCII:"
for style in Regular Bold Italic; do
	subset "${jetbrains_dir}/JetBrainsMonoNerdFontMono-${style}.ttf" \
		"${out_dir}/jetbrains-mono-nfm-${style,,}.woff2" \
		"${codepoints},U+0020-007E"
done

# What JetBrains Mono could not carry, and nothing else, from Adwaita Mono. Computed from the built
# file rather than listed, so a glyph a future JetBrains release adds drops out of the fallback on
# its own.
requested="$(printf '%s' "${codepoints}" | tr ',' '\n' | sort -u)"
primary="$(cmap "${out_dir}/jetbrains-mono-nfm-regular.woff2" | sort -u)"
gaps="$(comm -23 <(printf '%s\n' "${requested}") <(printf '%s\n' "${primary}") | paste -sd, -)"
rm -f "${out_dir}"/adwaita-mono-nfm-*.woff2
if [[ -n "${gaps}" ]]; then
	echo "fallback face — Adwaita Mono Nerd Font Mono, what JetBrains Mono lacks (${gaps}):"
	for style in Regular Bold; do
		subset "${adwaita_dir}/AdwaitaMonoNerdFontMono-${style}.ttf" \
			"${out_dir}/adwaita-mono-nfm-${style,,}.woff2" \
			"${gaps}"
	done
fi

# Display face: latin plus the punctuation editorial copy actually uses.
latin='U+0020-007E,U+00A0-00FF,U+2010-2015,U+2018-201F,U+2022,U+2026,U+2032-2033,U+20AC,U+2122'
echo "display face — Instrument Serif, latin:"
for style in Regular Italic; do
	url="https://github.com/google/fonts/raw/main/ofl/instrumentserif/InstrumentSerif-${style}.ttf"
	curl -sSL -o "${work_dir}/${style}.ttf" "${url}"
	subset "${work_dir}/${style}.ttf" "${out_dir}/instrument-serif-${style,,}.woff2" "${latin}"
done

curl -sSL -o "${work_dir}/instrument-ofl.txt" \
	"https://github.com/google/fonts/raw/main/ofl/instrumentserif/OFL.txt"
cp "${work_dir}/instrument-ofl.txt" "${out_dir}/OFL-InstrumentSerif.txt"
curl -sSL -o "${out_dir}/OFL-JetBrainsMono.txt" \
	"https://raw.githubusercontent.com/JetBrains/JetBrainsMono/master/OFL.txt"
curl -sSL -o "${out_dir}/OFL-AdwaitaMono.txt" \
	"https://gitlab.gnome.org/GNOME/adwaita-fonts/-/raw/main/LICENSE"
rm -f "${out_dir}/OFL-Lilex.txt" "${out_dir}"/lilex-nf-mono-*.woff2 \
	"${out_dir}/OFL-Iosevka.txt" "${out_dir}"/iosevka-term-nfm-*.woff2

# What the built files actually carry, read back out of them — see the header. One codepoint per
# line, so the check is a `comm` between two sorted lists rather than a font parse. Both mono faces
# count: the fallback exists to cover exactly what the first one drops.
{
	printf '%s\n' "${primary}"
	if [[ -f "${out_dir}/adwaita-mono-nfm-regular.woff2" ]]; then
		cmap "${out_dir}/adwaita-mono-nfm-regular.woff2"
	fi
} | sort -u >"${coverage_file}"

dropped="$(comm -23 <(printf '%s\n' "${requested}") <(sort -u "${coverage_file}"))"
if [[ -n "${dropped}" ]]; then
	echo
	echo "in neither mono face, so the reader's own font draws them:"
	echo "${dropped}" | tr '\n' ' '
	echo
	echo "list them in $(basename "${fallback_file}") with a reason, or stop drawing them"
fi

echo
echo "wrote $(find "${out_dir}" -name '*.woff2' | wc -l) fonts, $(du -sh "${out_dir}" | cut -f1) total"
