# Building and testing

The executable is Rust. Python 3.11+ is used for package installation and the independent CLI contract tests. SQLite is bundled. Development requires Linux, Rust 1.88 or newer, and a C compiler/linker. CI pins its Rust compiler in the workflow.

```sh
cargo build --locked
cargo test --locked
python3 -B -m unittest discover -s tests -p 'test_*.py' -v
node --experimental-vm-modules --test tests/test_js_watchers.mjs
```

Python adapter tests cover owned stdin, mailbox command selection, and recovery boundaries with fake SDK protocols and disposable processes. CI runs these tool and recovery fixtures on both CI Python versions. They do not establish provider durability or native crash recovery.

JavaScript watcher tests require Node 24 and use the actual adapters with modeled SDK calls and disposable local watcher processes. They need no npm dependencies and do not establish native client compatibility.

The CLI suite uses `target/debug/htalk`, temporary databases, fake client executables, Unix sockets and a loopback HTTP server. It never falls back to an installed `htalk`. To test a specific executable, set `HTALK_TEST_COMMAND` to a JSON argument list starting with its absolute path.

Schema migration tests use synthetic version 1/2 fixtures and cover automatic first opens, backup contents and permissions, concurrent writers, and rollback on failure. Pull and mixed native/pull exchanges use isolated mailboxes. These checks do not migrate a working mailbox.

The catalogue suite uses isolated schema-3 mailboxes to check private export,
publication and binding changes, read-only legacy-schema refusal, strict SSH
command dispatch, conversation scopes before pagination and per-call MCP
identity mismatches. It neither advertises on the LAN nor contacts real agents.
Native mDNS and SSH checks require explicitly selected owned devices and a
separate temporary endpoint.

The agent view is everything an agent reads from htalk. `tests/agent_view/` describes it in plain text: the rendered `--help` of every command (`help.txt`), what htalk supplies over MCP stdio (`mcp.txt`), the notification text (`notification.txt`), the keys and fixed values of command results (`results.txt`), the fixed error and detail codes with the `next_action` and `recovery` each failure returns (`errors.txt`; `docs/reference.md` has a row for each code, and the test fails when the two lists differ), the texts in the source that no captured entry shows (`texts.txt`), and how much of that text an agent loads (`size.txt`). `tests/test_agent_view.py` captures each part again through the executable and MCP stdio and fails on any difference. It reads `src/` only as text, for the fixed codes and the unshown texts that no command prints, so it runs in a checkout. After an intended change to the agent view, regenerate the files and review their diff with the rest of the change:

```sh
python3 -B tests/test_agent_view.py --update
```

`python3 -B tests/test_agent_view.py --size` prints the size report.

The CLI suite covers registration, request/reply/ACK states, pagination, recovery, migration and native/pull exchanges. Rust tests cover storage races and adapter-specific identity, discovery and delivery failures. Keep a behavior in one layer when another test already exercises the same failure; retain separate tests for distinct races and transport boundaries.

MCP tests exercise the same compiled executable through stdio. They cover
message exchange with pinned identity and saved-error reporting, legacy tool
arguments through an unbound connector, reconnecting after remote failure
without replaying a lost write, and cancellation or client loss without duplicate
writes or leftover children. They also check that terminal Ctrl-C preserves the
server connection. Run these on a host that permits
async signal/IPC handling; restricted sandboxes can prevent that handling.
The receiver cancellation checks require the same host access; a sandbox-only
shutdown timeout must be compared with an ordinary host run before changing
the product's signal handling.

Rust MCP connectors, watch receivers and bounded command capture use Linux
pidfds and readable `/proc` process metadata to stop their private groups. This
requires Linux 5.3 or newer. The direct child remains unreaped until cleanup
finishes; signals target inspected process handles, so a saved numeric group ID
cannot target a later group. Cleanup covers members that remain in the group
with process metadata owned by the current effective UID. Escaped descendants,
changed ownership and inaccessible metadata are outside the success guarantee;
inspection failures produce errors. Two-second TERM grace and four-second cleanup
budgets are checked between `/proc` scans; scanning and scheduler delays can
extend elapsed cleanup time.

