# File Explorer parity checklist

What Windows 11 File Explorer does, and where `oxi-explorer` stands. Matched by observation
(clean-room). ✅ done · 🟡 partial · ❌ not yet · ➖ out of scope (says why).

## Navigation

| Feature | Status | Notes |
| --- | --- | --- |
| Back / Forward / Up, Alt+Left/Right/Up, mouse back/forward buttons | ✅ | |
| Breadcrumb address bar, click a segment | ✅ | |
| Type a path / UNC path in the address bar (Ctrl+L, Alt+D) | ✅ | |
| Breadcrumb segment dropdown (subfolders after each segment) | ✅ | |
| Address bar autocomplete | ✅ | subfolders; Tab completes |
| Environment variables and shell: names in the address bar (`%TEMP%`, `shell:startup`) | ✅ | common shell: names |
| Recent locations history dropdown | ✅ | |
| Navigation pane: Home, known folders, drives with free space | ✅ | |
| Navigation pane: folder tree (expand/collapse) | ✅ | lazy, loaded in the background on first expand |
| Pin / unpin to Quick access | ✅ | own list, mirrors the shell's Pin to Quick access verb |
| This PC page (drives as tiles) | ✅ | known folders and drives with free space; Up from a drive root |
| Tabs (Ctrl+T, Ctrl+W, Ctrl+Tab, drag tab out) | ✅ | new/close/switch, middle-click; drag along the bar reorders, drag away opens the tab in a new window |
| Open in new window (Ctrl+N) | ✅ | |
| Command-line argument opens a folder | ✅ | `oxi-explorer.exe <folder>` |

## Network (SMB)

| Feature | Status | Notes |
| --- | --- | --- |
| Open `\\server\share\…` paths | ✅ | |
| Mapped network drives listed | ✅ | |
| `\\server` lists its shares | ✅ | `NetShareEnum`; tested against a real share on CI |
| Login prompt when a share needs credentials (remember password) | ✅ | Windows' own credential dialog, asked once per share, plus a Sign in button |
| Map / disconnect network drive | ✅ | … menu, sidebar right-click; reconnect at sign-in |
| Network page: computers on the LAN | ✅ | NetBIOS browse list plus the shell Network folder (Function Discovery: WSD computers) |
| Network media devices (DLNA), WSD printers | ✅ | listed from the shell Network folder; opened and browsed through the shell |

## Views and columns

| Feature | Status | Notes |
| --- | --- | --- |
| Details view: Name, Date modified, Type, Size | ✅ | |
| Sort by any column, folders first, natural order | ✅ | |
| Resize columns | ✅ | |
| More columns (Date created, Attributes), choose columns | ✅ | right-click a column header |
| Other columns (Owner, Dimensions, Authors, …) | ✅ | Authors, Title, Tags, Rating, Dimensions, Length, Date taken, Owner from the property system; not sortable yet |
| Group by (name, date modified, type, size) | ✅ | Details and icon views (not List) |
| Extra large / Large / Medium / Small icons, List, Details, Tiles (Ctrl+Shift+1…7) | ✅ | |
| Content view (Ctrl+Shift+8) | ✅ | name + type, date modified + size, thumbnails |
| Thumbnails for images and videos | ✅ | the shell's thumbnail cache (`IShellItemImageFactory`), Large icons view |
| System file icons | ✅ | `SHGetFileInfoW`, cached per extension |
| Overlay icons (shortcut arrow, OneDrive status) | ✅ | shell overlays on links, junctions, cloud files; desktop.ini folder icons |
| Show/hide hidden items | ✅ | |
| Show/hide file name extensions | ✅ | … menu |
| Show protected OS files | ✅ | separate checkbox; hidden+system items stay out unless it is on |
| Compact mode | ✅ | … menu ▸ Compact view: tighter rows and tiles |
| Preview pane (Alt+P) | ✅ | hosts the registered Windows preview handler (Office, PDF, mail, fonts, media) out of process; text and thumbnails otherwise; files marked as downloaded from the internet get a warning instead, as Explorer |
| Details pane (Alt+Shift+P) | ✅ | type, size, dates, attributes; totals for a multi-selection |
| Status bar: item count, selection count and size | ✅ | |

## Selection and keyboard

| Feature | Status | Notes |
| --- | --- | --- |
| Click, Ctrl+click, Shift+click, Ctrl+A, Esc | ✅ | |
| Arrows, Home/End, PageUp/PageDown, Shift to extend | ✅ | |
| Type-ahead to jump to a name | ✅ | |
| Rubber-band (drag rectangle) selection | ✅ | every view; in Details it starts outside the name, as in Explorer, and scrolls
| Item check boxes | ✅ | shown on hover or when selected; every view |
| Invert selection | ✅ | … menu |

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
| Batch rename | ✅ | "name (1).ext", "name (2).ext"…; one Undo puts all back |
| Delete to Recycle Bin, Shift+Delete permanently | ✅ | |
| Native progress, conflict and Undo dialogs | ✅ | `SHFileOperationW` |
| Undo / Redo (Ctrl+Z / Ctrl+Y) inside the app | ✅ | rename, new folder, copy, move, delete to Recycle Bin |
| New folder (Ctrl+Shift+N) | ✅ | |
| New → text file, shortcut, other registered types | ✅ | through the shell menu |
| Drag and drop into the window | ✅ | move on same drive, copy across drives |
| Drag onto folders, the navigation pane and the Recycle Bin | ✅ | Ctrl copies, Shift moves, as Explorer |
| Drag out of the window to other programs | ✅ | system OLE drag with the shell's data object (`SHDoDragDrop`) |
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
| Windows Search index, file contents, `kind:` / `date:` filters | ✅ | AQS parsed by `IQueryParser`, run by a shell search folder (`ISearchFolderItemFactory`), so the index answers where available; without it: own walk with kind:, date:, size:, ext:, name:, content:, wildcards |

## Special locations

| Feature | Status | Notes |
| --- | --- | --- |
| Recycle Bin (browse, restore, empty) | ✅ | reads `$Recycle.Bin` `$I` records; original location and date deleted columns |
| Home / Recent files | ✅ | Quick access + recent files from the Recent folder |
| Libraries, Gallery | ✅ | libraries read with IShellLibrary, folders merged; Gallery = Pictures library pictures and videos, newest first |
| OneDrive status | ✅ | overlays on cloud files, plus a Status column (available when online / on this device / always available) |
| Control Panel and other shell namespaces | ✅ | `::shell\<parsing name>` folders listed with IShellItem; shell:ControlPanelFolder in the address bar; items open with their default verb |
| Phones (MTP) | ✅ | under This PC; browse, open, thumbnails, preview, copy/cut/paste both ways and delete through the shell's copy engine (`IFileOperation`), right-click menu. Tested on an Honor Magic8 Pro |

## Behaviour

| Feature | Status | Notes |
| --- | --- | --- |
| Folder refreshes itself when others change it | ✅ | `FindFirstChangeNotificationW` |
| Large folders stay responsive | ✅ | streamed listing, virtual table |
| Long paths (> 260 chars) | ✅ | `\\?\` prefix |
| Remembers window size, view and sort per folder | ✅ | `%APPDATA%\oxidows\explorer.ini` |
| Dark / light theme follows Windows | ✅ | System / Light / Dark in the … menu |
| Replace Win+E / default folder handler | ✅ | opt-in checkbox in the … menu; per user (HKCU) folders, drives and Win+E; untick restores File Explorer |
