#!/usr/bin/env python3
"""Check the user guide (`docs/guide/`) for what a machine can judge.

The guide is read in three places: on GitHub, on luxe.dev, and by an agent. So its links are written
the way a repository writes them, `configuration.md#github`, and the site rewrites them. This checks:

  frontmatter  every page has a title, a summary, an order and a known group
  links        every link to another page names a page that exists, and every anchor a heading on
               it; a site-style `/docs/...` link is refused, since it is dead on GitHub
  voice        on a rewritten page: no lands/arrives/invariant, and an em-dash budget

The config examples, flags and slash commands are checked against the code by the Rust test
`guide_check` in `luxe-cli`, which knows the real types.

    python3 scripts/docs-check.py
"""

from __future__ import annotations

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
GUIDE = ROOT / "docs" / "guide"

# Sidebar groups, in order. `web/src/content.config.ts` holds the same list.
GROUPS = ["Get started", "Using Luxe", "Customize", "Extend", "Reference"]

# Pages exempt from the voice rules while a rewrite is in progress. Empty: every page follows them.
# Links and frontmatter apply to every page regardless.
PENDING: set[str] = set()

# Words that read as generated, in the guide's prose. Matched as whole words, case-insensitively.
BANNED = ["land", "lands", "landed", "arrive", "arrives", "arrived", "invariant", "invariants"]

# At most one em dash per this many words of prose, and never fewer than this many allowed.
DASH_WORDS = 250
DASH_FLOOR = 2


def split_frontmatter(text: str) -> tuple[dict[str, str], str]:
    if not text.startswith("---\n"):
        return {}, text
    end = text.index("\n---\n", 4)
    fields = {}
    for line in text[4:end].splitlines():
        key, _, value = line.partition(":")
        fields[key.strip()] = value.strip().strip('"')
    return fields, text[end + 5 :]


def without_code(text: str) -> str:
    """The page with fenced and inline code blanked, line count preserved."""
    text = re.sub(r"```.*?```", lambda m: "\n" * m.group(0).count("\n"), text, flags=re.S)
    return re.sub(r"`[^`\n]*`", "", text)


def slug(heading: str, seen: dict[str, int]) -> str:
    """The anchor github-slugger, which Astro uses, gives a heading."""
    text = re.sub(r"`([^`]*)`", r"\1", heading)
    text = re.sub(r"\[([^\]]*)\]\([^)]*\)", r"\1", text)
    text = re.sub(r"[*_]", "", text).strip().lower()
    text = re.sub(r"[^\w\- ]", "", text)
    base = text.replace(" ", "-")
    count = seen.get(base, 0)
    seen[base] = count + 1
    return base if count == 0 else f"{base}-{count}"


def anchors(body: str) -> set[str]:
    seen: dict[str, int] = {}
    found = set()
    in_code = False
    for line in body.splitlines():
        if line.startswith("```"):
            in_code = not in_code
            continue
        heading = re.match(r"#{1,6} (.+)", line)
        if heading and not in_code:
            found.add(slug(heading.group(1), seen))
    return found


def main() -> int:
    pages = {path.stem: path for path in sorted(GUIDE.glob("*.md"))}
    parsed = {name: split_frontmatter(path.read_text(encoding="utf-8")) for name, path in pages.items()}
    page_anchors = {name: anchors(body) for name, (_, body) in parsed.items()}
    problems: list[str] = []

    for name in sorted(PENDING - pages.keys()):
        problems.append(f"scripts/docs-check.py: PENDING names {name}, which is not a page")

    for name, (fields, body) in parsed.items():
        where = f"docs/guide/{name}.md"
        for key in ("title", "summary", "order", "group"):
            if not fields.get(key):
                problems.append(f"{where}: frontmatter has no {key}")
        if fields.get("group") and fields["group"] not in GROUPS:
            problems.append(f"{where}: group {fields['group']!r} is not one of {GROUPS}")

        prose = without_code(body)
        for number, line in enumerate(prose.splitlines(), start=1):
            for target in re.findall(r"\]\(([^)\s]+)\)", line):
                problem = check_link(target, name, pages, page_anchors)
                if problem:
                    problems.append(f"{where}:{number + offset(fields)}  {problem}: {target}")

        if name in PENDING:
            continue
        for number, line in enumerate(prose.splitlines(), start=1):
            for word in BANNED:
                if re.search(rf"\b{word}\b", line, re.IGNORECASE):
                    problems.append(f"{where}:{number + offset(fields)}  {word!r}: {line.strip()[:80]}")
        words = len(prose.split())
        dashes = prose.count("\u2014")
        budget = max(DASH_FLOOR, words // DASH_WORDS)
        if dashes > budget:
            problems.append(f"{where}: {dashes} em dashes in {words} words; the budget is {budget}")

    if problems:
        print(f"docs check: {len(problems)} problem(s)")
        for problem in problems:
            print(f"  {problem}")
        return 1
    rewritten = len(pages) - len(PENDING & pages.keys())
    print(f"docs check: {len(pages)} pages, {rewritten} rewritten, links and frontmatter clean")
    return 0


def offset(fields: dict[str, str]) -> int:
    """Lines the frontmatter takes, so a line number points into the file."""
    return len(fields) + 2 if fields else 0


def check_link(
    target: str, page: str, pages: dict[str, pathlib.Path], page_anchors: dict[str, set[str]]
) -> str | None:
    if re.match(r"[a-z]+:", target):
        return None
    if target.startswith("/docs"):
        return "a site route, which is dead on GitHub; link the file, like `install.md`"
    if target.startswith("/"):
        return None
    file, _, anchor = target.partition("#")
    if file.removeprefix("./") in pages:
        return "a page without its file name; link the file, like `install.md`"
    if file.startswith("./"):
        return "a `./` prefix, which the site does not rewrite; link the file, like `install.md`"
    if file and not file.endswith(".md"):
        return None
    name = file.removesuffix(".md") if file else page
    if name not in pages:
        return "no such page"
    if anchor and anchor not in page_anchors[name]:
        return "no such heading"
    return None


if __name__ == "__main__":
    sys.exit(main())
