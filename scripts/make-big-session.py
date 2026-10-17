#!/usr/bin/env python3
"""Build a long session out of a real one, so frame cost can be measured against length.

A perf claim about the TUI is only worth what it was measured on, and the interesting sizes are the
ones nobody has a handy session for. This repeats a real transcript end to end with fresh entry ids
and a correct parent chain, which `luxe --resume` then loads like any other.

    scripts/make-big-session.py .luxe/sessions/<project>/<id>.jsonl 40 <new-id>
    scripts/tui-latency.py --resume <new-id>
"""

from __future__ import annotations

import json
import pathlib
import sys
import uuid


def main() -> int:
    if len(sys.argv) != 4:
        print(__doc__, file=sys.stderr)
        return 2
    source = pathlib.Path(sys.argv[1])
    repeats = int(sys.argv[2])
    destination = source.parent / f"{sys.argv[3]}.jsonl"

    entries = [json.loads(line) for line in source.read_text().splitlines() if line.strip()]
    if not entries:
        print(f"{source} holds no entries", file=sys.stderr)
        return 1

    header = dict(entries[0], id=str(uuid.uuid4()), parent_id=None)
    out = [header]
    parent = header["id"]
    for _ in range(repeats):
        for entry in entries[1:]:
            copy = json.loads(json.dumps(entry))
            copy["id"] = str(uuid.uuid4())
            copy["parent_id"] = parent
            parent = copy["id"]
            out.append(copy)

    destination.write_text("".join(json.dumps(entry) + "\n" for entry in out))
    print(f"{destination} — {len(out)} entries, {destination.stat().st_size / 1024:.0f} KiB")
    return 0


if __name__ == "__main__":
    sys.exit(main())
