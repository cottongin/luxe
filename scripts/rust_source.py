"""Just enough Rust lexing for the size ratchets: what is code, what is test, where a block ends.

Not a parser. It masks comments and literals so braces and attributes can be found reliably, finds
inline `#[cfg(test)] mod … { … }` blocks by matching braces (production code after a test module
still counts), classifies files that only exist under a test module, and measures the bodies of
`#[async_trait]` methods, which clippy cannot see because the macro rewrites them.
"""

import re
import subprocess
from functools import lru_cache
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TEST_ATTR = re.compile(r"#\[cfg\(test\)\]")
ASYNC_TRAIT = re.compile(r"#\[(?:async_trait::)?async_trait(?:\([^)]*\))?\]")
MOD_DECL = re.compile(r"\bmod\s+(\w+)\s*([;{])")
FN_NAME = re.compile(r"\bfn\s+(\w+)")


def mask(source: str) -> str:
    """The source with comments blanked and literal contents replaced by `S`, newlines kept.

    Lines keep their length, so a line is code exactly when its masked form has a non-space
    character, and every brace left in the mask is a real one.
    """
    out, i, n = [], 0, len(source)
    while i < n:
        c = source[i]
        if source.startswith("//", i):
            end = source.find("\n", i)
            end = n if end < 0 else end
            out.append(" " * (end - i))
            i = end
        elif source.startswith("/*", i):
            depth, j = 1, i + 2
            while j < n and depth:
                if source.startswith("/*", j):
                    depth, j = depth + 1, j + 2
                elif source.startswith("*/", j):
                    depth, j = depth - 1, j + 2
                else:
                    j += 1
            out.append(_blank(source[i:j]))
            i = j
        elif (raw := re.match(r'b?r(#*)"', source[i : i + 260])) and not _ident_before(source, i):
            close = '"' + raw.group(1)
            j = source.find(close, i + raw.end())
            j = n if j < 0 else j + len(close)
            out.append(_literal(source[i:j]))
            i = j
        elif c == '"' or (c == "b" and source.startswith('b"', i) and not _ident_before(source, i)):
            j = i + (2 if c == "b" else 1)
            while j < n and source[j] != '"':
                j += 2 if source[j] == "\\" else 1
            out.append(_literal(source[i : j + 1]))
            i = j + 1
        elif c == "'" and (char := re.match(r"'(\\(x[0-9a-fA-F]{2}|u\{[0-9a-fA-F]+\}|.)|[^\\'\n])'", source[i : i + 14])):
            out.append(_literal(char.group(0)))
            i += char.end()
        else:
            out.append(c)
            i += 1
    return "".join(out)


def _ident_before(source: str, i: int) -> bool:
    return i > 0 and (source[i - 1].isalnum() or source[i - 1] == "_")


def _blank(text: str) -> str:
    return "".join("\n" if ch == "\n" else " " for ch in text)


def _literal(text: str) -> str:
    return "".join("\n" if ch == "\n" else "S" for ch in text)


def matching_brace(masked: str, open_at: int) -> int:
    """Index of the `}` closing the `{` at `open_at`, or the end of the text when it never closes."""
    depth = 0
    for k in range(open_at, len(masked)):
        if masked[k] == "{":
            depth += 1
        elif masked[k] == "}":
            depth -= 1
            if depth == 0:
                return k
    return len(masked) - 1


def line_of(masked: str, index: int) -> int:
    return masked.count("\n", 0, index)


def _after_attributes(masked: str, index: int) -> int:
    """Skip whitespace and further `#[…]` attributes after `index`."""
    while True:
        while index < len(masked) and masked[index].isspace():
            index += 1
        if not masked.startswith("#[", index):
            return index
        depth = 0
        for k in range(index, len(masked)):
            if masked[k] == "[":
                depth += 1
            elif masked[k] == "]":
                depth -= 1
                if depth == 0:
                    index = k + 1
                    break
        else:
            return len(masked)


def test_modules(masked: str) -> list[tuple[int, int]]:
    """Line spans `(first, last)` of inline `#[cfg(test)] mod name { … }` blocks."""
    spans = []
    for attr in TEST_ATTR.finditer(masked):
        start = _after_attributes(masked, attr.end())
        decl = re.match(r"(pub(\([^)]*\))?\s+)?mod\s+\w+\s*\{", masked[start:])
        if decl:
            open_at = start + decl.end() - 1
            spans.append((line_of(masked, attr.start()), line_of(masked, matching_brace(masked, open_at))))
    return spans


