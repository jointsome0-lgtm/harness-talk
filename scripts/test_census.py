#!/usr/bin/env python3
"""Count the tests by kind and list the ones that reach inside htalk.

    python3 scripts/test_census.py

A test reaches inside when it is a Rust test built on the crate (it imports
`harness_talk::`, is compiled into `src/` through `#[path]` or sits in `src/`),
when it reads `/proc/locks` or a SQLite journal file, or when it asserts a
Python exception class name. A helper in the same file counts for the tests
that call it. Class names are asked of this Python's standard library, so none
is written down here.

The census reads files as text. It needs no build and nothing but the standard
library, and prints the same numbers from any clean checkout of one commit.
"""
import ast
import importlib
from pathlib import Path
import re
import sys

REPO = Path(__file__).resolve().parents[1]
# The retired Python implementation raised from these modules.
RAISING = ("builtins", "json", "tomllib", "subprocess", "sqlite3", "http.client", "urllib.error", "socket", "ssl")
CRATE, PATHED, SRC, LOCKS, CLASS = "crate", "pathed", "src", "locks", "class"
MECHANISM = re.compile(r"/proc/locks|[\"']-(?:journal|wal|shm)[\"']")
RUST_TOKEN = re.compile(r"""//[^\n]*|/\*.*?\*/|b?r(#*)"(.*?)"\1|b?"((?:[^"\\]|\\.)*)"|'(?:[^'\\\n]|\\.)'""", re.S)
RUST_TEST = re.compile(r"#\[(?:\w+::)*test\]")
RUST_TEST_ONLY = re.compile(r"#\[cfg\((?:test|all\([^\]]*\btest\b[^\]]*\))\)\]")
RUST_FN = re.compile(r"\bfn\s+(\w+)")
RUST_PATH = re.compile(r"#\[path\s*=\s*\"([^\"]+)\"\]")
NODE_TEST = re.compile(r"^test\(", re.M)
QUOTED = re.compile(r"""(["'`])((?:(?!\1)[^\\\n]|\\.)*)\1""")
CALLED = re.compile(r"\b(\w+)\s*\(")


def class_names():
    names = set()
    for module in RAISING:
        for value in vars(importlib.import_module(module)).values():
            if isinstance(value, type) and issubclass(value, BaseException):
                names.add(value.__name__)
    # `Error`, `Warning` and `timeout` are words, not evidence.
    return {name for name in names if sum(letter.isupper() for letter in name) > 1}


NAMES = class_names()
NAMED = re.compile(r"(?:[a-z0-9]+_)*(%s)" % "|".join(sorted(NAMES)))


class Function:
    def __init__(self, name, text, literals, test, helper=True):
        self.name, self.text, self.test, self.helper = name, text, test, helper
        self.classes = {match.group(1) for match in map(NAMED.fullmatch, literals) if match}
        self.locks = bool(MECHANISM.search(text))
        self.calls = set(CALLED.findall(text))


def python_functions(text):
    lines = text.splitlines()
    for node in ast.walk(ast.parse(text)):
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
            literals = [inner.value for inner in ast.walk(node)
                        if isinstance(inner, ast.Constant) and isinstance(inner.value, str)]
            yield Function(node.name, "\n".join(lines[node.lineno - 1:node.end_lineno]), literals,
                           node.name.startswith("test_"))


def rust_skeleton(text):
    """The text with comments and literal contents blanked, and the literals by position."""
    literals, parts, last = [], [], 0
    for match in RUST_TOKEN.finditer(text):
        parts.append(text[last:match.start()])
        parts.append(re.sub(r"[^\n]", " ", match.group()))
        if match.group(2) is not None or match.group(3) is not None:
            literals.append((match.start(), match.group(2) if match.group(2) is not None else match.group(3)))
        last = match.end()
    return "".join(parts) + text[last:], literals


def rust_body(skeleton, start):
    """The braces of the item that starts here, or None for a declaration."""
    opening = re.compile(r"[{;]").search(skeleton, start)
    if opening is None or opening.group() == ";":
        return None
    depth, end = 0, opening.start()
    while True:
        depth += {"{": 1, "}": -1}.get(skeleton[end], 0)
        end += 1
        if depth == 0 or end == len(skeleton):
            return opening.start(), end


