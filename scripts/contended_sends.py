#!/usr/bin/env python3
"""Time `send` calls that reach one mailbox together and count what they lose.

    cargo build --release --locked
    HTALK_TEST_COMMAND="[\\"$PWD/target/release/htalk\\"]" python3 scripts/contended_sends.py

Without HTALK_TEST_COMMAND it measures `target/debug/htalk`, as the tests do. Run it where
the temporary directory is on a local disk, not a network mount: every commit waits for the disk.

Four measurements, each with 2, 8 and 32 senders, ROUNDS times:

  apart      the senders start together and each saves its own message for a peer that polls
  same-id    the senders start together with the same `--id` and body, so one creates the row
  notifying  as `apart`, while another send's notification is held at the fake Codex client
  stream     every sender sends ROUNDS messages one after another, so the mailbox stays contended

A send that fails is repeated with the same id until it succeeds, at most five times. Per
measurement the table gives the sends, the exit codes of the first tries, the repeats, the
rows lost, the rows doubled, and the median, the mean and the worst time of one first try
from start to exit. It uses the fake clients and the temporary mailbox of the tests and nothing else.
"""
import argparse
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
import statistics
import sys
import threading
import time
import uuid

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tests"))
from compat_support import HtalkCase  # noqa: E402

KINDS = ("apart", "same-id", "notifying", "stream")
GATE = "codex_queue"


def together(box, senders):
    """Start the senders at one moment, each with its calls in order. Every call gives the exit
    code, the seconds and the JSON."""
    barrier = threading.Barrier(len(senders))

    def one(calls):
        barrier.wait()
        results = []
        for words in calls:
            begun = time.monotonic()
            result = box.run_raw(*words)
            results.append((result.code, time.monotonic() - begun, result.json))
        return results

    with ThreadPoolExecutor(len(senders)) as pool:
        return [result for results in pool.map(one, senders) for result in results]


def hold_a_notification(box):
    for mark in (GATE + ".started", GATE + ".release"):
        (box.state / mark).unlink(missing_ok=True)
    process = box.spawn("--as", "alice", "send", "carol", "--message", "Held at the client")
    box.started(GATE)
    return process


def measure(box, kind, senders, rounds):
    first, repeats, lost, doubled = [], 0, 0, 0
    each, rounds = (rounds, 1) if kind == "stream" else (1, rounds)
    for _ in range(rounds):
        ids = [str(uuid.uuid4()) for _ in range(senders * each)]
        if kind == "same-id":
            ids = ids[:1] * senders
        calls = [("--as", "alice", "send", "bob", "--id", chosen, "--message", "Message " + chosen)
                 for chosen in ids]
        held = hold_a_notification(box) if kind == "notifying" else None
        results = together(box, [calls[sender * each:(sender + 1) * each] for sender in range(senders)])
        if held:
            box.release(GATE)
            held.communicate(timeout=30)
        first += results
        created = 0
        for words, (code, _, answer) in zip(calls, results):
            for _ in range(5):
                if code == 0:
                    break
                again = box.run_raw(*words)
                code, answer, repeats = again.code, again.json, repeats + 1
            created += code == 0 and answer["created"] is True
        rows = [row[0] for row in box.sql("SELECT id FROM messages") if row[0] in set(ids)]
        lost += len(set(ids) - set(rows))
        doubled += len(rows) - len(set(rows)) + max(0, created - len(set(ids)))
    codes = sorted({code for code, _, _ in first})
    seconds = [taken for _, taken, _ in first]
    return (kind, senders, len(first), " ".join("%d×%d" % (code, sum(c == code for c, _, _ in first)) for code in codes),
            repeats, lost, doubled, "%.3f" % statistics.median(seconds), "%.3f" % statistics.mean(seconds),
            "%.3f" % max(seconds))


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--rounds", type=int, default=20)
    parser.add_argument("--senders", type=int, nargs="+", default=[2, 8, 32])
    parser.add_argument("--only", nargs="+", default=KINDS, choices=KINDS, metavar="MEASUREMENT")
    args = parser.parse_args()
    box = HtalkCase()
    box.setUp()
    try:
        for name in ("alice", "bob"):
            box.htalk("peer", "add", name, "--harness", "generic", "--delivery", "pull")
        box.codex_recipient("carol")
        box.configure(codex_queue={"mode": "block"})
        print(box.run_raw("--version", db=False).stdout.strip(), "· %d rounds" % args.rounds)
        table = [("measurement", "senders", "sends", "first exit codes", "repeats", "lost", "doubled", "median s", "mean s", "worst s")]
        table += [measure(box, kind, senders, args.rounds) for kind in args.only for senders in args.senders]
        widths = [max(len(str(row[column])) for row in table) for column in range(len(table[0]))]
        for row in table:
            print("  ".join(str(cell).ljust(width) for cell, width in zip(row, widths)).rstrip())
    finally:
        box.doCleanups()


if __name__ == "__main__":
    main()
