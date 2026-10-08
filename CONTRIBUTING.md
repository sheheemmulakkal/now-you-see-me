# Contributing

Thanks for helping. Bug reports, measurements from other machines and pull
requests are all welcome.

## Before you start

- For anything larger than a small fix, open an issue first so the approach
  can be agreed before you spend time on it.
- The product principles are in [docs/product.md](docs/product.md). The
  most important one: a value that cannot be measured is shown with its
  reason, never as a made-up `0`.

## Build and test

Rust 1.88 or newer (the desktop app needs Rust 1.92 and GTK ≥ 4.12
development files, `libgtk-4-dev` on Debian/Ubuntu).

```sh
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo clippy -p nysm-desktop --all-targets -- -D warnings   # with GTK installed
python3 scripts/pty_smoke.py target/debug/nysm               # TUI in a real terminal
cargo run -p nysm-tui --example screens -- 80 24             # every TUI view as text
```

CI runs the same checks on every push ([docs/testing.md](docs/testing.md)).

## Pull requests

- Keep each pull request to one change, with tests where the behaviour can
  be tested (parsers, math and rendering all have test helpers).
- Describe what changed and how you checked it. For UI changes, add a
  screenshot (`nysm-desktop --screenshot FILE.png`, or the TUI `screens`
  example).
- Update the documentation in `docs/` when behaviour or options change, and
  add a line to `CHANGELOG.md` under *Unreleased*.
- New dependencies need a reason: the CLI must stay free of GUI and session
  libraries (CI checks this), and binaries should stay small.

## Licence

Unless you explicitly state otherwise, any contribution you intentionally
submit for inclusion in this work, as defined in the Apache-2.0 licence,
shall be dual licensed as MIT OR Apache-2.0, without any additional terms or
conditions.
