# File Explorer parity checklist

What Windows 11 File Explorer does, and where `oxi-explorer` stands. Matched by observation
(clean-room). ✅ done · 🟡 partial · ❌ not yet · ➖ out of scope (says why).

## Navigation

| Feature | Status | Notes |
| --- | --- | --- |
| Back / Forward / Up, Alt+Left/Right/Up, mouse back/forward buttons | ✅ | |
| Breadcrumb address bar, click a segment | ✅ | |
| Type a path / UNC path in the address bar (Ctrl+L, Alt+D) | ✅ | |
| Breadcrumb segment dropdown (siblings of each segment) | ❌ | |
| Address bar autocomplete | ❌ | |
| Environment variables and shell: names in the address bar (`%TEMP%`, `shell:startup`) | ❌ | |
| Recent locations history dropdown | ❌ | |
| Navigation pane: Home, known folders, drives with free space | ✅ | |
| Navigation pane: folder tree (expand/collapse) | ✅ | lazy, loaded in the background on first expand |
| Pin / unpin to Quick access | ❌ | |
| This PC page (drives as tiles) | ❌ | |
| Tabs (Ctrl+T, Ctrl+W, Ctrl+Tab, drag tab out) | 🟡 | new/close/switch, middle-click a folder or tab; no drag-out yet |
| Open in new window (Ctrl+N) | ❌ | |
| Command-line argument opens a folder | ✅ | `oxi-explorer.exe <folder>` |

## Network (SMB)

| Feature | Status | Notes |
| --- | --- | --- |
| Open `\\server\share\…` paths | ✅ | |
| Mapped network drives listed | ✅ | |
| `\\server` lists its shares | ✅ | `NetShareEnum`; tested against a real share on CI |
| Login prompt when a share needs credentials (remember password) | ✅ | Windows' own credential dialog, asked once per share, plus a Sign in button |
| Map / disconnect network drive | ✅ | … menu, sidebar right-click; reconnect at sign-in |
| Network page: computers on the LAN | 🟡 | `WNetOpenEnum` browse list; WSD-only devices not found yet |
| Network media devices (DLNA), WSD printers | ➖ | not file locations |

## Views and columns

| Feature | Status | Notes |
| --- | --- | --- |
| Details view: Name, Date modified, Type, Size | ✅ | |
| Sort by any column, folders first, natural order | ✅ | |
| Resize columns | ✅ | |
| More columns (Date created, Attributes, Owner, …), choose columns | ❌ | |
| Group by (type, date, …) | ❌ | |
| Large icons / Medium / Small / List / Tiles / Content views | ❌ | |
| Thumbnails for images and videos | ❌ | |
| System file icons | ✅ | `SHGetFileInfoW`, cached per extension |
| Overlay icons (shortcut arrow, OneDrive status) | ❌ | |
| Show/hide hidden items | ✅ | |
| Show/hide file name extensions | ✅ | … menu |
| Show protected OS files | ❌ | hidden+system shown with "Hidden items" |
| Compact mode | ➖ | |
| Preview pane | ❌ | |
| Details pane | ❌ | |
| Status bar: item count, selection count and size | ✅ | |

## Selection and keyboard

| Feature | Status | Notes |
| --- | --- | --- |
| Click, Ctrl+click, Shift+click, Ctrl+A, Esc | ✅ | |
| Arrows, Home/End, PageUp/PageDown, Shift to extend | ✅ | |
| Type-ahead to jump to a name | ✅ | |
| Rubber-band (drag rectangle) selection | ❌ | |
| Item check boxes | ❌ | |
| Invert selection | ❌ | |

## File operations

| Feature | Status | Notes |
| --- | --- | --- |
| Open with default app, run programs | ✅ | |
| Open with… (choose app) | ✅ | through the shell menu |
| Run as administrator | ✅ | through the shell menu |
| Cut / Copy / Paste, shared clipboard with Windows Explorer | ✅ | `CF_HDROP` + Preferred DropEffect |
| Paste into a folder | ✅ | |
| Copy as path | ✅ | |
| Paste shortcut | ✅ | through the shell menu |
| Rename (F2), selects name without extension | ✅ | |
| Batch rename | ❌ | |
| Delete to Recycle Bin, Shift+Delete permanently | ✅ | |
| Native progress, conflict and Undo dialogs | ✅ | `SHFileOperationW` |
| Undo / Redo (Ctrl+Z / Ctrl+Y) inside the app | ❌ | |
| New folder (Ctrl+Shift+N) | ✅ | |
| New → text file, shortcut, other registered types | ✅ | through the shell menu |
| Drag and drop into the window | ✅ | move on same drive, copy across drives |
| Drag and drop out of the window / between folders | ❌ | |
| Compress to ZIP / extract | ✅ | through the shell menu |
| Properties dialog (Alt+Enter) | ✅ | the shell's own |
| Open in Terminal | ✅ | Windows Terminal, else cmd |
| Shell context menu (all handlers: 7-Zip, Git, Send to, …) | ✅ | `IContextMenu3`, Shift for extended verbs; background menu (New ▸, Paste, …) too |
| Create shortcut | ✅ | through the shell menu |

## Search

| Feature | Status | Notes |
| --- | --- | --- |
| Filter the current folder as you type | ✅ | |
| Search subfolders (Enter) | ✅ | name match, streamed, skips links |
| Windows Search index, file contents, `kind:` / `date:` filters | ❌ | |

## Special locations

| Feature | Status | Notes |
| --- | --- | --- |
| Recycle Bin (browse, restore, empty) | ✅ | reads `$Recycle.Bin` `$I` records; original location and date deleted columns |
| Home / Recent files | ❌ | |
| Libraries, Gallery | ❌ | |
| OneDrive status | ❌ | |
| Control Panel and other shell namespaces | ➖ | not the file system |
| Phones (MTP) | ❌ | |

## Behaviour

| Feature | Status | Notes |
| --- | --- | --- |
| Folder refreshes itself when others change it | ✅ | `FindFirstChangeNotificationW` |
| Large folders stay responsive | ✅ | streamed listing, virtual table |
| Long paths (> 260 chars) | ✅ | `\\?\` prefix |
| Remembers window size, view and sort per folder | ❌ | |
| Dark / light theme follows Windows | 🟡 | egui default dark |
| Replace Win+E / default folder handler | ❌ | registry opt-in, later |
