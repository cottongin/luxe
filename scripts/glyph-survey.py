#!/usr/bin/env python3
"""glyph-survey — ask real font files which characters the fallback tiers may use.

`glyphs.rs` has two tiers that promise portability, and for a long time nothing checked the promise
against a font. The tests asserted no private-use codepoints and one cell wide by the Unicode width
property; neither is a question about what the reader's font contains. Measured, Liberation Mono was
missing twenty of the thirty-one marks the TUI draws at every tier — including the geometric shapes
chosen *because* they were supposed to be safe.

This is the measurement behind `crates/luxe-tui/src/glyphs/portable.txt`. It reads the installed
files with fontTools rather than trusting a vendor's claim, and it refuses to run rather than
quietly survey eight faces when it was asked for nine — a panel with a hole in it produces a list
that is wrong in the one direction that matters.

    scripts/glyph-survey.py                  # report coverage for the current vocabulary
    scripts/glyph-survey.py --candidates     # which of a wider palette every face has
    scripts/glyph-survey.py --check          # portable.txt still holds (CI)
    scripts/glyph-survey.py --face FILE...   # does a face draw everything luxe does, cleanly

U+2500-U+259F is not surveyed: terminals synthesize that range against the cell box, so the font is
not in the path. Everything else has to be in every face on the panel.

`--face` asks the opposite question, of a font you might *recommend* rather than one a reader
already has: does it carry every character luxe can put on screen at the nerd tier, and does each
one stay inside its cell? The second half is the one a coverage count misses. The non-`Mono` cut of
any Nerd Font has every icon and draws most of them twice the cell wide, and plain Iosevka has every
character while `→ … —` print over their neighbor. The character set is read out of the TUI's
string literals (test code stripped) and out of what the site's assets and captures draw, so it
follows the code instead of a list somebody has to remember to update. It takes font *files*, not
family names, for the same reason the panel does.

Requires fontTools (`pip install fonttools`). Point PANEL entries at real files with FONT_DIRS or
the `LUXE_FONT_DIRS` environment variable (colon-separated).
"""

from __future__ import annotations

import argparse
import os
import pathlib
import re
import sys
import unicodedata

ROOT = pathlib.Path(__file__).resolve().parent.parent
PORTABLE = ROOT / "crates" / "luxe-tui" / "src" / "glyphs" / "portable.txt"

# Where `--face` reads what luxe draws from.
DRAWING_SOURCES = (ROOT / "crates" / "luxe-tui" / "src", ROOT / "crates" / "luxe-cli" / "src")
DRAWING_ASSETS = (
    ROOT / "web" / "src" / "assets" / "generated",
    ROOT / "web" / "src" / "assets" / "captures",
)

# Sources whose literals reach the terminal without ever reaching a font.
NOT_GLYPHS = {
    # kitty's Unicode placeholder and its row/column diacritics address an image; the terminal
    # replaces them with pixels
    ROOT / "crates" / "luxe-tui" / "src" / "graphics" / "kitty.rs",
}

# How far ink may pass the advance before it lands on the next cell. Outlines routinely overshoot
# by a few units for optical balance; the failures this exists for overshoot by 50 to 100%.
INK_TOLERANCE = 0.15

# The faces a reader plausibly already has. Filenames rather than family names: fontconfig
# substitutes silently, and a control run that quietly became the experimental run is exactly how
# the wordmark probe produced confident wrong answers the first time.
PANEL: dict[str, tuple[str, ...]] = {
    "Liberation Mono": ("LiberationMono-Regular.ttf",),
    "DejaVu Sans Mono": ("DejaVuSansMono.ttf",),
    "Noto Sans Mono": ("NotoSansMono-Regular.ttf", "NotoSansMono[wdth,wght].ttf"),
    "Cascadia Mono": ("CascadiaMono.ttf", "CascadiaMono-Regular.otf"),
    "Adwaita Mono": ("AdwaitaMono-Regular.ttf",),
    "Hack": ("Hack-Regular.ttf",),
    "Fira Code": ("FiraCode-Regular.ttf", "FiraCode[wght].ttf"),
    "JetBrains Mono": ("JetBrainsMono-Regular.ttf", "JetBrainsMono[wght].ttf"),
    "Source Code Pro": ("SourceCodePro-Regular.ttf", "SourceCodePro-Regular.otf"),
}