def out_of_line_test_modules(masked: str) -> list[str]:
    """Names of `#[cfg(test)] mod name;` declarations: files that only compile under test."""
    names = []
    for attr in TEST_ATTR.finditer(masked):
        start = _after_attributes(masked, attr.end())
        decl = re.match(r"(pub(\([^)]*\))?\s+)?mod\s+(\w+)\s*;", masked[start:])
        if decl:
            names.append(decl.group(3))
    return names


def child_paths(path: str, name: str) -> list[str]:
    """Where `mod name;` in `path` can live."""
    file = Path(path)
    roots = file.name in ("lib.rs", "main.rs", "mod.rs") or file.parent.name in ("tests", "examples", "benches")
    base = file.parent if roots else file.parent / file.stem
    return [str(base / f"{name}.rs"), str(base / name / "mod.rs")]


@lru_cache(maxsize=None)
def rust_files() -> tuple[str, ...]:
    listed = subprocess.run(
        ["git", "ls-files", "--cached", "--others", "--exclude-standard", "crates/*.rs"],
        cwd=ROOT, check=True, capture_output=True, text=True,
    ).stdout.split()
    return tuple(path for path in listed if (ROOT / path).exists())


@lru_cache(maxsize=None)
def masked_source(path: str) -> str:
    return mask((ROOT / path).read_text(encoding="utf-8"))


def named_as_test(path: str) -> bool:
    name = path.rsplit("/", 1)[-1]
    return "/tests/" in path or name in ("tests.rs", "test_support.rs") or name.endswith("_tests.rs")


@lru_cache(maxsize=None)
def test_files() -> frozenset[str]:
    """Every file that only compiles under test: named so, declared under `#[cfg(test)]`, or a
    module declared from inside one of those."""
    known = set(rust_files())
    tests = {path for path in known if named_as_test(path)}
    for path in known:
        for name in out_of_line_test_modules(masked_source(path)):
            tests.update(candidate for candidate in child_paths(path, name) if candidate in known)
    frontier = list(tests)
    while frontier:
        path = frontier.pop()
        for decl in MOD_DECL.finditer(masked_source(path)):
            if decl.group(2) != ";":
                continue
            for candidate in child_paths(path, decl.group(1)):
                if candidate in known and candidate not in tests:
                    tests.add(candidate)
                    frontier.append(candidate)
    return frozenset(tests)


def code_lines(masked: str, first: int, last: int) -> int:
    """Non-blank, non-comment lines in `first..=last`, which is how clippy counts a function."""
    lines = masked.split("\n")[first : last + 1]
    return sum(1 for line in lines if line.strip())


def async_trait_methods(path: str) -> list[tuple[str, int, int]]:
    """`(name, first_line, length)` for each method in an `#[async_trait]` impl outside tests."""
    masked = masked_source(path)
    in_tests = test_modules(masked)
    methods = []
    for attr in ASYNC_TRAIT.finditer(masked):
        start = _after_attributes(masked, attr.end())
        if not re.match(r"(unsafe\s+)?impl\b", masked[start:]):
            continue
        open_at = masked.find("{", start)
        close_at = matching_brace(masked, open_at)
        if any(first <= line_of(masked, open_at) <= last for first, last in in_tests):
            continue
        depth, k = 0, open_at
        while k < close_at:
            ch = masked[k]
            if ch == "{":
                depth += 1
            elif ch == "}":
                depth -= 1
            elif depth == 1 and (fn := FN_NAME.match(masked, k)) and (k == 0 or not masked[k - 1].isalnum()):
                body = _body_open(masked, fn.end(), close_at)
                if body is None:
                    k = fn.end()
                    continue
                body_end = matching_brace(masked, body)
                first, last = line_of(masked, body), line_of(masked, body_end)
                methods.append((fn.group(1), line_of(masked, k) + 1, code_lines(masked, first, last)))
                k = body_end + 1
                continue
            k += 1
    return methods


def _body_open(masked: str, after: int, limit: int) -> int | None:
    """The `{` opening a function body after its signature, or `None` for a declaration."""
    depth = 0
    for k in range(after, limit):
        ch = masked[k]
        if ch in "(<[":
            depth += 1
        elif ch in ")>]":
            depth -= 1
        elif ch == ";" and depth <= 0:
            return None
        elif ch == "{" and depth <= 0:
            return k
    return None