macOS and Windows hold the group of an `htalk mcp` call in their own way, with the same two and four seconds. On macOS a signal goes to the group by its number, which the unreaped child keeps for it. On Windows the child is created suspended, put in a job and only then let run; at the end a child that still runs is sent Ctrl-Break, and the job is ended two seconds later. Bounded command capture is on Linux only.

Managed Python receivers require Linux kernel 5.3 or newer, Python 3.11+ with
`os.pidfd_open` and `signal.pidfd_send_signal`, and a mounted `/proc` readable
for entries owned by the launcher's effective UID and ambiguous root-owned
processes. They check this support
before native launch and refuse to launch on unsupported hosts. Group cleanup
covers members that remain in the managed session and process group and retain
that UID. Descendants that leave the group or change UID are outside this
guarantee. Isolated process fixtures do not verify native adapter cleanup.

Check what each assertion protects for the caller before preserving it. Old Python behavior and a passing test do not establish a requirement. Compare JSON fields and values without requiring key order or spacing. Isolate invalid inputs unless error precedence itself affects recovery. Delivery outcomes must follow whether submission could have begun, not the exception class that happened to escape an older adapter.

CI runs the full Rust and installed CLI suites, with the agent view, once, on Python 3.14. Both Python 3.11 and 3.14 build and install the package, then run `htalk --version`, `htalk --help` and `pip check` outside the checkout. A separate job checks the minimum supported Rust version.

For a local binary wheel:

```sh
python -m pip install 'maturin[zig]>=1.15,<2' twine
maturin build --release --locked --zig --compatibility manylinux_2_28 --sdist --target-dir "$(mktemp -d)" --out dist
python -m twine check --strict dist/*
python -m pip install dist/*.whl
```

`--sdist` rebuilds the wheel from the source archive, checking that it contains the required sources. Use a fresh Cargo target directory: archive timestamps are normalized, and reusing cached outputs for the same package version can retain an older executable. For source installation, `python -m pip install .` invokes maturin and the Rust toolchain. For a standalone executable, use `cargo build --release --locked`; copy `target/release/htalk` to a directory on `PATH`.

Distribution checks run the installed wheel from outside the checkout with an explicit executable path. A release still requires the live client checks in [releasing](releasing.md); fixture tests do not establish compatibility with an untested native client version.

## Contended writes

A `send` or `reply` that finds the mailbox busy before its first write queues for a turn, as the [reference](reference.md) describes, and this was kept after measuring it against plain SQLite lock waiting. With 32 senders writing one after another, plain waiting let one send in 9,600 fail with `database is locked` after 5.5 seconds, and with 128 senders 205 of 1,280 failed, while the turn lost none; the turn now has no random step.

`scripts/contended_sends.py` repeats the measurement. It starts 2, 8 and 32 senders against one temporary mailbox and prints, for each, the exit codes, the sends that had to be repeated, the rows lost or doubled, and the median, mean and worst time of one send:

```sh
cargo build --release --locked
HTALK_TEST_COMMAND="[\"$PWD/target/release/htalk\"]" python3 scripts/contended_sends.py
```

Run it on a local disk, not a network mount.

## Test rules

A test meets htalk where its callers do, at one of these seams:

- a command in and JSON out, through the executable selected by `HTALK_TEST_COMMAND` or through MCP stdio;
- a fake client that observes the adapter's calls: the fake `claude` and `codex` executables, the fake Claude socket and the loopback OpenCode server;
- a real mailbox file given to the command;
- the receiver contracts under `integrations/`.

Tests move outward to these seams. A new test never imports `harness_talk::`, never asserts an exception class name, never reads `/proc/locks` or a journal file, and never asserts a sleep length. It waits on something a caller could see too: the fake client's record of a call, what the database returns or refuses, a file the command reads, or the command's exit.

