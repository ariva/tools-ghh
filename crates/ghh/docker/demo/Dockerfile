# Demo recording for ghh: a rust stage builds the binary, the VHS image
# (ttyd + ffmpeg + headless Chrome) plays demo.tape and writes the GIF.
# `just demo`. The tape opens a repository that `make-repo` builds inside
# the container, so nothing on screen comes from a real project.
FROM rust:1-slim-bookworm AS build
# libgit2 is compiled by libgit2-sys (C compiler comes with the image) and
# links OpenSSL; git is what build.rs asks for the commit hash (.git is not
# in the build context, so the stamp says "unknown": the demo never shows it)
RUN apt-get update \
    && apt-get install -y --no-install-recommends git libssl-dev pkg-config \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY . .
RUN cargo build --release -p tool-ghh --locked

FROM ghcr.io/charmbracelet/vhs
RUN apt-get update \
    && apt-get install -y --no-install-recommends git libssl3 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --shell /bin/bash dev
COPY --from=build /src/target/release/ghh /usr/local/bin/ghh
COPY crates/ghh/docker/demo /opt/demo/
WORKDIR /out
ENTRYPOINT ["vhs", "/opt/demo/demo.tape"]
