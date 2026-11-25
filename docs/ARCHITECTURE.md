# Architecture

How `ghh` (`crates/ghh`, package `tool-ghh`) is put together and why.
For what it does from the outside, see the [README](../README.md) and
[FAQ.md](FAQ.md); for building and testing, [DEVELOPMENT.md](DEVELOPMENT.md);
for the rules new code follows, [CODING-STYLE.md](CODING-STYLE.md).
Paths below are relative to `crates/ghh/src`.

The repository is a Cargo workspace with one member. The workspace form
is kept so the version, edition, license and dependency versions sit in
the root `Cargo.toml` (`[workspace.package]`, `[workspace.dependencies]`)
and a second tool can be added under `crates/` without moving anything.

## Shape

One binary, one UI thread, a few short-lived worker threads. Three
layers, each knowing only the ones below it:

```text
 terminal (crossterm)                    the repository
   keys, mouse, paste, resize              .git, working tree
        |                                        ^
        v                                        |
  main.rs  event loop                     git/  libgit2 + the git command
        |                                        ^
        v                                        |
  app/input/   dispatch.rs, mouse.rs             |
        |                                        |
        v                                        |
  app/features/<x>.rs   impl App  ---------------+
        |      state changes, git calls, status message
        v
  app/state.rs  App  (the one state struct)  <-- worker threads, over mpsc channels
        |
        v
  ui/renderer.rs -> ui/screens/, ui/popups/      reads App, draws a frame
        |
        v
 terminal (ratatui)

  events/  + tracer.rs    every key, screen change and action -> log file and the in-app debugger
  settings.rs             pane splits and options, loaded at start, saved on quit
```

- `git/` has no idea a TUI exists. Functions take a repository path and
  plain arguments and return `git::Result<T>`.
- `app/` owns all state and every decision. Behaviour is split by
  feature: each file under `app/features/` adds its own `impl App { … }`
  block, so "everything about stashes" is one file per layer.
- `ui/` only draws. It reads `App` and never calls into `git/`.
- Geometry lives in `app/layout.rs`, not in `ui/`, because mouse
  hit-testing and rendering must agree on where each pane is.

## Modules

| Path | What it holds |
|---|---|
| `main.rs` | CLI (`clap`), the non-interactive `--hash … --mod-*` path, terminal setup and restore, the event loop, the top-level `Error` |
| `git/history.rs` | loading commits (streamed `git rev-list`), commit details, diff and stats, edit (`modify_commit`), squash, file search |
| `git/reorder.rs` | moving a commit: a scripted `git rebase -i`, continue, abort |
| `git/staging.rs` | status, stage/unstage, commit, per-file diff, the working tree patch (libgit2) |
| `git/stash.rs` | stash list, patch, stat, apply/pop/drop (git command) |
| `git/graph.rs` | classic lane engine; `compute_merge_points` (which merge brought a commit in) |
| `git/tree_layout.rs` | gitk-style layout for the focused view |
| `git/graph_engine.rs` | `GraphEngine` trait; `ClassicEngine`, `TreeEngine` |
| `git/models.rs`, `git/error.rs` | plain types (`CommitInfo`, `FileStatus`, …); `git::Error` |
| `app/state.rs` | the `App` struct, construction, cross-cutting helpers |
| `app/nav.rs` | `Navigator`: the stack of modes |
| `app/models.rs` | `Mode`, `ConfirmAction`, `CommitListView`, `VisibleRow`, … |
| `app/layout.rs` | pane regions, popup rectangles, hit tests |
| `app/input/` | `dispatch.rs` (keys), `mouse.rs`, `selection.rs` (mouse text selection), `clipboard.rs` |
| `app/features/` | one file per feature: `commit_list`, `history_load`, `search`, `edit`, `squash`, `move_commit`, `staging`, `working_tree`, `stash`, `context_menu`, `help` |
| `app/editor/` | the vim-like multi-line editor shared by every message popup |
| `ui/renderer.rs` | picks what to draw for the current mode |
| `ui/screens/` | `commit_list`, `details`, `diff`, `staging`, `stash`, `debugger` |
| `ui/popups/` | `confirm`, `edit`, `search`, `squash_msg`, `context_menu`, `help` |
| `ui/widgets/` | `text_input`: the single-line input used by search and edit |
| `events/` | `AppEvent`, `EventBus` (ring buffer of the last 500 events) |
| `tracer.rs` | `tracing` setup: log file plus a layer that forwards into the event bus |
| `settings.rs` | global and per-repository settings ([SETTINGS.md](SETTINGS.md)) |
| `stats/` | timing collector and report behind `--stats` |
| `automation/` | `--auto`: scripted key input from a string or a file |