FONT_DIRS = [
    "/usr/share/fonts",
    "/usr/local/share/fonts",
    os.path.expanduser("~/.local/share/fonts"),
    os.path.expanduser("~/Library/Fonts"),
    "/Library/Fonts",
    "/System/Library/Fonts",
    "C:/Windows/Fonts",
]

# A wider palette to measure when looking for a substitute, so the choice is made from numbers.
CANDIDATES = (
    "·•…‹›«»†‡¶§¤¬°±×÷√∞≈≡≤≥∙–—←↑→↓"
    "□■▪▫▲▼◄►○●◆◇◉◌◦△▽"
    "✓✔✗✖★☆↳↺⊘⋯ℹ⦸❯❮▸▾‣⁃∴∵⌐‾"
)


def synthesized(cp: int) -> bool:
    """Whether the terminal draws this itself, which takes the font out of the path."""
    return 0x2500 <= cp <= 0x259F


def find_font(names: tuple[str, ...]) -> pathlib.Path | None:
    roots = [pathlib.Path(d) for d in os.environ.get("LUXE_FONT_DIRS", "").split(":") if d]
    roots += [pathlib.Path(d) for d in FONT_DIRS]
    for root in roots:
        if not root.is_dir():
            continue
        for name in names:
            hit = next(root.rglob(name), None)
            if hit:
                return hit
    return None


def coverage() -> dict[str, set[int]]:
    try:
        from fontTools.ttLib import TTFont
    except ImportError:
        sys.exit("glyph-survey needs fontTools: pip install fonttools")

    found: dict[str, set[int]] = {}
    missing: list[str] = []
    for face, names in PANEL.items():
        path = find_font(names)
        if path is None:
            missing.append(f"{face} ({' or '.join(names)})")
            continue
        font = TTFont(path, fontNumber=0, lazy=True)
        chars: set[int] = set()
        for table in font["cmap"].tables:
            chars |= set(table.cmap.keys())
        font.close()
        found[face] = chars
    if missing:
        sys.exit(
            "the panel is incomplete, so the survey would understate what is missing.\n"
            "  absent: " + "\n          ".join(missing) + "\n"
            "  install them, or point LUXE_FONT_DIRS at a directory that has them."
        )
    return found


def parse_portable() -> list[int]:
    listed = []
    for line in PORTABLE.read_text(encoding="utf-8").splitlines():
        match = re.match(r"^U\+([0-9A-F]{4,6})\t", line)
        if match:
            listed.append(int(match.group(1), 16))
    return listed


# === What luxe draws, for --face ===


def rust_literals(src: str) -> tuple[list[tuple[int, int]], str]:
    """Every string and char literal's span, and the source with literals and comments blanked.

    The blanked copy is what braces are matched in, so a `{` inside a format string or a comment
    cannot unbalance the item it sits in. A lexer and not a regex because Rust's lifetimes look
    like the start of a char literal and raw strings nest their own quotes.
    """
    spans: list[tuple[int, int]] = []
    masked = list(src)
    i, n = 0, len(src)

    def blank(start: int, end: int) -> None:
        for k in range(start, end):
            if masked[k] != "\n":
                masked[k] = " "

    while i < n:
        c = src[i]
        if src.startswith("//", i):
            end = src.find("\n", i)
            end = n if end < 0 else end
            blank(i, end)
            i = end
        elif src.startswith("/*", i):
            depth, j = 1, i + 2
            while j < n and depth:
                if src.startswith("/*", j):
                    depth, j = depth + 1, j + 2
                elif src.startswith("*/", j):
                    depth, j = depth - 1, j + 2
                else:
                    j += 1
            blank(i, j)
            i = j
        elif c == "r" and re.match(r'r#*"', src[i : i + 260]) and not identifier_tail(src, i):
            hashes = re.match(r"r(#*)", src[i:]).group(1)
            start = i + 2 + len(hashes)
            end = src.find('"' + hashes, start)
            end = n if end < 0 else end
            spans.append((start, end))
            blank(start, end)
            i = end + 1 + len(hashes)
        elif c == '"':
            j = i + 1
            while j < n and src[j] != '"':
                j += 2 if src[j] == "\\" else 1
            spans.append((i + 1, j))
            blank(i + 1, j)
            i = j + 1
        elif c == "'" and i + 2 < n and (src[i + 1] == "\\" or src[i + 2] == "'"):
            # an escape is two characters or more, so `'\''` cannot close on itself
            end = src.find("'", i + 3 if src[i + 1] == "\\" else i + 2)
            end = n if end < 0 else end
            spans.append((i + 1, end))
            blank(i + 1, end)
            i = end + 1
        else:
            i += 1
    return spans, "".join(masked)


