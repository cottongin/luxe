#!/usr/bin/env python3
"""Function-length ratchet: clippy's `too_many_lines` at the threshold in `clippy.toml` (80).

Runs clippy over the shipped code (`--lib --bins`, so tests and examples are out) with only that
lint on, in its own target directory so it never invalidates the gate's clippy cache. A function
over the threshold fails unless `scripts/fn-length-allowlist.txt` lists it, and then it may not grow
past the number recorded there. The list only shrinks: an entry whose function is back under the
threshold, or shorter than recorded, fails as stale until `--tighten` rewrites it.

`#[async_trait]` rewrites a method's body inside a macro expansion, and clippy does not lint what a
macro produced, so those methods (every `Tool::run`, among others) are measured here instead by
`rust_source`, with the same rule: non-blank, non-comment lines of the body.

Functions are keyed by file and name, since line numbers move. Same-named functions in one file
(two `deserialize` impls, say) share a key and are compared largest to largest.

  scripts/fn-length-check.py             check
  scripts/fn-length-check.py --tighten   lower recorded lengths to current, drop what is now short
  scripts/fn-length-check.py --report    every function over the threshold, longest first
"""

import json
import re
import subprocess
import sys
from collections import defaultdict
from pathlib import Path

import rust_source

ROOT = Path(__file__).resolve().parent.parent
ALLOWLIST = ROOT / "scripts" / "fn-length-allowlist.txt"
LENGTH = re.compile(r"too many lines \((\d+)/(\d+)\)")
NAME = re.compile(r"\bfn\s+(\w+)")


def measure() -> dict[tuple[str, str], list[int]]:
    command = [
        "cargo", "clippy", "--workspace", "--lib", "--bins", "--quiet",
        "--message-format=json", "--target-dir", str(ROOT / "target" / "fn-length"),
        "--", "-A", "clippy::all", "-W", "clippy::too_many_lines",
    ]
    run = subprocess.run(command, cwd=ROOT, capture_output=True, text=True)
    if run.returncode != 0:
        sys.stderr.write(run.stderr)
        sys.exit(f"clippy failed ({run.returncode})")
    lengths = defaultdict(list)
    seen = set()
    for line in run.stdout.splitlines():
        message = json.loads(line).get("message") if line.startswith("{") else None
        if not message or (message.get("code") or {}).get("code") != "clippy::too_many_lines":
            continue
        span = message["spans"][0]
        name = NAME.search(" ".join(text["text"] for text in span["text"]))
        length = LENGTH.search(message["message"])
        site = (span["file_name"], span["line_start"])
        if not (name and length) or site in seen:
            continue  # a bin and a lib can both report the same function
        seen.add(site)
        lengths[(span["file_name"], name.group(1))].append(int(length.group(1)))
    threshold = int(re.search(r"too-many-lines-threshold\s*=\s*(\d+)", (ROOT / "clippy.toml").read_text()).group(1))
    for path in rust_source.rust_files():
        if path in rust_source.test_files() or "/examples/" in path:
            continue
        for name, line, length in rust_source.async_trait_methods(path):
            if length > threshold and (path, line) not in seen:
                lengths[(path, name)].append(length)
    return {key: sorted(values, reverse=True) for key, values in lengths.items()}


def load_allowlist() -> dict[tuple[str, str], tuple[list[int], str]]:
    entries = {}
    if not ALLOWLIST.exists():
        return entries
    for raw in ALLOWLIST.read_text().splitlines():
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        body, _, reason = line.partition("#")
        sizes, path, name = body.split()
        entries[(path, name)] = ([int(size) for size in sizes.split(",")], reason.strip())
    return entries


def write_allowlist(entries: dict[tuple[str, str], tuple[list[int], str]]) -> None:
    header = [
        "# Functions allowed over clippy's too_many_lines threshold. See scripts/fn-length-check.py.",
        "# <lines[,lines…]> <path> <fn>  # reason",
    ]
    rows = sorted(entries.items(), key=lambda item: (-item[1][0][0], item[0]))
    body = [
        f"{','.join(map(str, sizes)):>7} {path} {name}  # {reason}"
        for (path, name), (sizes, reason) in rows
    ]
    ALLOWLIST.write_text("\n".join(header + body) + "\n")


def check() -> int:
    measured, allowed = measure(), load_allowlist()
    problems = []
    for key, sizes in sorted(measured.items()):
        where = f"{key[0]}: fn {key[1]}"
        if key not in allowed:
            problems.append(f"{where} is {sizes[0]} lines, over the threshold: split it")
            continue
        limit = allowed[key][0]
        if len(sizes) > len(limit) or any(now > cap for now, cap in zip(sizes, limit)):
            problems.append(f"{where} is {sizes} lines, grew past its allowance of {limit}")
    for key, (sizes, _) in sorted(allowed.items()):
        now = measured.get(key, [])
        if len(now) < len(sizes) or any(n < s for n, s in zip(now, sizes)):
            problems.append(f"{key[0]}: fn {key[1]} allowlisted at {sizes}, now {now}: run --tighten")
    if problems:
        print("function-length check failed:")
        print("\n".join(f"  {problem}" for problem in problems))
        return 1
    print(f"function-length check: {sum(map(len, measured.values()))} over the threshold, all allowlisted")
    return 0


def tighten() -> int:
    measured, allowed = measure(), load_allowlist()
    kept = {}
    for key, (sizes, reason) in allowed.items():
        now = measured.get(key, [])
        if now:
            kept[key] = ([min(n, s) for n, s in zip(now, sizes)] + now[len(sizes):], reason)
    write_allowlist(kept)
    print(f"allowlist: {len(allowed)} -> {len(kept)} entries")
    return 0


def report() -> int:
    flat = [(size, path, name) for (path, name), sizes in measure().items() for size in sizes]
    for size, path, name in sorted(flat, reverse=True):
        print(f"{size:>5} {path} {name}")
    return 0


def freeze() -> int:
    """Seed the allowlist with everything over the threshold today. Used once."""
    write_allowlist({key: (sizes, "baseline") for key, sizes in measure().items()})
    return 0


if __name__ == "__main__":
    command = sys.argv[1] if len(sys.argv) > 1 else "--check"
    actions = {"--check": check, "--tighten": tighten, "--report": report, "--freeze": freeze}
    if command not in actions:
        print(__doc__)
        sys.exit(2)
    sys.exit(actions[command]())
