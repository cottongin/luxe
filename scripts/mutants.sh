#!/usr/bin/env bash
# cargo-mutants, in a memory-capped scope of its own.
#
# A mutant can turn a loop that pushes into one that never ends, and every test that reaches it
# then allocates until something stops it. With nextest running a thread per core that is several
# at once, and on a desktop the thing that stops it is systemd-oomd — which kills the whole unit the
# run lives in, the terminal it was started from included. It did, once, taking an agent's session
# with it.
#
# So the run gets a unit of its own with a ceiling. Hit it and the kernel ends that scope, the test
# in question fails, and the terminal stays up. Where there is no systemd user session (macOS, a
# container, CI) it runs uncapped and says so; the test-thread cap in `.cargo/mutants.toml` still
# applies.
#
#   LUXE_MUTANTS_MEMORY  the ceiling, as systemd reads it (default 24G)
#
# Usage: scripts/mutants.sh [cargo mutants arguments…]
#        scripts/mutants.sh -p luxe-tui -f crates/luxe-tui/src/mentions.rs
set -euo pipefail

cap="${LUXE_MUTANTS_MEMORY:-24G}"
if command -v systemd-run >/dev/null 2>&1 && systemctl --user show-environment >/dev/null 2>&1; then
    exec systemd-run --user --scope --quiet --collect \
        -p MemoryMax="$cap" -p MemorySwapMax=0 \
        -- cargo mutants "$@"
fi
echo "mutants.sh: no systemd user session here, so the run has no memory ceiling" >&2
exec cargo mutants "$@"