def rust_functions(text, production):
    """Every function with a body. A function is a test when `#[test]` stands between it and the item before.
    In a production file only the functions of a test-only item can be a test's helpers."""
    skeleton, literals = rust_skeleton(text)
    test_only = [span for span in (rust_body(skeleton, match.end()) for match in RUST_TEST_ONLY.finditer(skeleton))
                 if span]
    for match in RUST_FN.finditer(skeleton):
        span = rust_body(skeleton, match.end())
        if span is None:
            continue
        before = max(skeleton.rfind(mark, 0, match.start()) for mark in "{};") + 1
        inside = [value for position, value in literals if span[0] < position < span[1]]
        yield Function(match.group(1), text[span[0]:span[1]], inside,
                       bool(RUST_TEST.search(skeleton, before, match.start())),
                       not production or any(start < match.start() < end for start, end in test_only))


def node_functions(text):
    starts = [match.start() for match in NODE_TEST.finditer(text)] + [len(text)]
    for start, end in zip(starts, starts[1:]):
        body = text[start:end]
        yield Function(QUOTED.search(body).group(2), body, [found[1] for found in QUOTED.findall(body)], True)


def reasons(functions):
    """Each test with what it and the same-file helpers it calls reach for."""
    helpers = {}
    for function in functions:
        if function.helper and not function.test:
            helpers.setdefault(function.name, []).append(function)
    for test in (function for function in functions if function.test):
        seen, queue, classes, locks = set(), [test], set(), False
        while queue:
            function = queue.pop()
            classes |= function.classes
            locks = locks or function.locks
            for name in function.calls - seen:
                seen.add(name)
                queue.extend(helpers.get(name, []))
        yield test.name, classes, locks


def census(repo=REPO):
    pathed = {(source.parent / target).resolve() for source in (repo / "src").rglob("*.rs")
              for target in RUST_PATH.findall(source.read_text())}
    kinds = {"Python": 0, "Rust in tests/": 0, "Rust in src/": 0, "Node": 0}
    lines, importing, tests = 0, 0, []
    for path in sorted([*(repo / "tests").rglob("*"), *(repo / "src").rglob("*.rs")]):
        if path.suffix not in (".py", ".rs", ".mjs") or not path.is_file():
            continue
        text, inner = path.read_text(), path.is_relative_to(repo / "src")
        if not inner:
            lines += text.count("\n")
        if path.suffix == ".py":
            kind, found, built = "Python", python_functions(text), None
        elif path.suffix == ".mjs":
            kind, found, built = "Node", node_functions(text), None
        else:
            kind, found = "Rust in src/" if inner else "Rust in tests/", rust_functions(text, inner)
            imports = not inner and "harness_talk::" in text
            importing += imports
            built = SRC if inner else PATHED if path.resolve() in pathed else CRATE if imports else None
        for name, classes, locks in reasons(list(found)):
            kinds[kind] += 1
            why = {reason for reason, holds in ((built, built), (LOCKS, locks), (CLASS, classes)) if holds}
            tests.append((str(path.relative_to(repo)), name, why))
    return kinds, lines, importing, tests


def report(kinds, lines, importing, tests):
    def count(*wanted):
        return sum(1 for _, _, why in tests if why & set(wanted))

    def line(label, number, indent=0):
        return "%s%-*s %6s" % (" " * indent, 44 - indent, label, format(number, ","))

    def by_file(*wanted):
        files = {}
        for path, name, why in tests:
            if why & set(wanted):
                files.setdefault(path, []).append(name)
        return files

    out = [line("tests", len(tests)), *(line(kind, number, 2) for kind, number in kinds.items()),
           line("lines under tests/", lines), "",
           line("tests that reach inside", count(CRATE, PATHED, SRC, LOCKS, CLASS)),
           line("Rust tests built on the crate", count(CRATE, PATHED, SRC), 2),
           line("import harness_talk::", count(CRATE), 4),
           line("compiled into src/ by #[path]", count(PATHED), 4),
           line("sit in src/", count(SRC), 4),
           line("read /proc/locks or a journal file", count(LOCKS), 2),
           line("assert a Python exception class name", count(CLASS), 2),
           line("files under tests/ importing harness_talk::", importing), "",
           "Rust tests built on the crate"]
    out += ["  %3d  %s" % (len(names), path) for path, names in by_file(CRATE, PATHED, SRC).items()]
    for title, reason in (("Tests that read /proc/locks or a journal file", LOCKS),
                          ("Tests that assert a Python exception class name", CLASS)):
        out += ["", title]
        for path, names in by_file(reason).items():
            out += ["  " + path, *("    " + name for name in names)]
    return "\n".join(out)


if __name__ == "__main__":
    print(report(*census(Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else REPO)))
