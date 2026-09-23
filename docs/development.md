# Building and testing

The executable is Rust. Python 3.11+ is used for package installation and the independent CLI contract tests. SQLite is bundled. Development requires Linux, Rust 1.88 or newer, and a C compiler/linker. CI pins its Rust compiler in the workflow.

```sh
cargo build --locked
cargo test --locked
python3 -B -m unittest discover -s tests -p test_cli_compat.py -v
```

The CLI suite uses `target/debug/htalk`, temporary databases, fake client executables, Unix sockets and a loopback HTTP server. It never falls back to an installed `htalk`. To test a specific executable, set `HTALK_TEST_COMMAND` to a JSON argument list starting with its absolute path. `HTALK_TEST_VERSION` defaults to `0.6.0-dev.0`; set it when testing another release.

Schema migration tests use synthetic version 1/2 fixtures; pull and mixed native/pull exchanges use isolated mailboxes. These checks do not migrate a working mailbox.

The same CLI suite was first run against the Python 0.4 implementation, before running it against the Rust port. Native tests cover storage transactions and races, session recognition, discovery and all three transports. They replace the old tests that imported Python implementation details.

For a local binary wheel:

```sh
python -m pip install 'maturin[zig]>=1.15,<2' twine
maturin build --release --locked --zig --compatibility manylinux_2_28 --sdist --target-dir "$(mktemp -d)" --out dist
python -m twine check --strict dist/*
python -m pip install dist/*.whl
```

`--sdist` rebuilds the wheel from the source archive, checking that it contains the required sources. Use a fresh Cargo target directory: archive timestamps are normalized, and reusing cached outputs for the same package version can retain an older executable. For source installation, `python -m pip install .` invokes maturin and the Rust toolchain. For a standalone executable, use `cargo build --release --locked`; copy `target/release/htalk` to a directory on `PATH`.

Distribution checks run the installed wheel from outside the checkout with an explicit executable path. A release still requires the live client checks in [releasing](releasing.md); fixture tests do not establish compatibility with an untested native client version.