Every `mod.rs` is an index: declarations and re-exports only.

## Modes and navigation

The app is always in exactly one `Mode` (`app/models.rs`): `Normal`
(the commit list), `Search`, `Edit`, `Staging`, `Squash`,
`SquashMessage`, `Move`, `Stash`, `EditCommitMessage`,
`EditCommitField`, `Debugger`, `Confirm(action, yes_selected)`.

Modes form a stack in `Navigator` (`app/nav.rs`). Opening a screen or
popup pushes; Esc or Cancel pops back to whatever opened it; finishing
a whole flow (commit created, squash done) resets to the commit list.
No call site has to remember where to return to.

`app/input/dispatch.rs` reads the top of the stack and hands the key to
that mode's handler. The renderer does the same for drawing. Adding a
screen means one `Mode` variant, one handler, one render function.

## Loading history

History is loaded on a worker thread so the first frame appears at
once, even on a repository with hundreds of thousands of commits
(`app/features/history_load.rs`, `git/history.rs`):

1. `App::new` starts the loader and returns with an empty list.
2. The loader reads `git rev-list` output as a stream and sends three
   batches over an mpsc channel: a first page (300 commits), an initial
   batch (5 000), then the full history.
3. The event loop drains the channel between frames
   (`maybe_finish_history_load`) and swaps the list in, keeping the
   selection on the same commit.
4. Two more threads run beside it: one counts the commits
   (`rev-list --count`) so the total is known early, one writes git's
   commit-graph file so the next start is faster.

The rank table for the default view is built on the loader thread, so
the UI thread never does a pass over the whole history. Graph prefixes
(the lane drawing) are computed lazily for the rows on screen and
extended as the user scrolls: keeping them for every commit costs
hundreds of megabytes on a large repository for rows nobody looks at.

Per-commit stats and diffs are also lazy: loaded when a commit is
selected, not when it is listed.

## The commit graph

Three list views, cycled with `l` (`L` backwards): focused (gitk-style, the default),
graph (classic lanes) and simple (no graph). The two graph views sit
behind one trait, `GraphEngine` (`git/graph_engine.rs`), so the list
code does not care which is active:

- `ClassicEngine` (`git/graph.rs`): each branch takes the first free
  column.
- `TreeEngine` (`git/tree_layout.rs`): lanes ordered by a rank table,
  long-range merges cut with arrows instead of crossing the screen.

Both have a collapse-aware pass, because a merge's side branch can be
folded away and the lanes of the visible rows must still connect.

"Which merge brought this commit in" (`m`) is one pass over the loaded
history (`compute_merge_points`): mark the first-parent chain of HEAD,
then flood-fill ancestors from each chain commit, oldest first. The
README explains the idea with a picture.

## Rewriting history

Three ways, chosen by what can go wrong:

| Action | How | Why |
|---|---|---|
| Edit message, author, date | libgit2: recreate the commit and every descendant with the same trees | no patch is re-applied, so it cannot conflict |
| Squash | libgit2, same technique | a contiguous range, trees kept: cannot conflict |
| Move a commit | a real `git rebase -i` with a scripted todo | reordering re-applies patches and can conflict; git's own rebase leaves the standard state behind, so the user can finish with ghh or with plain git |

Rules that hold for all of them:

