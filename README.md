# GIT Handy Helper

## Install

```sh
cargo install --git https://github.com/ariva/tools-ghh tool-ghh --locked   # from GitHub, no checkout
just install                # from a checkout: cargo install --path crates/ghh --force --locked
```

The Cargo package is `tool-ghh`; the binary is `ghh`.

## Repository path

By default `ghh` opens the repository in the current directory.
Pass a path directly, like any editor:

```sh
ghh ~/dev/some-repo
```

`--path <dir>` (and its alias `--repo`) remain supported for existing scripts;
passing both the positional path and `--path` is an error.

## Find the PR merge of any commit (`t`)

Press `t` on any commit to jump to the merge that brought it onto the mainline —
for GitHub-style flows that's literally the "Merge pull request #NNNN …" commit.
The Details pane also shows it as a `Merged:` line without jumping. This is the
classic "which merge brought this commit in" problem (`git-when-merged` in tool
form); neither gitk nor guitar has it built in.

**The insight:**

```text
main (first-parent chain):  M3 ── M2 ── M1 ── M0(root)
                            │
                            ╰─ M2 = "Merge pull request #69541..."
                                  ╲
                                   c1 ── c2 ── C (your commit)
```

The PR-merge of commit `C` is the **oldest commit on the first-parent chain of
HEAD that has `C` as an ancestor**. The first-parent chain is the mainline as
merges see it; the first chain commit that "contains" C is exactly the merge
that brought it in.

**The algorithm** (`compute_merge_points` in `crates/ghh/src/git/graph.rs` — one pass,
answers it for ALL commits at once):

1. Walk `parent_indices[0]` from HEAD → mark the mainline chain (bitset).
2. Iterate chain commits **oldest → newest**; from each, flood-fill ancestors
   through *all* parents, stamping `merged_via[commit] = that chain commit` —
   skip already-stamped commits (they were reached by an older mainline point
   first, which is the correct answer).
3. Done: `merged_via` maps every commit to its introducing mainline commit.
   O(commits + edges) — one ~50–100 ms pass on a 568k-commit repo, computed
   lazily on first use, cached until reload.

`t` works from any view: drill scopes are exited, an active filter is dropped,
and collapsed branches are expanded when the merge lives outside them (the
status line says which). Commits on the mainline itself report "committed
directly". Pairs well with `Enter` (drill into everything that PR brought) and
`[` `]` `{` `}` topology jumps.

## Move a commit up or down (`m`)

Press `m` on a commit, slide it with `↑`/`↓` (live preview, git untouched), `Enter`
to apply, `Esc` to cancel. Applying asks for confirmation: it **rewrites history** —
every commit from the lowest moved row up to HEAD gets a new hash (force-push needed
if already pushed).

- Runs a real `git rebase -i` with a scripted todo (`crates/ghh/src/git/reorder.rs`), with
  `--committer-date-is-author-date` so rewritten commits don't all get "now".
- **Conflict:** the rebase stays in progress and a popup offers *Yes: continue* /
  *No: abort (original history restored)* / *Esc: decide later*. Fix the files, stage
  them (`s` or `git add`), press `m` to reopen the popup. Plain `git rebase
  --continue` / `--abort` in a shell works too.
- Limits: only commits above the newest merge on the current branch; clean working
  tree required; not inside a drill view or search filter.

Related keys: `v` toggles the diff pane, `c` / `C` cycle the list view forward / back.

## Browse stashes (`S`)

Press `S` in the commit list. Same shape as the main screen: stash list on the left,
the selected stash's full patch on the right (untracked files from `stash -u`
included), details at the bottom (ref, hash, base commit, date, author, message,
per-file stat).

| Key | Action |
|---|---|
| `j`/`k`, `g`/`G` | select stash — or scroll the changes when that pane has focus |
| `Tab` | switch focus: list / changes |
| `PgUp`/`PgDn` | page through the changes |
| `a` | apply to the working tree, stash kept |
| `p` | pop: apply, then remove (asks first; kept if it conflicts) |
| `d` | drop without applying (DANGER confirm, default No) |
| `r` | reload list |
| `Esc`, `Backspace` | back |

Mouse: click selects / focuses, wheel scrolls the pane under the cursor. Pop and drop
verify the `stash@{n}` still points at the listed stash before acting, so a stash
added from another terminal can't make them hit the wrong one.

## Search: clearing an input field

