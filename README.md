# oxidows

Default Windows apps rewritten in Rust, aiming to start faster and use less memory than the originals.
Clean-room: behaviour is matched by observation; no Microsoft code, icons or resources are used.
The apps draw text and in-app icons with the fonts Windows already has (Segoe UI, Segoe Fluent Icons),
read from `%WINDIR%\Fonts` at run time and never bundled; the app icons are our own
([`scripts/make-icons.py`](scripts/make-icons.py)).

| App | Status | Path |
| --- | --- | --- |
| Task Manager | v0.1: Processes + Performance tabs | [`apps/taskmgr`](apps/taskmgr) |
| File Explorer | v0.1: browse, search, copy/move/delete, clipboard shared with Windows | [`apps/explorer`](apps/explorer) |

## Download

Get `oxi-taskmgr.exe` and `oxi-explorer.exe` from the [latest release](https://github.com/vuuminhphuoc/oxidows/releases/latest)
(Windows 10/11 x64, no installer). The exe is not code-signed yet, so SmartScreen may ask you to
confirm on first run. Run it as administrator to see and end elevated processes.

## Use them as the default apps

Each app can take over the Windows shortcuts that open its original, and give them back:

| App | Where | Takes over | Needs |
| --- | --- | --- | --- |
| Task Manager | Settings > Default app > Set as default | Ctrl+Shift+Esc, Ctrl+Alt+Del > Task Manager, taskbar right-click > Task Manager, anything that runs `taskmgr.exe` | administrator (Windows asks) |
| File Explorer | `…` > Options > Set as default | Win+E, opening a folder or drive from the desktop, Start, Run or another app | nothing, your account only |

Explorer never replaces the shell: the taskbar, Start and the desktop stay File Explorer's.
**Restore** in the same place puts back exactly what was there before (Windows' own app, or a tool
such as Process Explorer or Files that had it), and removes every registry entry the app made.
Move or delete the exe only after restoring. To undo without opening the app, run
`oxi-taskmgr.exe --restore-default` or `oxi-explorer.exe --restore-default` (`--set-default` does
the opposite; add `--quiet` to skip the message box).

| Processes | Performance |
| --- | --- |
| ![Processes tab](docs/images/taskmgr-processes.png) | ![Performance tab](docs/images/taskmgr-performance.png) |

<sub>Screenshots from the Linux development build, with the Segoe fonts of a Windows 11 PC.</sub>

## One look for every app

All apps share [`crates/ui-common`](crates/ui-common): a Windows 11 theme (Mica-grey chrome, the
user's accent color, 4 px corners, Segoe UI), light/dark following the system or the app's setting,
one icon set (`ui_common::Icon`), the same navigation rows, toolbar buttons and page titles, and app
icons drawn as one family. A new app calls `ui_common::setup(ctx, theme)` once and uses those widgets.

Startup time, memory, idle CPU and feature coverage of every release next to the Windows originals: [docs/benchmarks.md](docs/benchmarks.md).

## Layout

- `apps/<name>`: one executable per rewritten app.
- `crates/<app>-*`: libraries for that app (`taskmgr-core` math, `taskmgr-sys` OS readers).
  Code shared by several apps goes in an unprefixed crate when a second app needs it.
- `crates/ui-common`: the shared look (theme, fonts, icons, widgets, app icons) and renderer choice.

## Build

```sh
cargo test --workspace
cargo build --release -p oxi-taskmgr
cargo build --release -p oxi-explorer
```

CI (`.github/workflows/ci.yml`) runs clippy, tests and a release build on `windows-latest`, then
[`scripts/smoke.ps1`](scripts/smoke.ps1) launches every `oxi-*.exe` and records time to window, private
working set, idle CPU and a screenshot (job summary + `smoke-results` artifact). Name a new app's
binary `oxi-<app>` and it is smoke-tested and released automatically.
Pushing a tag such as `v0.1.0` publishes the exe as a GitHub Release.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
