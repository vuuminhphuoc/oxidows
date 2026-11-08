# File Explorer (oxi-explorer)

A fast Windows File Explorer written in Rust. First version: browse, open, and manage files.

![Explorer](../../docs/images/explorer.png)

## What works

- Back / Forward / Up, clickable breadcrumbs, type a path (Ctrl+L or click the address bar).
- Sidebar: Home, Desktop, Downloads, Documents, Pictures, Music, Videos, and every drive with free space.
- Details view: Name, Date modified, Type, Size. Sort by any column (folders first, natural order:
  `file2` before `file10`), search the current folder, show/hide hidden items.
- Open files with their default app, open folders, Properties dialog, Open in Terminal.
- Cut / Copy / Paste through the real Windows clipboard, so it works both ways with the built-in Explorer.
  Delete to Recycle Bin, Shift+Delete permanently, Rename (F2), New folder (Ctrl+Shift+N).
  Copy/move/delete use the Windows shell, so progress, "replace or skip" and Undo work as usual.
- Drop files from another window to copy (across drives) or move (same drive).
- The folder refreshes by itself when other programs change it.
- Keyboard: arrows/Home/End/PageUp/PageDown (Shift to extend), Enter, Backspace, Alt+Left/Right/Up,
  F5, Ctrl+A, Ctrl+F, Esc, Alt+Enter, type the first letters of a name to jump to it.

## Nearby sharing (LocalSend)

The **Share** button and **Nearby sharing** in the sidebar send and receive files with phones and
computers running the free [LocalSend](https://localsend.org) app (Android, iPhone, Mac, Linux,
Windows; no account, no internet, no Google services needed). `crates/oxi-share` implements the
open [LocalSend v2 protocol](https://github.com/localsend/protocol): multicast discovery on every
network interface (with a subnet scan for networks that drop multicast), a HTTPS server with
self-signed certificates identified by fingerprint, and the sending side. Sharing is off until
turned on in the window.

- On the same Wi-Fi: open LocalSend on the phone; this PC shows up under *Send*, and the phone
  shows up in Explorer's window. Received files go to Downloads (changeable).
- No common Wi-Fi: *Connect with Wi-Fi Direct* makes the PC open its own Wi-Fi Direct network and
  shows a QR code. The phone's camera joins it, then LocalSend works over it. Needs a Wi-Fi adapter
  with Wi-Fi Direct (most made after 2015); *Allow* adds the firewall rule that network needs.

## Layout

| Path | What it does |
| --- | --- |
| `crates/explorer-sys` | Talks to Windows. A folder is read with one `FindFirstFileExW(FindExInfoBasic, FIND_FIRST_EX_LARGE_FETCH)` pass: no per-file stat, no 8.3 names, results stream in batches of 2048. `SHFileOperationW` for copy/move/delete, `CF_HDROP` clipboard, `FindFirstChangeNotificationW` watcher, `SHGetKnownFolderPath`, `GetDiskFreeSpaceExW`. All `unsafe` lives here; a `std::fs` fallback exists for Linux development only. Integration tests in `tests/windows.rs` run on CI. |
| `crates/explorer-core` | No OS calls: sorting, natural order, filtering, size/date/type text, name validation, back/forward history. Unit-tested. |
| `apps/explorer` | egui app. Listing, watching and file operations each run on their own thread, so the window never waits on the disk. The table is virtualised (only visible rows are laid out) and the sorted view is cached until the listing, sort or search changes. |

## Build

```sh
cargo build --release -p oxi-explorer                                  # on Windows
cargo build --release -p oxi-explorer --target x86_64-pc-windows-gnu   # cross from Linux (needs mingw-w64)
```

`oxi-explorer.exe <folder>` opens that folder. `OXI_EXPLORER_TIMING=1` prints how long the first listing took.

## Not yet

Icons from the system, thumbnails, tabs, Large icons view, recursive search, drag out of the window,
the shell's own right-click menu, Recycle Bin / Network / This PC pages.
