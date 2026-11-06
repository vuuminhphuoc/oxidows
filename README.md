# oxidows

Default Windows apps rewritten in Rust, aiming to start faster and use less memory than the originals.
Clean-room: behaviour is matched by observation; no Microsoft code, icons or resources are used.

| App | Status | Path |
| --- | --- | --- |
| Task Manager | v0.1: Processes + Performance tabs | [`apps/taskmgr`](apps/taskmgr) |

## Layout

- `apps/<name>`: one executable per rewritten app.
- `crates/<app>-*`: libraries for that app (`taskmgr-core` math, `taskmgr-sys` OS readers).
  Code shared by several apps goes in an unprefixed crate when a second app needs it.

## Build

```sh
cargo test --workspace
cargo build --release -p oxi-taskmgr
```

CI (`.github/workflows/ci.yml`) runs clippy, tests and a release build on `windows-latest`.
