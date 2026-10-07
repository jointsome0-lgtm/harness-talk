#!/usr/bin/env python3
"""Measure the core: what a change to mailbox behaviour must read.

    python3 scripts/core_tokens.py

The measure is the bytes of the core files divided by 3.3, a rough count of
tokens. The script prints each core file, the core total and, for information,
the core together with the adapters. It exits 1 when the core is above 70,000
tokens, and when a file under `src/` is on none of the lists below: a new file
is put in the core or outside it by hand, here and in `docs/development.md`.

It reads the sources as text, whatever features a build selects. It needs no
build and nothing but the standard library.
"""
from pathlib import Path
import sys

REPO = Path(__file__).resolve().parents[1]
BYTES_PER_TOKEN = 3.3
CEILING = 70_000
# A name that ends in `/` stands for every file below it. A file named in full
# goes before a directory: `src/adapters/mod.rs` is core, the rest of
# `src/adapters/` is not.
CORE = (
    "src/main.rs", "src/lib.rs", "src/cli.rs", "src/commands.rs", "src/commands/",
    "src/store.rs", "src/schema.rs", "src/write_turn.rs", "src/model.rs", "src/error.rs",
    "src/validate.rs", "src/guidance.rs", "src/notify.rs", "src/identity.rs",
    "src/adapters/mod.rs", "src/os/mod.rs",
)
ADAPTERS = ("src/adapters/",)
OUTSIDE = ("src/catalog/", "src/mcp.rs", "src/discovery.rs", "src/compat.rs", "src/os/")


def place(name):
    """The list a source file is on, or None."""
    for whole in (True, False):
        for names in (CORE, ADAPTERS, OUTSIDE):
            if any(name == n if whole else n.endswith("/") and name.startswith(n) for n in names):
                return names
    return None


def tokens(size):
    return round(size / BYTES_PER_TOKEN)


def main():
    sizes = {p.relative_to(REPO).as_posix(): p.stat().st_size for p in sorted((REPO / "src").rglob("*.rs"))}
    unlisted = [name for name in sizes if place(name) is None]
    core = {name: size for name, size in sizes.items() if place(name) is CORE}
    adapters = sum(size for name, size in sizes.items() if place(name) is ADAPTERS)
    total = sum(core.values())
    print(f"{'bytes':>8} {'tokens':>7}  core file")
    for name, size in core.items():
        print(f"{size:>8} {tokens(size):>7}  {name}")
    print(f"{total:>8} {tokens(total):>7}  the core")
    print(f"{total + adapters:>8} {tokens(total + adapters):>7}  the core plus adapters, for information")
    over = tokens(total) > CEILING
    for name in unlisted:
        print(f"on no list: {name}")
    print(f"ceiling {CEILING} tokens: {'above' if over else 'under'}, exit {int(over or bool(unlisted))}")
    return int(over or bool(unlisted))


if __name__ == "__main__":
    sys.exit(main())
