#!/usr/bin/env python3
"""Type at the real TUI through a pty and measure how far behind the input it falls.

Frame cost is easy to reason about wrongly and easy to measure directly. This spawns `luxe` on a
pty, waits for the first frame to settle, then types at a fixed rate and records what the process
did about it: how soon each key was answered, how much it wrote, how much CPU it burned, and — the
number that actually describes a freeze — how long it kept catching up after the typing stopped.

    scripts/tui-latency.py --resume <session-id> --keys 240 --rate 80

Pair it with `LUXE_FRAME_PROFILE`, which the binary reads to dump per-frame timings:

    scripts/tui-latency.py --resume <id> --env LUXE_FRAME_PROFILE=/tmp/frame.csv

`scripts/make-big-session.py` builds a long session to point it at.
"""

from __future__ import annotations

import argparse
import fcntl
import os
import pty
import select
import signal
import struct
import sys
import termios
import time

# Startup probes crossterm and the graphics layer send that block until something answers. A pty is
# not a terminal, so the harness plays one well enough to get past them.
PROBES: tuple[tuple[bytes, bytes], ...] = (
    (b"\x1b[6n", b"\x1b[1;1R"),  # cursor position
    (b"\x1b[c", b"\x1b[?62;c"),  # primary device attributes
    (b"\x1b[?u", b"\x1b[?0u"),  # kitty keyboard protocol
)


def set_winsize(fd: int, rows: int, cols: int) -> None:
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))


def answer_probes(fd: int, chunk: bytes) -> None:
    for probe, reply in PROBES:
        for _ in range(chunk.count(probe)):
            os.write(fd, reply)


def drain(fd: int, quiet_for: float, limit: float) -> tuple[int, float]:
    """Read until the child has been silent for `quiet_for`. Returns (bytes, seconds)."""
    started = time.monotonic()
    last = started
    total = 0
    while True:
        now = time.monotonic()
        if now - last >= quiet_for:
            return total, last - started
        if now - started >= limit:
            return total, now - started
        if not select.select([fd], [], [], quiet_for)[0]:
            continue
        try:
            chunk = os.read(fd, 1 << 16)
        except OSError:
            return total, time.monotonic() - started
        if not chunk:
            return total, time.monotonic() - started
        answer_probes(fd, chunk)
        total += len(chunk)
        last = time.monotonic()


def read_for(fd: int, seconds: float) -> None:
    """Read and discard for `seconds`, answering probes, so the child never blocks on its output."""
    until = time.monotonic() + seconds
    while (remaining := until - time.monotonic()) > 0:
        if not select.select([fd], [], [], remaining)[0]:
            continue
        try:
            chunk = os.read(fd, 1 << 16)
        except OSError:
            return
        if not chunk:
            return
        answer_probes(fd, chunk)


def cpu_of(pid: int) -> float:
    """User + system CPU seconds the process has burned."""
    with open(f"/proc/{pid}/stat", "rb") as handle:
        fields = handle.read().rsplit(b")", 1)[1].split()
    return (int(fields[11]) + int(fields[12])) / os.sysconf("SC_CLK_TCK")


