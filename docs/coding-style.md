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

Bad: putting the dispatch logic itself inside `input/mod.rs`.

Why: `mod.rs` files all share the same filename — a folder full of logic-heavy
`mod.rs` files is painful to navigate in editors and grep output. A named file
(`dispatch.rs`) says what it contains.
