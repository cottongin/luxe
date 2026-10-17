#!/usr/bin/env python3
"""Check shipped prose against `docs/brand/voice.md`.

Taste that cannot be asserted drifts. This asserts the one part of the voice that a machine can
actually judge: the words the rule bans outright. Everything else in that document — evidence with
claims, naming the trade-off, sentence variance — is a human's call, and pretending otherwise would
produce a checker nobody trusts.

Two severities, because they are different promises:

  banned   marketing adjectives with no defensible use. Exits non-zero.
  review   words that are usually filler but sometimes correct English ("just above the prompt").
           Reported, never fatal.

Code is not prose: fenced blocks and inline spans are skipped, so a banned word inside a command is
a command. `docs/brand/voice.md` is skipped entirely — it quotes every one of these on purpose.

    python3 scripts/voice-check.py            # everything shipped
    python3 scripts/voice-check.py --review   # include the judgement calls
"""

from __future__ import annotations

import argparse
import pathlib
import re
import subprocess

ROOT = pathlib.Path(__file__).resolve().parent.parent

# Lowercase, matched case-insensitively. Proper nouns that collide (Delve, the Go debugger) are
# handled by CASE_SENSITIVE below rather than by dropping the word.
BANNED = [
    "leverage",
    "delve",
    "robust",
    "seamless",
    "seamlessly",
    "intricate",
    "pivotal",
    "realm",
    "effortless",
    "effortlessly",
    "magical",
    "blazingly",
    "revolutionary",
    "game-changing",
    "supercharge",
    "supercharged",
    "next-generation",
    "unleash",
    "elevate",
    "cutting-edge",
    "best-in-class",
    "world-class",
    "empower",
    "utilize",
    "plethora",
    "myriad",
]

# Words whose *capitalised* form is a name, not an adjective. Only the lowercase use is a finding.
CASE_SENSITIVE = {"delve"}

# Usually filler, sometimes the right word. "Just above the prompt" is a position; "just run it" is
# a shrug at someone's difficulty.
REVIEW = ["simply", "just", "easily", "obviously", "basically"]

# British spellings, and the American ones this project writes.
#
# These are stems, not words: the "colour" entry also catches colours/coloured/colouring. They get a
# wider net than the banned words above, because a reader meets this project's prose in doc comments
# and identifiers as much as on the website, and half-converted spelling reads worse than either
# convention held consistently. None is a legitimate identifier here, so a hit is always a lapse.
#
# The keys are spellings this project does NOT use, which makes this table the one place in the repo
# that must keep them — hence the skip entry for this file below, and the self-check in `spelling()`.
BRITISH = {
    "colour": "color",
    "behaviour": "behavior",
    "neighbour": "neighbor",
    "honour": "honor",
    "favour": "favor",
    "labour": "labor",
    "humour": "humor",
    "rumour": "rumor",
    "labelled": "labeled",
    "labelling": "labeling",
    "travelling": "traveling",
    "emphasise": "emphasize",
    "recognise": "recognize",
    "serialise": "serialize",
    "initialise": "initialize",
    "organise": "organize",
    "realise": "realize",
    "analyse": "analyze",
    "prioritise": "prioritize",
    "summarise": "summarize",
    "normalise": "normalize",
    "customise": "customize",
    "minimise": "minimize",
    "maximise": "maximize",
    "specialise": "specialize",
    "synchronise": "synchronize",
    "centre": "center",
    "theatre": "theater",
    "licence": "license",
    "defence": "defense",
    "grey": "gray",
    "storey": "story",
    "catalogue": "catalog",
    "artefact": "artifact",
    "sceptical": "skeptical",
    "calibre": "caliber",
    "fibre": "fiber",
    "lustre": "luster",
    "whilst": "while",
    "amongst": "among",
    "learnt": "learned",
    "spelt": "spelled",
}

# Text we did not write, plus this file — which contains every banned spelling by definition, the
# same reason `docs/brand/voice.md` is skipped above. Lockfiles are resolver output and can name a
# real package `@img/color`; upstream license grants keep whatever spelling they shipped with. The
# GitHub fixtures are GitHub's own answers, saved verbatim, with strangers' titles in them.
SPELLING_SKIP = (
    ".pi/",
    "crates/luxe-core/src/github/fixtures/",
    "web/public/fonts/",
    "web/src/assets/generated/",
    "node_modules/",
    "scripts/voice-check.py",
)
SPELLING_SKIP_EXT = (
    ".lock",
    "-lock.json",
    ".png",
    ".jpg",
    ".jpeg",
    ".svg",
    ".cast",
    ".woff2",
    ".woff",
    ".ico",
    ".webp",
)

