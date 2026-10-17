#!/usr/bin/env python3
"""Comment report: where shipped Rust source spends its comment lines, and which blocks to read.

Not a gate. The ratio of comment to code says little on its own (well-documented libraries run from
25% to over 100%); what matters is whether a block earns its length. So this lists the blocks most
likely not to: long ones, and lines that narrate history ("used to", "measured on a live run")
rather than state why the code is as it is.

  scripts/comment-report.py              totals, then the long blocks and the narration, worst first
  scripts/comment-report.py --totals     totals only
"""

import re
import sys

import rust_source

HISTORY = re.compile(
    r"\b(used to|no longer|which is how|this used|once (?:shipped|was|did)|measured on|it shipped|"
    r"was the bug|before this|until (?:this|now)|on a live|a real (?:session|run|loop))\b",
    re.I,
)
LONG = 15


def blocks(path: str):
    """`(first_line, lines, text)` for each run of comment lines outside test modules."""
    source = (rust_source.ROOT / path).read_text(encoding="utf-8").split("\n")
    masked = rust_source.masked_source(path)
    tests = {line for first, last in rust_source.test_modules(masked) for line in range(first, last + 1)}
    at = 0
    while at < len(source):
        if at not in tests and source[at].strip().startswith("//"):
            end = at
            while end < len(source) and end not in tests and source[end].strip().startswith("//"):
                end += 1
            yield at + 1, end - at, [line.strip() for line in source[at:end]]
            at = end
        else:
            at += 1


def main() -> int:
    shipped = [
        path for path in rust_source.rust_files()
        if path not in rust_source.test_files() and "/examples/" not in path
    ]
    doc = inline = code = 0
    long_blocks, narration = [], []
    for path in shipped:
        masked = rust_source.masked_source(path).split("\n")
        source = (rust_source.ROOT / path).read_text(encoding="utf-8").split("\n")
        tests = {
            line
            for first, last in rust_source.test_modules(rust_source.masked_source(path))
            for line in range(first, last + 1)
        }
        for index, (line, mask) in enumerate(zip(source, masked)):
            stripped = line.strip()
            if index in tests or not stripped:
                continue
            if stripped.startswith(("///", "//!")):
                doc += 1
            elif stripped.startswith("//"):
                inline += 1
            elif mask.strip():
                code += 1
        for first, length, text in blocks(path):
            if length >= LONG:
                long_blocks.append((length, f"{path}:{first}"))
            narration += [(f"{path}:{first + n}", t) for n, t in enumerate(text) if HISTORY.search(t)]
    print(f"code {code}, doc {doc} ({100 * doc / code:.1f}%), inline {inline} "
          f"({100 * inline / code:.1f}%), together {100 * (doc + inline) / code:.1f}%")
    print(f"blocks of {LONG}+ lines: {len(long_blocks)} holding {sum(n for n, _ in long_blocks)} "
          f"lines; history narration: {len(narration)} lines")
    if "--totals" in sys.argv:
        return 0
    print(f"\n# blocks of {LONG}+ lines, longest first")
    for length, where in sorted(long_blocks, reverse=True):
        print(f"{length:4}  {where}")
    print("\n# lines narrating history")
    for where, text in narration:
        print(f"{where}  {text[:110]}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
