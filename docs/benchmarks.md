# Benchmarks and feature comparison

Every oxidows release next to the Windows app it replaces, measured on one real PC on
2026-11-07: Windows 11 Pro 26200, Intel Core i5-13500 (20 threads), 96 GB RAM, NVIDIA RTX 3060,
one 1920×1080 monitor at 100 %. Each oxidows build is a release build of its git tag; the v0.4.0
timings are of the branch that became v0.4.0, before its later features (NPU pages, power usage,
GPU and disk temperatures, the shared look, nearby sharing) were merged; their feature counts are
of v0.4.0 itself.

How it was measured:

- **Window**: time from process start to its first top-level window, median of 3.
- **Memory**: private working set (the Memory column of Task Manager's Processes tab) and
  private bytes (committed memory, GPU driver heaps included), 5-6 s after start.
- **Idle CPU**: share of the whole machine over 3 s with the app on its first page, median of 3.

Windows Task Manager runs elevated, so a script can measure it once but can't time or close it.
File Explorer windows normally live inside the desktop's `explorer.exe`, so it was measured
as `explorer.exe /separate,<folder>`. That time includes starting a shell process.

## Task Manager

| Version | Renderer | Window | Memory (private WS) | Private bytes | Idle CPU | Features done* |
| --- | --- | --- | --- | --- | --- | --- |
| Windows Task Manager | - | n/a | 120 MB | 162 MB | 0.58 % | - |
| oxidows v0.1.0 | wgpu | 46 ms | 142 MB | 497 MB | 0.10 % | 47 of 73 |
| oxidows v0.1.1 | wgpu | 46 ms | 142 MB | 494 MB | 0.08 % | 47 of 73 |
| oxidows v0.2.0 | wgpu | 47 ms | 142 MB | 495 MB | 0.08 % | 53 of 75 |
| oxidows v0.3.0 | wgpu | 46 ms | 143 MB | 495 MB | 0.08 % | 73 of 78 |
| oxidows v0.4.0 | OpenGL (default on GPU PCs) | 48 ms | **53 MB** | **128 MB** | 0.05 % | 80 of 80 |
| oxidows v0.4.0 | wgpu (`OXI_RENDERER=wgpu`) | 46 ms | - | 496 MB | 0.08 % | 80 of 80 |

\* Rows of [`parity/taskmgr.md`](parity/taskmgr.md) marked done, out of all rows at that tag.
The rest are partial, or won't be done because Windows doesn't expose the data.

What each version added:

- **v0.1.0**: Processes and Performance tabs.
- **v0.1.1**: idle CPU fix under software rendering.
- **v0.2.0**: Network column, a page per GPU, adapter addresses, Details columns, tray icon.
- **v0.3.0**: App history, Summary view, disk, memory and GPU details, wait chain, percent
  values, Store startup apps.
- **v0.4.0**: disk capacity shown again (it was "-" without admin), "Disk 0 (C:)" titles,
  disk transfer rate graph, Segoe UI font (Vietnamese and other accented names), OpenGL by default,
  NPU pages, Power usage and trend, Startup impact, user-set metered connections, GPU and disk
  temperatures, the Windows 11 look shared with Explorer.

Up to v0.3.0, oxi-taskmgr used *more* memory than the original (142 MB against 120 MB), and
nearly all of it was the wgpu Vulkan/DX12 driver heap. OpenGL brings it to less than half of the
original.

## File Explorer

| Version | Renderer | Window | Memory (private WS) | Private bytes | Idle CPU | Features done* |
| --- | --- | --- | --- | --- | --- | --- |
| Windows File Explorer (separate process) | - | 765 ms | 67 MB | 145 MB | 0.00 % | - |
| oxidows explorer v0.1 (PR #1) | wgpu | 47 ms | 136 MB | 488 MB | 0.05 % | 77 of 82 |
| oxidows v0.4.0 | OpenGL (default on GPU PCs) | 47 ms | **40 MB** | **121 MB** | 0.00 % | 84 of 84 |

\* Rows of [`parity/explorer.md`](parity/explorer.md) marked ✅, out of all rows. The rest in
v0.1: Network page WSD devices (partial), phones over MTP (not yet), and three rows out of
scope. v0.4.0 has all 84: phones, Control Panel, WSD and media devices through the shell,
Compact view, and nearby sharing (LocalSend, Wi-Fi Direct).

Fixed in v0.4.0 after testing on the real PC (none of these show up on the CI runner):

| Area | Before | After |
| --- | --- | --- |
| Search | `search-ms:` enumeration always failed, so every search fell back to a name-only walk | Windows Search index through `ISearchFolderItemFactory`: file contents, `kind:`, `date:`, ~50 ms |
| Accented names | "Sơ Đồ Kịch Bản.pdf" drew as boxes | Segoe UI, as in Windows |
| Preview of downloaded files | previewed (PDF, Excel), or an icon when the handler refused (Word) | warning instead of a preview, as Explorer (Mark of the Web, zone ≥ 3) |
| Word / Excel / PowerPoint / PDF preview | - | checked working for local files |
| Status bar, breadcrumb | "1 items"; the folder's own crumb re-ran the search | "1 item"; the crumb goes back to the folder |

Checked and working as is: drag out to other apps (moves within a drive, as Explorer), light
and dark theme, UI zoom (Ctrl +/-) with a preview handler open, windows on the remaining
screen after a monitor is turned off. Not checked: display scaling other than 100 %.

## Renderer choice

OpenGL is the default when a desktop-attached NVIDIA, AMD or Intel adapter is present in a
local session. Otherwise it is wgpu, which falls back to the WARP software rasteriser on VMs,
RDP and CI runners, where OpenGL is only the GDI 1.1 stub. If OpenGL fails to start, the app
relaunches itself with wgpu. `OXI_RENDERER=glow|wgpu` overrides the choice
(`crates/ui-common/src/renderer.rs`).