**Alt+Backspace** clears the field (on mac that's **Option+Delete**, the chord your
fingers already know). Why this one works when Ctrl/Shift+Backspace didn't:
terminals encode Alt+Backspace distinctly as an escape-prefixed byte (`ESC DEL`),
while most terminals collapse Ctrl+Backspace and Shift+Backspace into the same
byte as plain Backspace — the modifier physically never reaches the app.

Full clear roster, in reliability order:

```text
┌────────────────────────────────────┬──────────────────────────────────────────────┐
│               Chord                │                    Works                     │
├────────────────────────────────────┼──────────────────────────────────────────────┤
│ Alt+Backspace (mac: Option+Delete) │ virtually everywhere                         │
├────────────────────────────────────┼──────────────────────────────────────────────┤
│ Ctrl+U                             │ everywhere (plain 0x15)                      │
├────────────────────────────────────┼──────────────────────────────────────────────┤
│ Ctrl+Backspace / Shift+Backspace   │ only where the terminal reports the modifier │
├────────────────────────────────────┼──────────────────────────────────────────────┤
│ ✕ click                            │ everywhere with mouse                        │
└────────────────────────────────────┴──────────────────────────────────────────────┘
```

Discoverability: the popup's hint line opens with "Alt+Backspace/Ctrl+U: clear
field", and the Search help section lists it too.

## Statistics (`--stats`)

Print runtime, event-loop, rendering, commit-count, and application timing statistics when the app exits:

```sh
ghh --stats
```

## Source structure

```
crates/ghh/src/
├── main.rs              CLI argument parsing, TUI initialization, event loop
├── stats.rs             Runtime and application statistics collector
│
├── automation/          Automation / scripted replay
│   ├── mod.rs           Re-exports
│   ├── commands.rs      AutomationCommand enum (data)
│   ├── loader.rs        Script/CSV loader and parser
│   └── keys.rs          Key enum + FromStr parser + unit tests
│
├── git/                 All git operations (no UI dependencies)
│   ├── mod.rs           Re-exports
│   ├── models.rs        Plain data types: CommitInfo, CommitStats, FileStatus, StageState
│   ├── graph.rs         Classic lane engine — first-free-column prefixes, branch commits
│   ├── graph_engine.rs  GraphEngine trait + ClassicEngine/TreeEngine implementations;
│   │                    each commit-list view plugs in through this interface
│   ├── tree_layout.rs   gitk-style engine (focused view): token-ordered lanes, arrow cuts
│   │                    for long-range merges, per-line colors (+ tests)
│   ├── history.rs       load_commits, modify_commit, squash_commits, get_commit_stats
│   ├── reorder.rs       move_commit via scripted `git rebase -i`, continue/abort (+ tests)
│   ├── staging.rs       load_status, stage/unstage helpers, commit_staged, get_file_diff
│   └── stash.rs         list_stashes, patch/stat, apply/pop/drop via git CLI (+ test)
│
├── app/                 Application state and input handling
│   ├── mod.rs           Re-exports
│   ├── state.rs         App struct: shared state, construction, cross-cutting helpers
│   │                    (mode(), commit hash→index lookup, replace_commits)
│   ├── nav.rs           Navigator — stack-based screen/popup navigation (+ tests).
│   │                    push() opens, pop() returns to the opener, reset() ends a flow
│   ├── models.rs        Enums/structs with no heavy logic: ConfirmAction, Mode, EditField,
│   │                    VisibleRow, SquashState
│   ├── input.rs         Global key dispatch; commit-list, confirm and debugger key handlers
│   ├── mouse.rs         Clicks, double-click word selection, drag selection, scrolling
│   ├── layout.rs        Screen geometry: pure region math (+ tests) and popup rects
│   ├── commit_list.rs   Selection movement, lazy stats/diff loading, branch collapsing,
│   │                    graph-prefix computation
│   ├── history_load.rs  Background progressive history loading over mpsc channels
│   ├── search_ops.rs    Search execution + autocomplete streaming (+ date-range tests)
│   ├── selection.rs     Mouse text selection: panel bounds, text extraction
│   ├── context_menu.rs  Right-click / `.` menu: actions, execution, copy formatting
│   ├── clipboard.rs     arboard + CLI-fallback clipboard, paste routing
│   ├── help.rs          Help popup content per screen
│   ├── editor.rs        Generic vim-like MultilineEditor (shared by all message editors)
│   ├── editor_keys.rs   The one shared vim key-binding handler for those editors (+ tests)
│   ├── edit.rs          EditState + commit-edit popup (↑↓ field nav, Enter opens editor)
│   ├── staging.rs       StagingState / StagingFocus + staging screen + commit message editor
│   ├── squash.rs        SquashMessageState + squash picker and message editor
│   ├── move_commit.rs   Move mode: ↑/↓ preview, confirm, conflict continue/abort popup
│   └── stash.rs         StashState + stash screen keys, apply / pop / drop
│
├── events/              Event bus + event model (no dependencies on app/ or ui/)
│   ├── mod.rs           Re-exports
│   ├── models.rs        AppEvent, ScreenId (9 human-readable IDs), ActionKind, TraceLevel;
│   │                    format_key_event() — key → display string; EventTime (Instant + local wall-clock)
│   └── bus.rs           EventBus (500-event ring buffer, scroll state, emit() logs via tracing);
│                        EventHandler trait — implement to add custom sinks
│
├── tracer.rs            tracing subscriber setup: file writer (tracing-appender) + EventBusLayer
│                        (forwards tracing events → EventBus; filters "events::bus" target to avoid recursion)
│
└── ui/                  Rendering only — no business logic
    ├── mod.rs           Re-exports
    ├── renderer.rs      Top-level render() dispatcher
    ├── helpers.rs       centered_rect(), truncate()
    ├── screens/         Full-screen views
    │   ├── mod.rs       Re-exports
    │   ├── commit_list.rs   Commit list, graph-lane colouring, title bars
    │   ├── details.rs   Details panel + status bar
    │   ├── staging.rs   Staging view (file list + diff + commit message)
    │   └── debugger.rs  Event log viewer (D to open/close from any screen)
    └── popups/          Modal overlays
        ├── mod.rs       Re-exports
        ├── confirm.rs   Confirm dialog
        ├── edit.rs      Commit edit popup
        └── squash_msg.rs    Multi-line editor popup (shared: squash message, commit message, field editor)
```