def test_ranges(masked: str) -> tuple[list[tuple[int, int]], list[str]]:
    """The spans of `#[cfg(test)]` items, and the names of test modules kept in their own file."""
    ranges: list[tuple[int, int]] = []
    outlined: list[str] = []
    for match in re.finditer(r"#\[cfg\(test\)\]", masked):
        body = re.compile(r"[{;]").search(masked, match.end())
        if body is None:
            continue
        if body.group() == ";":
            declared = re.search(r"\bmod\s+(\w+)\s*$", masked[match.end() : body.start()])
            if declared:
                outlined.append(declared.group(1))
            continue
        depth, j = 0, body.start()
        while j < len(masked):
            if masked[j] == "{":
                depth += 1
            elif masked[j] == "}":
                depth -= 1
                if depth == 0:
                    break
            j += 1
        ranges.append((match.start(), j))
    return ranges, outlined


def source_codepoints() -> set[int]:
    """Non-ASCII characters in the TUI's string literals, outside test code.

    Errs toward including: a literal that never reaches the screen costs one extra row in the
    report, while a missed one is a character the recommended face was never asked about.
    """
    parsed = {}
    skipped: set[pathlib.Path] = set(NOT_GLYPHS)
    for path in (p for root in DRAWING_SOURCES for p in root.rglob("*.rs")):
        src = path.read_text(encoding="utf-8")
        spans, masked = rust_literals(src)
        ranges, outlined = test_ranges(masked)
        parsed[path] = (src, spans, ranges)
        # `foo.rs` keeps its children in `foo/`; `lib.rs`, `main.rs` and `mod.rs` beside themselves
        home = path.parent if path.stem in ("lib", "main", "mod") else path.parent / path.stem
        for name in outlined:
            skipped.update({home / f"{name}.rs", home / name})

    # a second pass, because a directory's files can be walked before the file declaring them tests
    used: set[int] = set()
    for path, (src, spans, ranges) in parsed.items():
        if any(path == s or s in path.parents for s in skipped):
            continue
        for start, end in spans:
            if any(lo <= start <= hi for lo, hi in ranges):
                continue
            literal = src[start:end]
            used.update(ord(c) for c in literal if ord(c) > 0x7F)
            used.update(int(h, 16) for h in re.findall(r"\\u\{([0-9A-Fa-f]{1,6})\}", literal))
    return {cp for cp in used if cp > 0x7F}