When a module is restructured, its tests move to a seam first and pass on the old code. Then the code changes. Then the inner tests go. A ported test asserts on what a command returns. A test that only checked internal calls or wire framing is deleted, not ported.

The sdist includes `tests/**` and the workflows name test files. A change that adds, moves or deletes a test file updates `.github/workflows/tests.yml`, and `publish.yml` where it names the same file, in the same pull request.

`scripts/test_census.py` counts the tests by kind and lists the ones that still reach inside: Rust tests built on the crate (they import `harness_talk::`, are compiled into `src/` through `#[path]` or sit in `src/`), tests that read `/proc/locks` or a journal file, and tests that assert a Python exception class name. The last two are read off a test's string literals, so they are hints for a person to go through, and a helper in the same file counts for the tests that call it. The script reads files as text and uses only the standard library, so it needs no build:

```sh
python3 scripts/test_census.py
```

The same script counts the lines of `src/` outside `src/os/` that name something only a Unix or Linux system has: `libc::`, a signal, `/proc`, a Unix extension of the standard library. That number is 0. Such a call goes into `src/os/`, and the rest of the program asks `os::` for it.

`build.rs` says which parts a system has: native delivery to Codex and Claude Code with `receive`, the MCP server, and the catalogue. The program asks for a part with `cfg(native_clients)`, `cfg(mcp_server)` or `cfg(catalog)` and never for a system. Where a part is missing it answers `unsupported_on_this_platform` and writes nothing of its own. A port adds its system to a line of `build.rs` and its code to `src/os/`. The `platforms` job of `tests.yml` builds, lints and runs the Rust tests on macOS and Windows. `tests/platform.rs` holds what every system must do, and what a missing part answers. On both the job also runs `test_cli_compat`, `test_mcp` and `test_mail_recovery` against the built executable. There `add_peer(name)` of the shared fixture registers a pull peer, `any_peer` names what a test expects of either kind, and a test that needs Codex or Claude Code delivery is skipped by the fixture with one reason. On Windows two tests that watch a send through a named pipe are skipped because the file tree has none.

The core cleanup, issue #65, adds rules for each of its pull requests. `main` stays green and releasable after each merge: `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings`, `cargo test --locked`, then the Python and Node suites named in `.github/workflows/tests.yml`. Each pull request reports, before and after, the core size in tokens as `scripts/core_tokens.py` prints it, the tests that reach inside, the test count and the lines of test code. A visible change found along the way is not made there; it is noted on issue #76. Code the next phase deletes, the Python class names, the `python_*` helpers and the hand-rolled HTTP client, is gathered into one place, not polished.

## The core

The core is what a change to mailbox behaviour must read: the commands and their dispatch, the store and its schema, the turn of a contended send, the model of a peer and a message, errors, validation, the recovery guidance, the notification, the session a command runs in, the adapter contract and the portable part of `src/os/`.

- In the core: `src/main.rs`, `src/lib.rs`, `src/cli.rs`, `src/commands.rs`, `src/commands/**`, `src/store.rs`, `src/schema.rs`, `src/write_turn.rs`, `src/model.rs`, `src/error.rs`, `src/validate.rs`, `src/guidance.rs`, `src/notify.rs`, `src/identity.rs`, `src/adapters/mod.rs` and `src/os/mod.rs`.
- Outside: the adapters under `src/adapters/`, `src/catalog/**`, `src/mcp.rs`, `src/discovery.rs` and the platform code under `src/os/`.

`scripts/core_tokens.py` measures it. The measure is the bytes of the core files divided by 3.3, a rough count of tokens. The script prints each core file, the total and, for information, the core together with the adapters:

```sh
python3 scripts/core_tokens.py
```

CI runs it on every push. It fails when the core is above 70,000 tokens. It also fails when a file under `src/` is on neither list, so a new file is placed by hand, in the script and here. A file belongs in the core when a change to what `send`, `reply`, `wait`, `show`, `ack`, `inbox`, `watch`, `sent` or `peer` does cannot be made without reading it. Code for one harness, one channel or one platform stays outside.
