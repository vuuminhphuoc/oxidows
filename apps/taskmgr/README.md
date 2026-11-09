# Task Manager (oxi-taskmgr)

A fast Windows Task Manager written in Rust, with every page of the Windows 11 one
([parity checklist](../../docs/parity/taskmgr.md): 80 of 80 rows done in v0.4.0).

## What works

- **Processes**: apps / background / Windows groups, child processes under their app, CPU, memory,
  disk, network, GPU and GPU engine, power usage and trend, status, publisher; heat-map colours,
  values as % or absolute; End task / process tree, Efficiency mode, priority, affinity, memory dump,
  Open file location, Restart for Windows Explorer.
- **Performance**: CPU (per logical processor, kernel times), memory (composition, speed, slots),
  every physical disk, network adapter, GPU and NPU with its own graph and details, GPU and disk
  temperature where the hardware reports it, Summary view, Copy, Open Resource Monitor.
- **App history** (from SRUM, needs administrator), **Startup apps** (enable / disable, startup
  impact, Store apps), **Users** (disconnect, sign off), **Details** (34 columns, wait chain, UAC
  virtualization), **Services** (start, stop, restart).
- **Settings**: start page, update speed, always on top, theme, hide when minimized with a
  notification-area CPU icon, minimize on use; saved in `%APPDATA%\oxidows\taskmgr.ini`.
  Run new task, Run as administrator, search on every page, Ctrl+Tab between pages.

## Layout

| Path | What it does |
| --- | --- |
| `crates/taskmgr-sys` | Reads the OS. Windows: one `NtQuerySystemInformation(SystemProcessInformation)` call for every process (no `OpenProcess` per process), `GetSystemTimes` + per-core NT counters, `GlobalMemoryStatusEx`, `GetIfTable2`, PDH for disk and GPU, DXGI / DXCore / D3DKMT for GPU and NPU details, ETW for per-process network, the ESE API for SRUM, SCM for services. Linux `/proc` reader is for development only. All `unsafe` lives here. |
| `crates/taskmgr-core` | Platform-independent math: CPU % from deltas, bytes/s, PID-reuse safety, 60-point history rings, sort/filter. Unit-tested. |
| `apps/taskmgr` | egui app. A sampler thread sends an `Arc<Snapshot>` per tick; the UI repaints only when data arrives or on input. Process table is virtualised (only visible rows are laid out). |

## Build

```sh
cargo build --release -p oxi-taskmgr            # on Windows
cargo build --release -p oxi-taskmgr --target x86_64-pc-windows-gnu   # cross from Linux (needs mingw-w64)
cargo test --workspace
```

`OXI_TASKMGR_TIMING=1` prints the time to the first process list and the sampler's cost.
`OXI_TASKMGR_TAB=performance|app|startup|users|details|services|settings` opens on that page,
`OXI_TASKMGR_RES=memory|disk0|net0|gpu0` on that Performance entry (CI uses these for screenshots,
see `smoke.json`).

## Measured

On a Windows 11 PC (Core i5-13500, RTX 3060), with OpenGL: window in 48 ms, 53 MB private
working set (Windows' Task Manager: 120 MB), 0.05 % idle CPU. The v0.4.0 exe is 11 MB. Every release is in
[docs/benchmarks.md](../../docs/benchmarks.md).

Clean-room: behaviour is matched by observation; no Microsoft code, icons or resources are used.
