# GIT Handy Helper
[Install](#install) · [Support](#buy-me-pizzacoffeetea) · [Find PR merge](#find-the-pr-merge-of-any-commit-m) · [Move commit](#move-a-commit-up-or-down-e) · [Files column](#changed-files-beside-the-diff) · [Uncommitted changes](#uncommitted-changes-row-0) · [Stashes](#browse-stashes-s) · [Search](#search-clearing-an-input-field) · [Documentation](#documentation) · [Logging](#logging---logs) · [Automation](#automation) · [License](#license)

`ghh` (git handy helper) is a terminal UI for browsing, searching, analyzing and
rewriting Git history: a commit graph with merged branches you can collapse or drill
into, the diff of any commit, and the merge (pull request) that brought a
commit to the main line in one key. It also rewrites history, asking first:
edit a commit's message, author or date, squash, move a commit up or
down; and it stages, commits and browses stashes.

**An investigative tool for complex searches.** When you need to find out
who changed what, when and where, press `/` and narrow history down by any
combination of quick text, hash, message, author, date range, file, branch
and tag. `n` / `N` walk through the matches, `f` leaves only the matches on
screen, `F` drops the search.

**The fastest Git CLI tool on large repositories.** History streams in while
you already work, rows are built only when they scroll into view, and
whole-graph answers, like the PR merge of every commit, take a single pass.

![ghh on a made-up repository: diff pane, m to the PR merge and b back, drill in, collapse, search and filter, stashes, move a commit](docs/assets/demo.gif)

## Install

It builds the binary from the source and installs it to `~/.cargo/bin/ghh` (or `$CARGO_HOME/bin` if set).

```sh
cargo install --git https://github.com/ariva/tools-ghh tool-ghh --locked   # from GitHub, no checkout
```

Inside codebase checkouts, use `just install` (requires `just` to be installed).
```sh
just install                # from a checkout: cargo install --path crates/ghh --force --locked
```

The Cargo package is `tool-ghh`; the binary is `ghh`.

## Buy me Pizza/Coffee/Tea
* Like what I do? GHH is free and stays free. If it saved you time or allowed you to be more productive, you can support the next release: **[Buy me a coffee ☕](https://buymeacoffee.com/ariva)** — or scan the code.

<p align="center"><a href="https://buymeacoffee.com/ariva"><img src="docs/assets/bmc_qr.png" alt="Buy me a coffee — QR code" width="140"></a></p>

## Support
* Have a question, found a bug, or want to suggest a feature? Send me a message: **arunas.work.hg [ AT ] [g] [m] [a] [i] [l] [.] [c] [o] [m]**.

## Repository path

By default `ghh` opens the repository in the current directory.
Pass a path directly, like any editor:

```sh
ghh ~/dev/some-repo
```

`--path <dir>` (and its alias `--repo`) remain supported for existing scripts;
passing both the positional path and `--path` is an error.

## Find the PR merge of any commit (`m`)

Press `m` on any commit to jump to the merge that brought it onto the mainline —
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

`b` goes back to the commit `m` jumped from, so you can look at the
merge and return. It is one step, not a history: after returning, `b` has
nothing to go back to until the next `m`.

`m` works from any view: drill scopes are exited, an active filter is dropped,
and collapsed branches are expanded when the merge lives outside them (the
status line says which). Commits on the mainline itself report "committed
directly". Pairs well with `Enter` (drill into everything that PR brought) and
`[` `]` `{` `}` topology jumps.

## Move a commit up or down (`E`)

Press `E` (Shift+e) on a commit, slide it with `↑`/`↓` (live preview, git untouched), `Enter`
to apply, `Esc` to cancel. Applying asks for confirmation: it **rewrites history** —
every commit from the lowest moved row up to HEAD gets a new hash (force-push needed
if already pushed).

- Runs a real `git rebase -i` with a scripted todo (`crates/ghh/src/git/reorder.rs`), with
  `--committer-date-is-author-date` so rewritten commits don't all get "now".
- **Conflict:** the rebase stays in progress and a popup offers *Yes: continue* /
  *No: abort (original history restored)* / *Esc: decide later*. Fix the files, stage
  them (`s` or `git add`), press `E` to reopen the popup. Plain `git rebase
  --continue` / `--abort` in a shell works too.
- Limits: only commits above the newest merge on the current branch; clean working
  tree required; not inside a drill view or search filter.

Related keys: `v` toggles the diff pane, `l` / `L` cycle the list view forward / back.

## Changed files beside the diff

With the diff pane open (`v`) the changed files have their own column left of
the diff:

```text
┌ Commit Info ────────────────────────────┐
│ Hash, Author, Date, Message             │
├ Files [3] ──────────┬ Diff: abc123 ─────┤
│>All files           │ @@ -1,4 +1,6 @@   │
│ M README.md +1 -0   │ +line             │
│ M …routes.py +4 -2  │ -line             │
│ A notes.txt +9 -0   │                   │
└─────────────────────┴───────────────────┘
```

- `All files` (shown when the commit changed more than one file): the whole
  diff. The file you are scrolled into is highlighted in the column.
- Click a file: the diff pane shows the changes of that file only, and its
  title names the file. Click `All files` to get everything back, at the file
  you were on.
- By keyboard: `Tab` to the column (border turns yellow), then `j`/`k`,
  `p`/`P`, `g`/`G` move the pick. The wheel over the column moves it one row
  at a time.
- On row 0 a changed file carries `s` (staged, green) or `u` (unstaged, red)
  after its status letter, same colours as the staging screen; an untracked
  file shows a grey `?` instead of a status, as in `git status`.
- A path too long for the column keeps its end, the file name. Drag the
  border between the column and the diff to make it wider or narrower; drag
  the border under Commit Info to change the heights. Both are saved on quit
  (`splits.files_pct`, `splits.diff_info_pct`, see
  [settings](docs/SETTINGS.md)).

## Uncommitted changes (row 0)

When the working tree is not clean, the commit list starts with one extra row,
numbered 0, above the newest commit:

```text
0 ● Uncommitted changes: 1 staged, 2 unstaged, 1 untracked
1 ○ a33788b 2026-03-03 01:27 Ines Duarte  [main] Bump version to 1.2.0.dev0
```

- Select it: the Details pane lists the files, each marked staged, unstaged or
  untracked. More files than fit: `Tab` to the Details pane (its border turns
  yellow), then `j`/`k`, `p`/`P` or PgDn/PgUp, `g`/`G` scroll it; the mouse
  wheel over the pane scrolls it too. `Tab` or `Esc` goes back to the list.
- `v` (diff pane): the files in the Files column (click one to see only its
  changes), their changes beside it — staged first, then unstaged, then untracked files as
  additions. An untracked file over 1 MB or a binary one is named, not shown.
- `Enter` on the row opens staging, same as `s`.
- It is not a commit: edit, squash, move, `m` and `M` do nothing on it, and
  search does not match it.
- Shown in the top-level list only, not inside a drill view or a filtered list.
- The status is read at startup, when you leave the staging or stash screen,
  and after a commit or a history rewrite. A change made in another terminal
  shows after one of those, not by itself.

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

## Documentation

| | |
|---|---|
| [docs/FAQ.md](docs/FAQ.md) | questions and the key that answers each |
| [docs/SETTINGS.md](docs/SETTINGS.md) | every settings key |
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | how the code is put together, module by module, and why |
| [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) | building, testing, adding a feature |
| [docs/CODING-STYLE.md](docs/CODING-STYLE.md) | the rules new code follows |

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

## License

MIT, see [LICENSE](LICENSE).
