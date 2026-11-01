set shell := ["bash", "-cu"]

# list available recipes
default:
    @just --list

# debug build -> target/debug/ghh; extra flags go to cargo
build *args:
    RUSTC_WRAPPER= cargo build -p tool-ghh {{args}}

# lint, then optimised build -> target/release/ghh
release *args:
    just lint
    just build --release {{args}}

# format the code
fmt:
    RUSTC_WRAPPER= cargo fmt

# clippy, warnings are errors
lint:
    RUSTC_WRAPPER= cargo clippy --workspace --all-targets --all-features -- -D warnings

# unit tests
test:
    RUSTC_WRAPPER= cargo test --workspace

# everything CI runs
check: lint test

# run the tool from source: just run ~/some/repo
run *args:
    RUSTC_WRAPPER= cargo run -q -p tool-ghh -- {{args}}

# install into ~/.cargo/bin, overwriting any existing binary, deps pinned to Cargo.lock
install *args:
    RUSTC_WRAPPER= cargo install --path crates/ghh --force --locked {{args}}

# Synthetic engine/search benches (machine-independent ceilings).
bench:
    RUSTC_WRAPPER= cargo test --release bench_engines_and_search_synthetic -- --ignored --nocapture

# Stage costs against the big local test repo (skips if absent).
bench-repo:
    RUSTC_WRAPPER= cargo test --release perf_big_repo_stages -- --ignored --nocapture

# End-to-end on the big test repo: load, jump bottom/top, search, filter; prints --stats.
bench-e2e:
    RUSTC_WRAPPER= cargo build --release
    script -qec "./target/release/ghh --path /home/arunas/dev/ariva/repos/big-dummy-repo --auto 'SLEEP 6000,SLEEP 200,SLEEP 200,SLEEP 200,UPPER_G,SLEEP 300,LOWER_G,SLASH,LOWER_F,LOWER_I,LOWER_X,ENTER,SLEEP 400,LOWER_F,SLEEP 300,LOWER_Q' --stats" /tmp/ghh-bench-e2e.log > /dev/null
    @grep -aE "ms=" /tmp/ghh-bench-e2e.log | grep -avE "count=0" | sort -u | tail -30
