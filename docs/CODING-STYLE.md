# Coding Style

## Module layout: `mod.rs` is an index, nothing else

`mod.rs` only declares submodules and re-exports. No logic, no types, no
`impl` blocks. Code lives in named sibling files.

```
input/
├── mod.rs        # index only: `pub mod dispatch;` + re-exports
├── dispatch.rs   # key dispatch logic
├── mouse.rs
└── clipboard.rs
```

Good `input/mod.rs`:

```rust
pub mod clipboard;
pub mod dispatch;
pub mod mouse;

pub use dispatch::process_key;
```

Bad: putting the logic itself inside `input/mod.rs`.

Why: `mod.rs` files all share the same filename — a folder full of logic-heavy
`mod.rs` files is painful to navigate in editors and grep output. A named file
(`dispatch.rs`) says what it contains.

## Layers: `git` → `app` → `ui`

- `git/` talks to the repository (libgit2 or the `git` CLI) and knows nothing
  about the TUI. Functions take `&Path` + plain args and return
  `git::Result<T>`.
- `app/` owns state and key handling. One file per feature under
  `app/features/`, each extending `App` with its own `impl App { … }` block.
- `ui/` only draws: `render_*(frame, …, area)` functions reading state they are
  handed. No git calls, no state mutation. It may use `git` *types*
  (`CommitInfo`, `StashEntry`), never `git` functions.
- Popup/screen rectangles are computed in `app/layout.rs` so rendering and mouse
  hit-testing share one source of truth.

Why: a feature is then three predictable files (`git/x.rs`,
`app/features/x.rs`, `ui/screens/x.rs`), and rendering stays testable without
a repository.

## Plain types go in `models.rs`

Shared enums and structs of a module live in its `models.rs` (`app/models.rs`,
`git/models.rs`, `events/models.rs`). State that belongs to a single feature
stays in that feature's file (`SquashMessageState` in `features/squash.rs`).

## Errors: return them, then show them

- Fallible code returns a typed error defined with `thiserror`: one error enum
  per module that can fail (`git::Error`, `automation::Error`),
  plus a `Result<T>` alias, with a
  `#[error("…")]` message per variant and `#[from]` for wrapped sources
  (`git2::Error`, `std::io::Error`). No stringly errors, no hand-written
  `impl Display`/`impl Error`.
- `main.rs` has its own small enum that wraps each layer's error with `#[from]`,
  and prints the message itself before exiting non-zero. No `anyhow`, no
  `Box<dyn Error>`.
- The `app` layer is where errors stop: turn them into
  `self.status_message = Some(format!("Error …: {e}"))`. A failed git
  operation must never crash or silently vanish.
- `unwrap()` is for tests and for invariants proven on the line above. For
  anything touching the repo, the filesystem or user input, use `?`,
  `let … else`, or `unwrap_or`.

## Visibility: as narrow as works

Default to `pub(crate)` (or `pub(super)`) for `App` methods and helpers. Use
`pub` for what a parent `mod.rs` re-exports. Callers import through the
re-export (`crate::git::StashEntry`), not the inner path
(`crate::git::stash::StashEntry`).

## Comments and docs

- Start each file with a `//!` line saying what it is for.
- `///` on public items and on every non-obvious enum variant or struct field —
  say what the value *means* (`/// Stash commit hash — the stable identity.`),
  not what its type is.
- Inline `//` comments explain *why*, especially git quirks
  (`// No --date here: it would turn %gd into a date instead of the index.`).
- Split long files into sections with a banner:
  `// ── App impl: squash mode ────────────────`.

## Tests

- Unit tests sit at the bottom of the file they test, in `#[cfg(test)] mod tests`
  (or a descriptive name such as `cli_tests` when a file has several).
- Test names state the behaviour: `parses_settings_values_case_insensitive`,
  not `test_parse`.
- Benchmarks and anything needing a local repo are `#[ignore]`d and run through
  `just bench*`.

## Logging

Never `println!`/`eprintln!` while the TUI is running — it corrupts the screen.
Use `tracing::{debug,info,warn}!` or emit an `AppEvent`; both end up in the log
file and the in-app debugger.

## Tooling

- `just fmt` before committing; default rustfmt, no overrides.
- `just check` (clippy with `-D warnings`, then tests) must pass.
- Fix the lint instead of silencing it. An `#[allow(…)]` needs a reason next to
  it.
- New dependencies go in `[workspace.dependencies]` in the root `Cargo.toml` and
  are referenced with `{ workspace = true }`.
