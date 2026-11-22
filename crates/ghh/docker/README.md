# Docker images for ghh

Images for things the host should not do itself. Each subfolder is one
image with its own Dockerfile and a `just` recipe that builds and runs
it; none of them is needed to build or install ghh.

| Folder | What it does | Recipe |
|---|---|---|
| `demo/` | records `docs/assets/demo.gif`: ghh on a made-up repository, through [VHS](https://github.com/charmbracelet/vhs) | `just demo` |

The build context is the repository root (`docker build -f
crates/ghh/docker/<image>/Dockerfile .`), so an image can compile ghh
from source in a `rust` stage; `.dockerignore` at the root keeps
`target`, `.git` and `.private` out of it.

## demo/

| File | Role |
|---|---|
| `Dockerfile` | `rust:1-slim-bookworm` (plus `git`, `libssl-dev`, `pkg-config`) builds `ghh --release` with the toolchain `rust-toolchain.toml` pins; `ghcr.io/charmbracelet/vhs` (ttyd, ffmpeg, headless Chrome) gets the binary, `git`, a `dev` user and this folder as `/opt/demo`; the entrypoint is `vhs /opt/demo/demo.tape` |
| `demo.tape` | the script VHS plays: terminal size, theme, every key pressed and how long the result stays on screen; `Output demo.gif` is relative to `/out`, the volume `just demo` mounts on `docs/assets` (a second `Output demo.mp4` line is all an MP4 needs) |
| `enter` | the tape's first, hidden command: runs `make-repo` as `dev`, then becomes a `dev` shell |
| `make-repo` | builds `~/shop-api`: three merged pull requests, two tags, three plain commits on top (the ones `E` can move), two stashes; names, code and dates are made up, and the dates are fixed |

What the tape shows, in order: the diff pane (`v`), the merge that
brought a commit to main and the way back (`m`, `b`), drilling into that merge (`Enter`,
`Backspace`), collapsing it (`Space`), search and filter (`/`, `f`,
`F`), the stash browser (`S`), and moving a commit (`E`, with its
DANGER confirmation answered `y`).

Nothing on screen is real: the repository is built inside the container
on every run, the user is `dev`, the hostname is `demo-box`
(`--hostname`), and ghh's settings and log are the container's. The
keys are typed by VHS into a real terminal, not fed through `--auto`,
so the recording is what a user would see. `.git` is outside the build
context, so `ghh --version` in the image says the commit is `unknown`;
the tape never shows it. A last `docker run` hands the GIF to the
calling user; the volume is written as root otherwise.

Row positions in the tape (`Down 5`, the two `Down` in move mode) depend
on the commits `make-repo` creates: change one, check the other.

Iterating on the tape or the repository without a rebuild (the image
already has the binary):

```bash
docker run --rm --hostname demo-box -v "$PWD/docs/assets:/out" \
    -v "$PWD/crates/ghh/docker/demo:/opt/demo:ro" ghh-demo
```
