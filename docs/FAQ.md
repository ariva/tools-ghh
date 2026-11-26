# ghh FAQ

Questions and the key or option that answers each. Keys are typed in
the commit list unless a row says otherwise. The same keys are listed
inside the tool: `h` or `?` opens help for the screen you are on. The
"More" column links to the longer explanation where there is one.

The same list at the command line, without opening a repository:
`ghh faq`. Words narrow it: `ghh faq stash`, `ghh faq move conflict`
(every word must appear in the section title, question or answer),
`ghh faq 3` (a section), `ghh faq 3.7` (one row), `ghh faq -k E` (rows
whose answer names the key `E`, case kept). `--json` prints the rows as
JSON. A new row goes at the end of its section: the command reads this
file, and `just check` fails on a number out of order.

## 1. Looking around

| # | Question | Answer | More |
|---|---|---|---|
| 1.1 | How do I open a repository? | `ghh` in it, or `ghh ~/some/repo` | [README](../README.md#repository-path) |
| 1.2 | How do I move through the list? | `j`/`k` or arrows, `PgUp`/`PgDn` (or `P`/`p` on a keyboard without them), `g`/`G` for top and bottom | |
| 1.3 | How do I see what a commit changed? | `v` shows the diff pane; `Tab` moves focus to it, `Esc` hides it | |
| 1.4 | How do I change how the graph is drawn? | `l` cycles focused (gitk-style), graph and simple; `L` goes the other way | [architecture](ARCHITECTURE.md#the-commit-graph) |
| 1.5 | How do I hide the commits of a merged branch? | `Space` or `←`/`→` on the merge collapses and expands it | |
| 1.6 | How do I see only what one merge brought in? | `Enter` on the merge drills in; `Backspace` goes back | |
| 1.7 | Which pull request did this commit come in with? | `m` jumps to the merge that brought it to the main line; `b` goes back to the commit you came from | [README](../README.md#find-the-pr-merge-of-any-commit-m) |
| 1.8 | How do I jump between branch tips and tags? | `{` and `}`: previous and next labelled commit | |
| 1.9 | How do I go to a commit's parent or child? | `[` first parent, `]` nearest child | |
| 1.10 | How do I list commits strictly by date? | `o` switches between topological order and date order | |
| 1.11 | How do I resize the panes? | drag a border with the mouse; the layout is saved on quit | [settings](SETTINGS.md) |
| 1.12 | How do I copy a hash or a message? | `.` or right-click opens the pane's menu | |
| 1.13 | How do I see the changes of one file of a commit only? | `v`, then click the file in the Files column left of the diff (or `Tab` to the column, `j`/`k`). `All files` on top shows everything again | [README](../README.md#changed-files-beside-the-diff) |

## 2. Finding commits

| # | Question | Answer | More |
|---|---|---|---|
| 2.1 | How do I search? | `/`, fill one or more fields, `Enter` | |
| 2.2 | What can I search by? | quick text, hash, message, author, date range, file, branch, tag | the search popup's hint line |
| 2.3 | How do I go to the next match? | `n` next, `N` previous | |
| 2.4 | How do I show only the matches? | `f` filters the list; `f` again removes the filter | |
| 2.5 | How do I drop the search completely? | `F` | |
| 2.6 | How do I clear one search field? | `Alt+Backspace` (mac: `Option+Delete`) or `Ctrl+U` | [README](../README.md#search-clearing-an-input-field) |
| 2.7 | Which commits touched this file? | `/`, then the File field; it completes paths with `Tab` | |

## 3. Changing history

Every row here rewrites commits: the changed commit and everything
above it get new hashes, and a branch that was already pushed needs a
force-push afterwards. Each asks before doing anything.

| # | Question | Answer | More |
|---|---|---|---|
| 3.1 | How do I fix a commit message? | `e`, `Enter` on the message, edit, save | |
| 3.2 | How do I change the author or the date of a commit? | `e`, move to the field with `Tab`, type, `Enter` | |
| 3.3 | How do I do that from a script? | `ghh --hash <hash> --mod-commit "text"` (also `--mod-author`, `--mod-email`, `--mod-date`); `-y` skips the question | |
| 3.4 | How do I squash commits into one? | `r` on the commit to keep, `Space` on the ones to fold in, `Enter`, edit the message | |
| 3.5 | Why does squash refuse my selection? | the commits must be a contiguous range on the current branch and none may be a merge | [architecture](ARCHITECTURE.md#rewriting-history) |
| 3.6 | How do I reorder commits? | `E`, slide the commit with `↑`/`↓`, `Enter` | [README](../README.md#move-a-commit-up-or-down-e) |
| 3.7 | A move stopped on a conflict. Now what? | fix the files, stage them, press `E`: continue or abort. Plain `git rebase --continue` / `--abort` works too | [README](../README.md#move-a-commit-up-or-down-e) |
| 3.8 | Why can't I move this commit? | only commits above the newest merge can move, and the working tree must be clean | |
| 3.9 | I rewrote something by mistake. How do I get it back? | `git reflog` in a shell shows the branch before the change; `git reset --hard <that hash>` returns to it | |

## 4. Staging and stashes

| # | Question | Answer | More |
|---|---|---|---|
| 4.1 | How do I commit my changes? | `s`, `Space` on each file (it moves on to the next one; `a` stages all), `Enter`, type the message in the editor, Save. The line `Enter: write the message and commit` under the panes shows when something is staged | |
| 4.2 | How do I unstage a file? | select it in the staged group on top and press `Space`; `u` unstages everything | |
| 4.3 | How do I write a longer commit message? | the editor `Enter` opens in staging takes as many lines as you like; first line is the subject | |
| 4.4 | Can I stage part of a file? | no, whole files only | [not built](ARCHITECTURE.md#not-built) |
| 4.5 | How do I look at my stashes? | `S` | [README](../README.md#browse-stashes-s) |
| 4.6 | How do I bring a stash back? | in the stash screen: `a` applies and keeps it, `p` applies and removes it | [README](../README.md#browse-stashes-s) |
| 4.7 | How do I delete a stash? | `d` in the stash screen; it asks, and the default is No | |
| 4.8 | What is row 0, "Uncommitted changes", on top of the list? | everything not committed yet: staged, unstaged and untracked files. Select it to see the files in Details, `Tab` to Details scrolls a long file list, `v` shows their changes, `Enter` opens staging. No such row when the working tree is clean | [README](../README.md#uncommitted-changes-row-0) |
| 4.9 | In what order are uncommitted files listed? | staged first, then unstaged, then untracked (`?`), by path inside each group; the same in staging, in row 0's Files column and Details, and in its diff. A file you stage moves up into the staged group; the cursor goes to the file that was under it | |
| 4.10 | How do I move fast through a long file list or diff in staging? | `PgDn`/`PgUp` (or `p`/`P`) page the pane that has focus, `g`/`G` jump to its start and end; `Tab` switches between file list and diff | |
| 4.11 | How do I leave the staging screen? | `Esc` or `Backspace` from the file list; from the diff either one goes to the file list first | |

## 5. Options and files

| # | Question | Answer | More |
|---|---|---|---|
| 5.1 | Where are my settings kept? | `~/.config/ghh/ghh-settings.json`; a repository can override with `.ghh-settings.json` | [settings](SETTINGS.md) |
| 5.2 | How do I keep settings for one repository only? | start once with `--save-local-settings`; the file then keeps updating by itself | [settings](SETTINGS.md) |
| 5.3 | Where is the log? | `/tmp/ghh/logs.txt` unless `--logs <file>` or the `logs` setting says otherwise; `off` disables it | [README](../README.md#logging---logs) |
| 5.4 | How do I get more detail in the log? | `RUST_LOG=debug ghh` | [README](../README.md#logging---logs) |
| 5.5 | How do I see what the app is doing right now? | `D` opens the event log; `D` or `Esc` closes it | |
| 5.6 | How slow was that? | `ghh --stats` prints timings on exit | [README](../README.md#statistics---stats) |
| 5.7 | How do I replay keys for a demo or a test? | `ghh --auto "ARROW_DOWN,ENTER"` or `--auto keys.txt`, with `--auto-delay <ms>` | [README](../README.md#automation) |
| 5.8 | Which version is this? | `ghh --version` | |
| 5.9 | How do I read this FAQ in the terminal? | `ghh faq`; `ghh faq stash` or `ghh faq 3.7` narrows it, `ghh faq -k E` asks what a key does | [README](../README.md#faq-at-the-command-line-ghh-faq) |

## 6. When something looks wrong

| # | Question | Answer | More |
|---|---|---|---|
| 6.1 | The screen is garbled. | `Ctrl+L` repaints it | |
| 6.2 | The graph lines are broken or misaligned. | `ghh --ascii` (or `GHH_ASCII=1`) draws them with plain characters; otherwise try another font | |
| 6.3 | The list is short right after start. | history loads in the background; the status line says so and the rest arrives by itself | [architecture](ARCHITECTURE.md#loading-history) |
| 6.4 | Search did not find an old commit just after start. | it searches what has been loaded so far; search again when loading has finished | [architecture](ARCHITECTURE.md#known-shortcuts) |
| 6.5 | `Ctrl+Backspace` does not clear a field. | most terminals do not report that chord; use `Alt+Backspace` or `Ctrl+U` | [README](../README.md#search-clearing-an-input-field) |
| 6.6 | My terminal is broken after ghh was killed. | run `reset` | |
| 6.7 | "stash list changed outside the app" | another terminal added or removed a stash; press `r` and try again | |
| 6.8 | The help popup is cut off at the bottom. | it scrolls when the terminal is too short for it: `j`/`k`, `PgDn`/`PgUp` (or `p`/`P`), `g`/`G`, or the mouse wheel; the title says so and the right border shows where you are | |

## Keeping this file and the help popup in step

The keys above are the ones in `crates/ghh/src/app/features/help.rs`.
When a key is added or changed, change it there, here, and in the
README section that describes the feature.
