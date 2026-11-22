# Development

## Requirements

- Rust: the version pinned in `rust-toolchain.toml` (rustup installs it
  on first build), edition 2024
- [`just`](https://github.com/casey/just)
- a C compiler (libgit2 is built by `libgit2-sys`); on Linux also
  OpenSSL headers and `pkg-config`
- `git` on `PATH`: the build stamps the commit hash, the tests create
  repositories, and the tool itself calls it

## Recipes

```bash
just                  # list recipes
just check            # clippy (warnings are errors) + tests
just lint             # clippy only
just test             # tests only
just fmt              # format
just build            # debug build -> target/debug/ghh
just release          # lint, then optimised build -> target/release/ghh
just run ~/some/repo  # run from source
just install          # cargo install --path crates/ghh --force --locked -> ~/.cargo/bin/ghh

just bench            # synthetic graph-engine and search benches
just bench-repo       # stage costs against a big local repository (skips if absent)
just bench-e2e        # release build driven by --auto on that repository, prints --stats
```

`just --list` shows every recipe with its comment. `build`, `release`,
`run` and `install` pass extra arguments through.

`just check` must pass before a commit.

`bench-repo` and `bench-e2e` expect a large repository at a path
written in the test and in the `justfile`; change it to one you have.

## Layout

```text
Cargo.toml            workspace: members, one version, shared dependency versions
Cargo.lock            committed
rust-toolchain.toml   the pinned Rust version
justfile              recipes
crates/ghh/           the tool: package tool-ghh, binary ghh
crates/ghh/build.rs   version stamp: commit hash and build date
crates/ghh/src/       code; module table in ARCHITECTURE.md
crates/ghh/testing/   sample --auto key scripts
docs/                 ARCHITECTURE, DEVELOPMENT, FAQ, SETTINGS, CODING-STYLE
```

[ARCHITECTURE.md](ARCHITECTURE.md) says where code goes and why;
[CODING-STYLE.md](CODING-STYLE.md) says how it is written.

## Tests

All tests are unit tests inside `crates/ghh/src`, at the bottom of the
file they cover.

```bash
just test                               # everything
cargo test -p tool-ghh stash            # tests whose name contains "stash"
cargo test -p tool-ghh -- --nocapture   # with output
```

Tests that need a repository create one in a temp directory and remove
it; none touches the repository you are working in. Tests that need a
screen render into ratatui's test backend, so no terminal is required.

Benchmarks are `#[ignore]`d tests run in release mode by the `bench*`
recipes; they are not part of `just check`.

### Driving the real binary

`--auto` feeds keys to the running app, from a comma-separated string
or a script file, and `--auto-delay` slows it down enough to watch:

```bash
just run ~/some/repo --auto "ARROW_DOWN,ARROW_DOWN,ENTER,SLEEP 500,ESC" --auto-delay 200
just run ~/some/repo --auto crates/ghh/testing/keys1.txt --auto-delay 300
```

End with `LOWER_Q` to quit, otherwise the app stays open for manual
use. Add `--stats` for timings on exit and `-y` to skip confirmation
prompts. Key names and the script syntax are in the
[README](../README.md#automation).

This is the only way to check a change end to end. What it still cannot
show: how a particular terminal or font renders the graph, real mouse
behaviour, the clipboard.

### Seeing what the app does

```bash
RUST_LOG=debug just run ~/some/repo --logs /tmp/ghh.log    # terminal 1
tail -f /tmp/ghh.log                                       # terminal 2
```

Inside the app, `D` opens the same event stream. Never debug with
`println!`: it writes over the screen.

## Adding a feature

A feature is one file per layer.

1. `git/<x>.rs`: the repository operation. Takes a `&Path`, returns
   `git::Result<T>`; a new failure is a new `git::Error` variant in
   `git/error.rs`. Re-export it from `git/mod.rs`. Test it against a
   temp repository.
2. `app/models.rs`: a `Mode` variant if it has its own screen or popup;
   a `ConfirmAction` variant if it changes the repository (and add it
   to `is_dangerous` if it destroys something).
3. `app/features/<x>.rs`: state and key handling in an `impl App`
   block. Open with `self.nav.push(Mode::X)`, close with `pop`. Errors
   become `self.status_message`. List the file in `app/features/mod.rs`.
4. `app/input/dispatch.rs`: route the mode's keys to the handler, and
   the key that opens it. Mouse behaviour, if any, in `app/input/mouse.rs`.
5. `app/layout.rs`: the regions, so drawing and hit-testing share them.
6. `ui/screens/<x>.rs` or `ui/popups/<x>.rs`: a `render_*` function,
   called from `ui/renderer.rs`.
7. `app/features/help.rs`: the keys, for the help popup.
8. `events/models.rs`: a `ScreenId` for the debugger.
9. Docs: the README section, a row in [FAQ.md](FAQ.md), a row in the
   module table of [ARCHITECTURE.md](ARCHITECTURE.md). A new settings
   key also goes into [SETTINGS.md](SETTINGS.md).

`stash` is the smallest complete example: `git/stash.rs`,
`app/features/stash.rs`, `ui/screens/stash.rs`.

## Releasing

Bump `version` once, in `[workspace.package]` of the root `Cargo.toml`.
`ghh --version` shows it together with the commit hash and build date.

## Troubleshooting

**The terminal is left in a strange state after a crash.** Run `reset`.
The restore guard covers normal exits and signals, not a `kill -9`.

**`ghh --version` says the commit is `unknown`.** The binary was built
outside a git checkout (a source tarball, a packaged crate).

**The build fails in `openssl-sys`.** Install the OpenSSL development
package and `pkg-config` (Debian/Ubuntu: `libssl-dev pkg-config`).

**The graph shows broken or misaligned glyphs.** A font problem, not a
logic one: compare with `--ascii`.
