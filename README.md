# oxidows

Default Windows apps rewritten in Rust, aiming to start faster and use less memory than the originals.
Clean-room: behaviour is matched by observation; no Microsoft code, icons or resources are used.

| App | Status | Path |
| --- | --- | --- |
| Task Manager | v0.1: Processes + Performance tabs | [`apps/taskmgr`](apps/taskmgr) |

## Download

Get `oxi-taskmgr.exe` from the [latest release](https://github.com/vuuminhphuoc/oxidows/releases/latest)
(Windows 10/11 x64, no installer). The exe is not code-signed yet, so SmartScreen may ask you to
confirm on first run. Run it as administrator to see and end elevated processes.

| Processes | Performance |
| --- | --- |
| ![Processes tab](docs/images/taskmgr-processes.png) | ![Performance tab](docs/images/taskmgr-performance.png) |

<sub>Screenshots from the Linux development build.</sub>

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
Pushing a tag such as `v0.1.0` publishes the exe as a GitHub Release.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