def asset_codepoints() -> set[int]:
    """Non-ASCII characters the site's assets and captures draw, as site-fonts.sh reads them."""
    import json

    used: set[str] = set()
    for directory in DRAWING_ASSETS:
        for path in directory.glob("*.svg"):
            body = re.sub(r"<[^>]+>", "", path.read_text(encoding="utf-8"))
            used.update(body.replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&"))
        for path in directory.glob("*.cast"):
            for line in path.read_text(encoding="utf-8").splitlines():
                try:
                    event = json.loads(line)
                except ValueError:
                    continue
                if isinstance(event, list) and len(event) == 3 and isinstance(event[2], str):
                    used.update(event[2])
    return {ord(c) for c in used if ord(c) > 0x7F}


def identifier_tail(src: str, i: int) -> bool:
    """Whether position `i` continues an identifier, which makes an `r` there not a raw string."""
    return i > 0 and (src[i - 1].isalnum() or src[i - 1] == "_")


def private_use(cp: int) -> bool:
    return 0xE000 <= cp <= 0xF8FF or 0xF0000 <= cp <= 0xFFFFD


def braille(cp: int) -> bool:
    return 0x2800 <= cp <= 0x28FF


def survey_faces(paths: list[str]) -> int:
    """Report each face against everything luxe draws. Non-zero if any misses or spills anything."""
    try:
        from fontTools.pens.boundsPen import BoundsPen
        from fontTools.ttLib import TTFont
    except ImportError:
        sys.exit("glyph-survey needs fontTools: pip install fonttools")

    drawn = source_codepoints() | asset_codepoints()
    classes = {
        "icons": [cp for cp in drawn if private_use(cp)],
        "box and block": [cp for cp in drawn if synthesized(cp)],
        "braille": [cp for cp in drawn if braille(cp)],
    }
    classes["marks"] = [cp for cp in drawn if not any(cp in group for group in classes.values())]
    print(
        f"luxe draws {len(drawn)} non-ASCII characters: "
        + ", ".join(f"{len(group)} {name}" for name, group in classes.items())
        + "\n",
        file=sys.stderr,
    )

    failed = False
    for raw in paths:
        path = pathlib.Path(raw)
        if not path.is_file():
            print(f"{raw}: no such file")
            failed = True
            continue
        font = TTFont(path, fontNumber=0)
        cmap = font.getBestCmap()
        glyphs = font.getGlyphSet()
        # a symbols-only face has no letters to take the cell from; its space is the next best thing
        cell_glyph = cmap.get(ord("M")) or cmap.get(ord(" "))
        if cell_glyph is None:
            print(f"{path.name}: no letter or space to measure a cell from")
            failed = True
            continue
        advance = font["hmtx"][cell_glyph][0]
        em = font["head"].unitsPerEm

        def spills(cp: int) -> bool:
            pen = BoundsPen(glyphs)
            glyphs[cmap[cp]].draw(pen)
            if pen.bounds is None:
                return False
            left, _, right, _ = pen.bounds
            return right > advance * (1 + INK_TOLERANCE) or left < -advance * INK_TOLERANCE

        missing = {name: [cp for cp in group if cp not in cmap] for name, group in classes.items()}
        spilled = sorted(cp for cp in drawn if cp in cmap and not synthesized(cp) and spills(cp))
        absent = sum(len(group) for group in missing.values())
        print(f"{path.name}")
        print(f"  has {len(drawn) - absent}/{len(drawn)}, cell {advance / em:.2f} em")
        for name, group in missing.items():
            if group:
                shown = "" if name == "icons" else "  " + " ".join(chr(cp) for cp in sorted(group))
                print(f"  missing {name}: {len(group)}{shown}")
        if spilled:
            icons = [cp for cp in spilled if private_use(cp)]
            text = [cp for cp in spilled if not private_use(cp)]
            if icons:
                print(f"  {len(icons)} icons spill into the next cell (the non-Mono cut?)")
            if text:
                print(f"  spills into the next cell: {' '.join(chr(cp) for cp in text)}")
        # braille is the one class allowed to come from another font, so it cannot fail a face
        if absent - len(missing["braille"]) or spilled:
            failed = True
        else:
            print("  draws everything luxe does, each inside its cell")
        font.close()
    return 1 if failed else 0


def emit(cps: list[int]) -> None:
    for cp in cps:
        try:
            name = unicodedata.name(chr(cp))
        except ValueError:
            name = "?"
        print(f"U+{cp:04X}\t{chr(cp)}\t{name}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="verify portable.txt against the panel")
    parser.add_argument("--candidates", action="store_true", help="survey the wider palette")
    parser.add_argument(
        "--face", nargs="+", metavar="FILE", help="measure font files against everything luxe draws"
    )
    args = parser.parse_args()

    if args.face:
        return survey_faces(args.face)

    cov = coverage()
    panel = list(cov)
    print(f"panel: {len(panel)} faces — {', '.join(panel)}\n", file=sys.stderr)

    if args.candidates:
        rows = []
        for ch in dict.fromkeys(CANDIDATES):
            if synthesized(ord(ch)):
                continue
            absent = [f for f in panel if ord(ch) not in cov[f]]
            rows.append((len(absent), ch, absent))
        for count, ch, absent in sorted(rows):
            verdict = "PORTABLE" if not count else f"missing {count}/{len(panel)}: {', '.join(absent)}"
            print(f"  {ch} U+{ord(ch):04X}  {verdict}")
        return 0

    listed = parse_portable()
    broken = {}
    for cp in listed:
        absent = [f for f in panel if cp not in cov[f]]
        if absent:
            broken[cp] = absent

    if args.check:
        if broken:
            print(f"{PORTABLE.relative_to(ROOT)} claims coverage the panel does not have:\n")
            for cp, absent in broken.items():
                print(f"  U+{cp:04X} {chr(cp)} — absent from {', '.join(absent)}")
            return 1
        print(f"portable.txt — {len(listed)} characters, present in all {len(panel)} faces")
        return 0

    print(f"# {len(listed)} surveyed characters, present in all {len(panel)} faces")
    emit(listed)
    if broken:
        print("\n# NOT portable, listed anyway:", file=sys.stderr)
        for cp, absent in broken.items():
            print(f"#   U+{cp:04X} {chr(cp)} — {', '.join(absent)}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
