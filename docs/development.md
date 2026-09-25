# Building and testing

The executable is Rust. Python 3.11+ is used for package installation and the independent CLI contract tests. SQLite is bundled. Development requires Linux, Rust 1.88 or newer, and a C compiler/linker. CI pins its Rust compiler in the workflow.

```sh
cargo build --locked
cargo test --locked
python3 -B -m unittest discover -s tests -p 'test_*.py' -v
```

The CLI suite uses `target/debug/htalk`, temporary databases, fake client executables, Unix sockets and a loopback HTTP server. It never falls back to an installed `htalk`. To test a specific executable, set `HTALK_TEST_COMMAND` to a JSON argument list starting with its absolute path.

Schema migration tests use synthetic version 1/2 fixtures and cover automatic first opens, backup contents and permissions, concurrent writers, and rollback on failure. Pull and mixed native/pull exchanges use isolated mailboxes. These checks do not migrate a working mailbox.

The CLI suite covers registration, request/reply/ACK states, pagination, recovery, migration and native/pull exchanges. Rust tests cover storage races and adapter-specific identity, discovery and delivery failures. Keep a behavior in one layer when another test already exercises the same failure; retain separate tests for distinct races and transport boundaries.

Two MCP cases exercise the same compiled executable through stdio: message
exchange with pinned identity and saved-error reporting, and cancellation or
client loss without duplicate writes or leftover children. They also check that
terminal Ctrl-C preserves the server connection. Run these on a host that permits
async signal/IPC handling; restricted sandboxes can prevent that handling.

Check what each assertion protects for the caller before preserving it. Old Python behavior and a passing test do not establish a requirement. Compare JSON fields and values without requiring key order or spacing. Isolate invalid inputs unless error precedence itself affects recovery. Delivery outcomes must follow whether submission could have begun, not the exception class that happened to escape an older adapter.

CI runs the full Rust and installed CLI suites once, on Python 3.14. Both Python 3.11 and 3.14 build and install the package, then run `htalk --version`, `htalk --help` and `pip check` outside the checkout. A separate job checks the minimum supported Rust version.

For a local binary wheel:

```sh
python -m pip install 'maturin[zig]>=1.15,<2' twine
maturin build --release --locked --zig --compatibility manylinux_2_28 --sdist --target-dir "$(mktemp -d)" --out dist
python -m twine check --strict dist/*
python -m pip install dist/*.whl
```

`--sdist` rebuilds the wheel from the source archive, checking that it contains the required sources. Use a fresh Cargo target directory: archive timestamps are normalized, and reusing cached outputs for the same package version can retain an older executable. For source installation, `python -m pip install .` invokes maturin and the Rust toolchain. For a standalone executable, use `cargo build --release --locked`; copy `target/release/htalk` to a directory on `PATH`.

Distribution checks run the installed wheel from outside the checkout with an explicit executable path. A release still requires the live client checks in [releasing](releasing.md); fixture tests do not establish compatibility with an untested native client version.