def rss_of(pid: int) -> int:
    """Resident set size, in KiB."""
    with open(f"/proc/{pid}/status", "rb") as handle:
        for line in handle:
            if line.startswith(b"VmRSS:"):
                return int(line.split()[1])
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", default="target/release/luxe")
    parser.add_argument("--resume", help="session id to resume, so the transcript has some length")
    parser.add_argument("--rows", type=int, default=50)
    parser.add_argument("--cols", type=int, default=200)
    parser.add_argument("--keys", type=int, default=240)
    parser.add_argument("--rate", type=float, default=80.0, help="keystrokes per second")
    parser.add_argument("--settle", type=float, default=10.0, help="seconds to let startup finish")
    parser.add_argument("--char", default="x", help="the character to type")
    parser.add_argument(
        "--prompt",
        help="submit this first and measure while the turn streams, which is the harder case: "
        "the reveal drives the loop at ~80fps and the tail relays out on every one of them",
    )
    parser.add_argument(
        "--warmup", type=float, default=8.0, help="seconds to let a --prompt turn get going"
    )
    parser.add_argument("--env", action="append", default=[], help="NAME=VALUE for the child")
    args = parser.parse_args()

    argv = [args.binary] + (["--resume", args.resume] if args.resume else [])
    env = dict(os.environ, TERM="xterm-256color")
    for pair in args.env:
        name, _, value = pair.partition("=")
        env[name] = value

    pid, fd = pty.fork()
    if pid == 0:
        os.execvpe(argv[0], argv, env)
        os._exit(127)

    set_winsize(fd, args.rows, args.cols)
    try:
        startup, _ = drain(fd, quiet_for=0.5, limit=args.settle)
        print(f"startup: {startup / 1024:.0f} KiB, cpu {cpu_of(pid):.2f}s, rss {rss_of(pid) / 1024:.0f} MiB")

        if args.prompt:
            os.write(fd, args.prompt.encode() + b"\r")
            # Let the request go out and the first tokens land, so the burst below falls in the
            # middle of a stream rather than before one.
            read_for(fd, args.warmup)
            print(f"prompt submitted, {args.warmup:g}s ago; measuring while it streams")

        interval = 1.0 / args.rate
        payload = args.char.encode()
        cpu_before, rss_before = cpu_of(pid), rss_of(pid)
        written = 0
        latencies: list[float] = []
        unanswered = 0

        start = time.monotonic()
        for index in range(args.keys):
            deadline = start + index * interval
            if (slack := deadline - time.monotonic()) > 0:
                time.sleep(slack)
            sent = time.monotonic()
            os.write(fd, payload)
            # Read whatever comes back inside this keystroke's slot; the first byte of the answer
            # is the frame that belongs to it.
            first = None
            while (remaining := deadline + interval - time.monotonic()) > 0:
                if not select.select([fd], [], [], remaining)[0]:
                    break
                chunk = os.read(fd, 1 << 16)
                if not chunk:
                    break
                answer_probes(fd, chunk)
                first = first if first is not None else time.monotonic() - sent
                written += len(chunk)
            if first is None:
                unanswered += 1
            else:
                latencies.append(first)

        typing = time.monotonic() - start
        cpu_after = cpu_of(pid)
        tail_bytes, tail_seconds = drain(fd, quiet_for=0.25, limit=10.0)
        latencies.sort()

        def percentile(fraction: float) -> float:
            if not latencies:
                return float("nan")
            return latencies[min(int(len(latencies) * fraction), len(latencies) - 1)] * 1000

        print()
        print(f"typed {args.keys} keys at {args.rate:g}/s over {typing:.2f}s")
        print(f"  keys with no frame inside their slot : {unanswered}")
        print(
            f"  first-byte latency  p50 {percentile(0.5):6.1f} ms  "
            f"p95 {percentile(0.95):6.1f} ms  max {percentile(1.0):6.1f} ms"
        )
        print(
            f"  bytes written while typing           : {written / 1024:.0f} KiB "
            f"({written / max(typing, 1e-9) / 1024:.0f} KiB/s)"
        )
        # With a turn in flight this is the stream, not a backlog; `unanswered` and the latency
        # percentiles above are what say whether the loop actually fell behind.
        label = "output after the last key" if args.prompt else "still catching up after the last key"
        print(f"  {label:<36} : {tail_seconds * 1000:.0f} ms, {tail_bytes / 1024:.0f} KiB")
        print(
            f"  cpu burned while typing              : {cpu_after - cpu_before:.2f}s "
            f"({(cpu_after - cpu_before) / max(typing, 1e-9) * 100:.0f}% of one core)"
        )
        print(f"  rss                                  : {rss_before / 1024:.0f} -> {rss_of(pid) / 1024:.0f} MiB")
        return 0
    finally:
        # Twice: the first interrupt ends a running turn, the second ends the session. The frame
        # profile is written on the way out, so the wait has to be long enough for it.
        try:
            for _ in range(2):
                os.write(fd, b"\x03")
                read_for(fd, 0.6)
        except OSError:
            pass
        try:
            os.kill(pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        os.close(fd)
        os.waitpid(pid, 0)


if __name__ == "__main__":
    sys.exit(main())