- Nothing is rewritten without a `Mode::Confirm`. Actions that destroy
  something (`ConfirmAction::is_dangerous`) show DANGER and default to
  No. `--yes` skips the prompts and exists for scripts and tests.
- The branch ref is moved last, after every new commit exists.
- Preconditions are checked first and reported as a `git::Error`
  variant: clean working tree for a move, no merge commits in a squash,
  a contiguous range.
- Stash pop and drop check that `stash@{n}` still points at the hash
  that was listed, because the index shifts when another terminal adds
  a stash.

The same edit is available without the UI: `ghh --hash <h>
--mod-commit/--mod-author/--mod-email/--mod-date`.

## libgit2 and the git command

Both are used on purpose.

- libgit2 (`git2`): reading objects, diffs, status, staging, and the
  rewrites above. In-process, no parsing.
- The `git` command: streaming `rev-list` for the history (faster than
  walking in-process on large repositories and easy to read in
  batches), `rebase`, `stash`, file-history search, `commit-graph
  write`. These are things git does better or that libgit2 does not
  offer.

So `git` must be on `PATH` at run time. `git/reorder.rs` sets
`GIT_EDITOR=true` so a rebase never opens an editor behind the TUI.

## Errors

Each layer that can fail has a `thiserror` enum: `git::Error`,
`automation::Error`, and `main.rs` wraps both plus terminal I/O. Inside
the UI an error never ends the program: the feature code turns it into
the status line (`"Move error: …"`). Only start-up failures and the
non-interactive path reach `main`, which prints the message and exits
with code 1.

Settings are the exception by design: an unreadable or corrupt file
falls back to defaults and a failed save is ignored. Settings must
never block start or exit.

## Events, logging, debugger

Every key press, screen change and action is an `AppEvent` sent through
the `EventBus` (`events/`). The bus keeps the last 500 in memory for
the in-app debugger (`D`) and writes each one through `tracing`.
`tracer.rs` sets up the log file and a layer that forwards ordinary
`tracing::info!` calls back into the bus, so the debugger and the file
show the same stream. The bus's own log target is filtered out of that
layer to avoid a loop.

Nothing may print to stdout or stderr while the TUI is up; it would
corrupt the screen.

## Settings

`settings.rs`. A global file `~/.config/ghh/ghh-settings.json` and an
optional `.ghh-settings.json` in the repository. Merge order, later
wins, field by field: built-in defaults, global `defaults`, global
`repos[<name>]`, the local file. Pane splits are stored as percentages
so a saved layout fits any terminal size. Every key is described in
[SETTINGS.md](SETTINGS.md).

## Terminal handling

`main.rs` enters raw mode and the alternate screen, enables mouse
capture and bracketed paste, and restores all of it through a guard on
every exit path, including SIGINT, SIGTERM, SIGQUIT and SIGHUP. A
watchdog ends the process if the event loop stops responding for five
minutes, so a hung instance cannot hold a terminal forever. `--ascii`
(or `GHH_ASCII=1`) draws the graph without Unicode box glyphs.

## Testing

Unit tests sit in the file they test. Three kinds:

- pure logic: layout math, graph engines, key parsing, search queries;
- git operations against a throw-away repository created in a temp
  directory (stash, reorder, history);
- screens: build an `App` on a temp repository, feed keys, render into
  ratatui's test backend and assert on the buffer.

`--auto` replays keys against the real binary for end-to-end runs, and
`--stats` prints timings on exit; the `just bench*` recipes combine
them. See [DEVELOPMENT.md](DEVELOPMENT.md).

## Known shortcuts

- `build.rs` shells out to `git` and `date` for the version stamp;
  outside a checkout the hash is `unknown`.
- History is held in memory as one `Vec<CommitInfo>`. Fine at 500k
  commits; there is no paging to disk.
- Search runs over the loaded list, so it sees what the loader has
  delivered so far.
- Moving a commit is limited to commits above the newest merge on the
  current branch.

## Not built

- Remotes: no fetch, pull or push.
- Branch management, merging, cherry-picking.
- Hunk- or line-level staging: whole files only.
- Windows support (not tried).
