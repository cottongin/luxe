#!/usr/bin/env python3
"""File-size ratchet for the Rust tree.

Smaller files are cheaper to read and grep, for people and for agents alike. Three limits:

  prod      500  lines of a source file outside its inline `#[cfg(test)] mod … { … }` blocks
  tests     150  lines of those inline test blocks; past it, move them to `foo/tests.rs`
  testfile  800  lines of a test-only file: under `tests/`, named `tests.rs`, `*_tests.rs` or
                 `test_support.rs`, declared as `#[cfg(test)] mod name;`, or a module of one of those

A file over a limit fails unless `scripts/size-allowlist.txt` lists it, and then it may not grow
past the number recorded there. The list only shrinks: an entry whose file is back under the limit,
or smaller than recorded, fails as stale until `--tighten` rewrites it. Give each entry a reason;
`baseline` marks one nobody has looked at yet.

  scripts/size-check.py             check
  scripts/size-check.py --tighten   lower recorded sizes to current, drop entries now under the limit
  scripts/size-check.py --report    the files over a limit, largest first
"""

import sys
from pathlib import Path

import rust_source

ROOT = Path(__file__).resolve().parent.parent
ALLOWLIST = ROOT / "scripts" / "size-allowlist.txt"
LIMITS = {"prod": 500, "tests": 150, "testfile": 800}


def measure(path: str) -> dict[str, int]:
    masked = rust_source.masked_source(path)
    total = masked.count("\n") + 1
    if path in rust_source.test_files():
        return {"testfile": total}
    inline = sum(last - first + 1 for first, last in rust_source.test_modules(masked))
    return {"prod": total - inline, "tests": inline} if inline else {"prod": total}


def current() -> dict[tuple[str, str], int]:
    sizes = {}
    for path in rust_source.rust_files():
        for kind, size in measure(path).items():
            sizes[(path, kind)] = size
    return sizes


def load_allowlist() -> dict[tuple[str, str], tuple[int, str]]:
    entries = {}
    if not ALLOWLIST.exists():
        return entries
    for raw in ALLOWLIST.read_text().splitlines():
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        body, _, reason = line.partition("#")
        size, kind, path = body.split()
        entries[(path, kind)] = (int(size), reason.strip())
    return entries


def write_allowlist(entries: dict[tuple[str, str], tuple[int, str]]) -> None:
    header = [
        "# Files allowed over a size limit, and the most they may grow to. See scripts/size-check.py.",
        "# <lines> <kind> <path>  # reason",
    ]
    rows = sorted(entries.items(), key=lambda item: (item[0][1], -item[1][0], item[0][0]))
    body = [f"{size:>5} {kind:<8} {path}  # {reason}" for (path, kind), (size, reason) in rows]
    ALLOWLIST.write_text("\n".join(header + body) + "\n")


def over(sizes: dict[tuple[str, str], int]) -> dict[tuple[str, str], int]:
    return {key: size for key, size in sizes.items() if size > LIMITS[key[1]]}


def check() -> int:
    sizes, allowed = current(), load_allowlist()
    problems = []
    for (path, kind), size in sorted(over(sizes).items()):
        limit = LIMITS[kind]
        if (path, kind) not in allowed:
            fix = "move the test module to a sibling tests.rs" if kind == "tests" else "split it by responsibility"
            problems.append(f"{path}: {size} {kind} lines, over {limit}: {fix}")
        elif size > allowed[(path, kind)][0]:
            problems.append(f"{path}: {size} {kind} lines, grew past its allowance of {allowed[(path, kind)][0]}")
    for (path, kind), (size, _) in sorted(allowed.items()):
        now = sizes.get((path, kind), 0)
        if now <= LIMITS[kind] or now < size:
            problems.append(f"{path}: allowlist says {size} {kind} lines, now {now}: run scripts/size-check.py --tighten")
    if problems:
        print("size check failed:")
        print("\n".join(f"  {problem}" for problem in problems))
        return 1
    print(f"size check: {len(sizes)} measurements, {len(allowed)} allowlisted, none over")
    return 0


def tighten() -> int:
    sizes, allowed = current(), load_allowlist()
    kept = {}
    for key, (size, reason) in allowed.items():
        now = sizes.get(key, 0)
        if now > LIMITS[key[1]]:
            kept[key] = (min(size, now), reason)
    write_allowlist(kept)
    print(f"allowlist: {len(allowed)} -> {len(kept)} entries")
    return 0


def report() -> int:
    for (path, kind), size in sorted(over(current()).items(), key=lambda item: -item[1]):
        print(f"{size:>5} {kind:<8} {path}")
    return 0


def freeze() -> int:
    """Seed the allowlist with everything over a limit today. Used once, when the ratchet starts."""
    write_allowlist({key: (size, "baseline") for key, size in over(current()).items()})
    return 0


if __name__ == "__main__":
    command = sys.argv[1] if len(sys.argv) > 1 else "--check"
    actions = {"--check": check, "--tighten": tighten, "--report": report, "--freeze": freeze}
    if command not in actions:
        print(__doc__)
        sys.exit(2)
    sys.exit(actions[command]())
