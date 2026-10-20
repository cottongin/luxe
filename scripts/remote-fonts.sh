#!/usr/bin/env bash
# Build the phone app's webfonts into crates/luxe-remote/app/fonts/.
#
# The same two faces as the website (see scripts/site-fonts.sh for the licensing, which holds here
# too): JetBrains Mono Nerd Font Mono, and Adwaita Mono for the marks it lacks. The subset differs.
# The site draws a known set of characters; the app draws whatever a model writes. So it keeps the
# ranges text is written in (Latin, Greek, Cyrillic, punctuation, arrows, maths, box drawing) plus
# every glyph Luxe draws: the TUI's glyph table at every tier (the glyph_codepoints example), and,
# when the website has been built, its coverage.txt, which catches what captures show outside the
# table. Anything else falls through the CSS stack to the phone's own font, one glyph at a time.
#
# The fonts are embedded in the luxe binary, so the size matters twice: once in the download, and
# once on every phone's first load.
#
# Requires: fonttools + brotli, and both faces installed (or JETBRAINS_DIR / ADWAITA_DIR). The OFL
# texts beside the fonts are committed and don't change, so they aren't copied again.
#
# Usage: scripts/remote-fonts.sh

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
out_dir="${repo_root}/crates/luxe-remote/app/fonts"
site_fonts="${repo_root}/web/public/fonts"
jetbrains_dir="${JETBRAINS_DIR:-${HOME}/.local/share/fonts/JetBrainsMonoNFM}"
adwaita_dir="${ADWAITA_DIR:-/usr/share/fonts/TTF}"

for needed in "${jetbrains_dir}/JetBrainsMonoNerdFontMono-Regular.ttf" \
	"${adwaita_dir}/AdwaitaMonoNerdFontMono-Regular.ttf"; do
	[[ -f "${needed}" ]] || {
		echo "missing ${needed} (the header of scripts/remote-fonts.sh says where each comes from)" >&2
		exit 1
	}
done
command -v pyftsubset >/dev/null || {
	echo "pyftsubset not found — pip install fonttools brotli" >&2
	exit 1
}

text='U+0020-007E,U+00A0-024F,U+0370-03FF,U+0400-04FF,U+2000-206F,U+20A0-20CF,U+2100-214F'
symbols='U+2190-21FF,U+2200-22FF,U+2300-23FF,U+2500-259F,U+25A0-25FF,U+2700-27BF'
luxe_glyphs() {
	cargo run -q -p luxe-tui --example glyph_codepoints --manifest-path "${repo_root}/Cargo.toml"
	if [[ -f "${site_fonts}/coverage.txt" ]]; then
		sed 's/#.*//' "${site_fonts}/coverage.txt" | tr -d '[:blank:]' | grep .
	fi
}
luxe="$(luxe_glyphs | sort -u | paste -sd, -)"
unicodes="${text},${symbols},${luxe}"

subset() {
	pyftsubset "$1" --output-file="$2" --flavor=woff2 --unicodes="$3" \
		--layout-features= --no-hinting --desubroutinize \
		--name-IDs='0,1,2,3,4,5,6,13,14' --notdef-outline 2>/dev/null
	printf '  %-34s %7s\n' "$(basename "$2")" "$(stat -c%s "$2")"
}

pyftsubset_python="$(head -1 "$(command -v pyftsubset)" | sed 's/^#!//')"
cmap() {
	${pyftsubset_python} -c 'import sys
from fontTools.ttLib import TTFont
print("\n".join(f"U+{c:04X}" for c in sorted(TTFont(sys.argv[1]).getBestCmap())))' "$1"
}

mkdir -p "${out_dir}"
rm -f "${out_dir}"/*.woff2
for style in Regular Bold Italic; do
	subset "${jetbrains_dir}/JetBrainsMonoNerdFontMono-${style}.ttf" \
		"${out_dir}/jetbrains-mono-${style,,}.woff2" "${unicodes}"
done

# Only Luxe's own marks come from Adwaita: filling every gap in the text ranges would add its
# drawing of characters JetBrains Mono chose to leave out, for little a phone can't draw itself
gaps="$(comm -23 <(printf '%s' "${luxe}" | tr ',' '\n' | sort -u) \
	<(cmap "${out_dir}/jetbrains-mono-regular.woff2" | sort -u) | paste -sd, -)"
if [[ -n "${gaps}" ]]; then
	for style in Regular Bold; do
		subset "${adwaita_dir}/AdwaitaMonoNerdFontMono-${style}.ttf" \
			"${out_dir}/adwaita-mono-${style,,}.woff2" "${gaps}"
	done
fi

echo "wrote $(du -sh "${out_dir}" | cut -f1) to ${out_dir#"${repo_root}/"}"
