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

The agent view is everything an agent reads from htalk. `tests/agent_view/` describes it in plain text: the rendered `--help` of every command (`help.txt`), what htalk supplies over MCP stdio (`mcp.txt`), the notification text (`notification.txt`), the keys and fixed values of command results (`results.txt`), the fixed error and detail codes with the `next_action` and `recovery` each failure returns (`errors.txt`), and how much of that text an agent loads (`size.txt`). `tests/test_agent_view.py` captures each part again through the executable and MCP stdio and fails on any difference. It reads `src/` only as text, for the fixed codes that no command prints, so it runs in a checkout. After an intended change to the agent view, regenerate the files and review their diff with the rest of the change:

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
