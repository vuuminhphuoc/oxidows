# Task Manager (oxi-taskmgr)

A fast Windows Task Manager written in Rust. First version: **Processes** and **Performance** tabs.

## Layout

| Path | What it does |
| --- | --- |
| `crates/taskmgr-sys` | Reads the OS. Windows: one `NtQuerySystemInformation(SystemProcessInformation)` call for every process (no `OpenProcess` per process), `GetSystemTimes` + per-core NT counters, `GlobalMemoryStatusEx`, `GetIfTable2`, PDH for disk and GPU. Linux `/proc` reader is for development only. All `unsafe` lives here. |
| `crates/taskmgr-core` | Platform-independent math: CPU % from deltas, bytes/s, PID-reuse safety, 60-point history rings, sort/filter. Unit-tested. |
| `apps/taskmgr` | egui app. A sampler thread sends an `Arc<Snapshot>` per tick; the UI repaints only when data arrives or on input. Process table is virtualised (only visible rows are laid out). |

## Build

```sh
cargo build --release -p oxi-taskmgr            # on Windows
cargo build --release -p oxi-taskmgr --target x86_64-pc-windows-gnu   # cross from Linux (needs mingw-w64)
cargo test --workspace
```

`OXI_TASKMGR_TIMING=1` prints the time to the first process list. `OXI_TASKMGR_TAB=performance` opens on the Performance tab.

## Performance targets (to verify on Windows)

| Metric | Target |
| --- | --- |
| Launch to process list | < 150 ms |
| Private working set, idle | < 25 MB |
| CPU at idle, 1 s updates | < 0.5 % |
| One snapshot of 300 processes | < 5 ms |
| Exe size | < 8 MB (currently 5.5 MB) |

Clean-room: behaviour is matched by observation; no Microsoft code, icons or resources are used.