# Prose that ships. The planning docs and the journal are development-only and write for a different
# reader; holding them to the product's voice would be a category error.
INCLUDE = ["README.md", "docs/guide/*.md", "web/src/**/*.astro", "web/src/**/*.svelte"]
SKIP = {"docs/brand/voice.md"}


def prose(text: str) -> str:
    """The document with code and quoted words removed, line count preserved.

    Two things are not the author using a word. **Code** — a banned word inside a command is a
    command. And a word set in **emphasis on its own**, which is how both the voice document and the
    page describing it point at the words to avoid: those are mentioning the word, not using it.
    """

    def blank(match: re.Match[str]) -> str:
        return "\n" * match.group(0).count("\n")

    text = re.sub(r"```.*?```", blank, text, flags=re.S)
    text = re.sub(r"`[^`\n]*`", "", text)
    return re.sub(r"(?<!\*)\*([A-Za-z][A-Za-z -]{0,24})\*(?!\*)", "", text)


def findings(
    path: pathlib.Path, words: list[str], case_sensitive: set[str]
) -> list[str]:
    text = prose(path.read_text(encoding="utf-8", errors="replace"))
    lines = text.split("\n")
    found = []
    for word in words:
        flags = 0 if word in case_sensitive else re.IGNORECASE
        for match in re.finditer(rf"\b{re.escape(word)}\b", text, flags):
            number = text[: match.start()].count("\n")
            found.append(
                f"{path.relative_to(ROOT)}:{number + 1}  {word} \u2014 {lines[number].strip()[:88]}"
            )
    return found


def spelling() -> list[str]:
    """Every British spelling in text this project actually wrote.

    Deliberately a raw substring scan over the whole tree rather than the prose-only extraction the
    banned words get: an identifier spelled `honors_ignore_files` is as much a lapse as a sentence
    that says it, and none of these stems has a defensible use here.
    """
    # A blanket search-and-replace over the repo will happily rewrite the table above into
    # `{"color": "color", ...}`, after which every correct word matches and the checker condemns the
    # very spelling it exists to enforce. Ask for that mistake out loud rather than acting on it.
    flattened = sorted(k for k, v in BRITISH.items() if k == v)
    if flattened:
        raise SystemExit(
            f"voice-check: BRITISH maps {flattened[0]!r} to itself — the table was rewritten by a "
            "spelling pass and no longer distinguishes the two conventions."
        )

    listing = subprocess.run(
        ["git", "ls-files", "--cached", "--others", "--exclude-standard"],
        capture_output=True,
        text=True,
        cwd=ROOT,
    ).stdout.split()
    pattern = re.compile(
        "|".join(sorted(BRITISH, key=len, reverse=True)), re.IGNORECASE
    )

    found = []
    for name in listing:
        if name.startswith(SPELLING_SKIP) or name.endswith(SPELLING_SKIP_EXT):
            continue
        try:
            text = (ROOT / name).read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError):
            continue
        for match in pattern.finditer(text):
            number = text[: match.start()].count("\n")
            word = match.group(0)
            line = text.split("\n")[number].strip()[:76]
            found.append(
                f"{name}:{number + 1}  {word} \u2192 {BRITISH[word.lower()]} \u2014 {line}"
            )
    return found


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--review", action="store_true", help="also report the judgement calls"
    )
    args = parser.parse_args()

    paths = sorted(
        {
            p
            for pattern in INCLUDE
            for p in ROOT.glob(pattern)
            if str(p.relative_to(ROOT)) not in SKIP
        }
    )

    banned = [f for p in paths for f in findings(p, BANNED, CASE_SENSITIVE)]
    print(f"voice check \u2014 {len(paths)} files")

    if banned:
        print(f"\n{len(banned)} banned word(s):")
        for line in banned:
            print(f"  {line}")
    else:
        print("  no banned words")

    british = spelling()
    if british:
        print(f"\n{len(british)} British spelling(s):")
        for line in british:
            print(f"  {line}")
    else:
        print("  no British spellings")

    if args.review:
        soft = [f for p in paths for f in findings(p, REVIEW, set())]
        print(
            f"\n{len(soft)} for review (never fatal \u2014 most are correct English):"
        )
        for line in soft:
            print(f"  {line}")

    return 1 if banned or british else 0


if __name__ == "__main__":
    raise SystemExit(main())