## Logging (`--logs`)

All events (key presses, screen changes, actions) and `tracing` macro calls are
written to a log file in a consistent format when `--logs` is provided.

```sh
# in terminal 1
RUST_LOG=debug ./ghh --logs /tmp/gh.log

# in terminal 2 — tail the log live
tail -f /tmp/gh.log
```

Log level is controlled by `RUST_LOG` (default: `info`). Module-specific:
```sh
RUST_LOG=ghh::app=debug ./ghh --logs /tmp/gh.log
```

Add tracing calls anywhere in the code:
```rust
tracing::info!("commit modified: {}", hash);
tracing::debug!(screen = "staging", "file staged");
```

## Automation

Replay synthetic key presses for automated testing using `--automation-keys` and `--automation-delay`.

### `--auto-delay <ms>`
Inter-key pause (milliseconds) applied after every key press. Default: `0` (no delay).

```sh
just run --auto-delay 300
```

### `--auto <spec>`
Feed keys from a **comma-separated string** or a **script file**.

#### Inline CSV
Key names separated by commas. `SLEEP <ms>` and `KEY_PRESS_DELAY <ms>` are also accepted inline.

```sh
just run --auto "ARROW_DOWN,ARROW_DOWN,ENTER,SLEEP 500,ESC" --auto-delay 200
```

#### Script file (`.txt`)
Each line is one command. Pass the file path as the value.

```sh
just run --auto crates/ghh/testing/keys1.txt --auto-delay 300
```

Script syntax:

| Command | Example | Description |
|---|---|---|
| `KEY_PRESS <KEY>` | `KEY_PRESS ARROW_DOWN` | Emit a key press |
| `SLEEP <ms>` | `SLEEP 500` | Pause N milliseconds |
| `KEY_PRESS_DELAY <ms>` | `KEY_PRESS_DELAY 800` | Override inter-key delay from here on |
| `# comment` | `# navigate down` | Ignored |

### Key names

| Category | Names |
|---|---|
| Navigation | `ARROW_UP` `ARROW_DOWN` `ARROW_LEFT` `ARROW_RIGHT` `HOME` `END` `PAGE_UP` `PAGE_DOWN` |
| Control | `ENTER` `ESC` `BACKSPACE` `DELETE` `TAB` `SPACE` |
| Symbols | `SLASH` (`/` search) &nbsp; `DOT` (`.` context menu) &nbsp; `QUESTION` (`?` help) |
| Function | `F1` … `F12` |
| Letters | `UPPER_A` … `UPPER_Z` &nbsp; `LOWER_A` … `LOWER_Z` |
| Digits | `DIGIT_0` … `DIGIT_9` |
| Ctrl combos | `CTRL_C` `CTRL_D` `CTRL_Q` `CTRL_S` `CTRL_Z` |

After all demo commands are replayed the app continues in normal interactive mode.
