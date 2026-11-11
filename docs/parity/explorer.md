# File Explorer parity checklist

What Windows 11 File Explorer does, and where `oxi-explorer` stands. Matched by observation
(clean-room). ✅ done · 🟡 partial · ❌ not yet · ➖ out of scope (says why).

## Navigation

| Feature | Status | Notes |
| --- | --- | --- |
| Back / Forward / Up, Alt+Left/Right/Up, mouse back/forward buttons | ✅ | |
| Breadcrumb address bar, click a segment | ✅ | |
| Type a path / UNC path in the address bar (Ctrl+L, Alt+D, F4) | 🟡 | Ctrl+L / Alt+D verified; F4 added with select-all (live GUI not yet verified) |
| Breadcrumb segment dropdown (subfolders after each segment) | ✅ | |
| Address bar autocomplete | ✅ | subfolders; Tab completes |
| Environment variables and shell: names in the address bar (`%TEMP%`, `shell:startup`) | ✅ | common shell: names |
| Recent locations history dropdown | ✅ | |
| Navigation pane: Home, known folders, drives with free space | ✅ | |
| Navigation pane: folder tree (expand/collapse) | ✅ | lazy, loaded in the background on first expand |
| Expand navigation pane to current folder (Ctrl+Shift+E) | 🟡 | path/ancestor matching unit-tested; live tree expansion and scroll not yet verified |
| Navigation pane: OneDrive and Linux (`\\wsl.localhost`) | 🟡 | Windows only; OneDrive env dedup/naming unit-tested; Linux shown when `wsl.exe` exists without startup enumeration; live GUI not yet verified |
| Pin / unpin to Quick access | ✅ | own list, mirrors the shell's Pin to Quick access verb |
| This PC page (drives as tiles) | ✅ | known folders and drives with free space; Up from a drive root |
| Tabs (Ctrl+T, Ctrl+W, Ctrl+Tab, drag tab out) | ✅ | new/close/switch, middle-click; drag along the bar reorders, drag away opens the tab in a new window |
| Tab menu: Duplicate, Move to new window, Close other tabs, Close tabs to the right | ✅ | right-click on any tab |
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

## Window layout (Windows 11 look)

| Feature | Status | Notes |
| --- | --- | --- |
| Command bar: New ▾, Cut, Copy, Paste, Rename, Share, Delete, Sort ▾, View ▾, See more, Details | ✅ | buttons turn off when they don't apply; Recycle Bin shows Empty / Restore the selected items instead of New |
| New ▾: Folder, Shortcut and the file types Windows registers (ShellNew) | ✅ | types read from the registry (NullFile, Data, FileName templates, Command), sorted by name; Shortcut opens Windows' Create Shortcut wizard |
| Sort ▾: Name, Date modified, Type, Size, More, Ascending / Descending, Group by | ✅ | works in every view, not only Details |
| Status bar: item and selection counts, Details / Large icons switches on the right | ✅ | |
| Back / Forward right-click lists recent locations | ✅ | |
| Address bar fits long paths (hidden segments in the “…” menu) | ✅ | |
| Sort chevron above the column title, plain header text | ✅ | shared with Task Manager |
| Tabs in the title bar, Mica background | ❌ | own row under the system title bar |
| Windows 11 context menu with “Show more options” | ✅ | top icon row (Cut, Copy, Rename, Share, Delete), common verbs; Shift+right-click opens classic shell menu |
| Rename in place (not in a dialog) | ✅ | the name turns into a text box in every view; Enter or clicking elsewhere keeps it, Esc cancels; a bad name shows a tip and stays editable |
| Header filter drop-downs, More… column chooser | ❌ | |

## Hover and tooltips

| Feature | Status | Notes |
| --- | --- | --- |
| Toolbar and command-bar buttons: no frame until hovered, tooltip with the shortcut | ✅ | |
| Breadcrumb segments and arrows light up separately | ✅ | |
| Navigation pane: hover fill; expand chevrons only while the pointer is over the pane | ✅ | |
| Item tip when the pointer rests on a file (type, size, date modified) or folder (date created) | ✅ | |
| No zebra stripes in lists; hover fill shows | ✅ | |
| Drag hover opens a folder in the tree / switches tab | 🟡 | 600 ms hover timer unit-tested; expands collapsed tree folders and switches tabs; drag-hover open folder in list/grid skipped (navigating drops drag); live GUI not yet verified |

## Views and columns

| Feature | Status | Notes |
| --- | --- | --- |
| Details view: Name, Date modified, Type, Size | ✅ | |
| Sort by any column, folders first, natural order | ✅ | |
| Resize columns | ✅ | |
| More columns (Date created, Attributes), choose columns | ✅ | right-click a column header |
| Other columns (Owner, Dimensions, Authors, …) | ✅ | Authors, Title, Tags, Rating, Dimensions, Length, Date taken, Owner from the property system; not sortable yet |
| Group by (name, date modified, type, size) | ✅ | Details and icon views (not List); remembered per folder |
| Extra large / Large / Medium / Small icons, List, Details, Tiles (Ctrl+Shift+1…7) | ✅ | |
| Content view (Ctrl+Shift+8) | ✅ | name + type, date modified + size, thumbnails |
| Ctrl+mouse wheel changes view / icon size | 🟡 | view stepping logic unit-tested; list scroll suppressed while Ctrl is held; live GUI not yet verified |
| Thumbnails for images and videos | ✅ | the shell's thumbnail cache (`IShellItemImageFactory`), Large icons view |
| System file icons | ✅ | `SHGetFileInfoW`, cached per extension |
| Overlay icons (shortcut arrow, OneDrive status) | ✅ | shell overlays on links, junctions, cloud files; desktop.ini folder icons |
| Show/hide hidden items | ✅ | View ▸ Show |
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
| Arrows, Home/End, PageUp/PageDown, Shift to extend | ✅ | in the icon views Left/Right move along the row and Up/Down to the row above/below (across groups); in List Left/Right move a column |
| Type-ahead to jump to a name | ✅ | |
| Rubber-band (drag rectangle) selection | ✅ | every view; in Details it starts outside the name, as in Explorer, and scrolls
| Item check boxes | ✅ | shown on hover or when selected; every view |
| Invert selection | ✅ | … menu |
| Keyboard shortcuts (Ctrl+E, F4, Ctrl+D, F11, Ctrl+Shift+E, Ctrl+Shift+1…8) | 🟡 | Ctrl+Shift+1…8 existing; Ctrl+E, F4, Ctrl+D, F11, Ctrl+Shift+E added (live GUI not yet verified); F6 focus cycle skipped (incompatible with egui focus model) |

## File operations

| Feature | Status | Notes |
| --- | --- | --- |
| Open with default app, run programs | ✅ | |
| Open with… (choose app) | ✅ | through the shell menu |
| Run as administrator | ✅ | through the shell menu |
| Cut / Copy / Paste, shared clipboard with Windows Explorer | ✅ | `CF_HDROP` + Preferred DropEffect |
| Cut items faded (~50% opacity) until paste, clipboard change, or Esc | 🟡 | shown in Details and icon/tile/content views via clipboard sequence check; Esc clears local cut state without modifying system clipboard; live GUI not yet verified |
| Paste into a folder | ✅ | |
| Copy as path | ✅ | |
| Paste shortcut | ✅ | through the shell menu |
| Rename (F2), selects name without extension | ✅ | |
| Batch rename | ✅ | "name (1).ext", "name (2).ext"…; one Undo puts all back |
| Delete to Recycle Bin, Shift+Delete permanently | ✅ | |
| Native progress, conflict and Undo dialogs | ✅ | `SHFileOperationW` |
| Undo / Redo (Ctrl+Z / Ctrl+Y) inside the app | ✅ | rename, new folder, copy, move, delete to Recycle Bin |
| New folder (Ctrl+Shift+N) | ✅ | made at once as "New folder" and renamed in place, as in Windows 11 (also New ▸ file types) |
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
| Search: Everything when it runs, multi-threaded walk fallback | ✅ | Instant name search via Everything WM_COPYDATA IPC, parallel directory walk (up to 8 threads) as fallback (beyond Windows' own Explorer) |
| Search options menu (Date modified, Kind, Size filters) | 🟡 | pure date/kind/size filter buckets unit-tested in `explorer-core`; active filters shown in command bar and status bar; live GUI not yet verified |

## Special locations

| Feature | Status | Notes |
| --- | --- | --- |
| Recycle Bin (browse, restore, empty) | ✅ | reads `$Recycle.Bin` `$I` records; original location and date deleted columns |
| Home: Quick access, Favorites, Recent | 🟡 | 3 collapsible sections; Quick access folder tile grid with pin markers & frequent folders counter (ranking unit-tested); Favorites table with Add/Remove starred files; Recent files table; collapsed state and lists persisted in explorer.ini |
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
| Remembers window size, view and sort per folder | ✅ | `%APPDATA%\oxidows\explorer.ini`; a change applies to that folder only |
| Folder type defaults (Pictures/Videos large icons, Downloads grouped by date) | ✅ | by known folder and its subfolders |
| Folder Options (General, View, Search) | 🟡 | 3-tab dialog matching Windows. General: open to Home/This PC, browse folders in same/new window, single/double-click open, privacy toggles & clear history, folder views, default file manager. View: advanced checkboxes (full path in title, show hidden, hide empty drives, hide extensions, drive letters before/after, never thumbnails, checkboxes, status bar, expand to open, show all folders, show libraries), restore defaults button. Search: non-indexed content search |
| Folder Options: skipped items | — | Hide folder merge conflicts (no conflict UI), Always show menus (modern command bar only), Show sync provider notifications (no sync notifications UI), Display file size in folder tips (requires recursive tree scan), Launch folder windows in separate process (single-process multi-window design), Don't use index for system files / Include system directories (parallel search covers subtree), Include compressed files (no archive search) |
| Shell icons and thumbnails for images | ✅ | Resolves AppX Photos and system shell icons from imageres/shell32 for .png/.jpg and image types; thumbnails in icon views |
| Tab context menu icons | ✅ | Icons for Close tab, Close other tabs, Close tabs to the right, Duplicate tab, Move to new window |
| Navigation pane expand chevrons | ✅ | Smoothly rotating chevron shown when mouse hovers over pane |
| Menu icons and shortcut keys in menu rows | ✅ | View rows show a picture of each layout; Show, Group by, Theme submenus |
| Dark / light theme follows Windows | ✅ | System / Light / Dark in the … menu |
| Replace Win+E / default folder handler | ✅ | … > Options > Set as default; per user (HKCU) folders, drives and Win+E; restore gives back File Explorer or the file manager that had Win+E, removes every key; `--restore-default` |

## Sharing

| Feature | Status | Notes |
| --- | --- | --- |
| Share button, Nearby sharing (send to devices around you, receive from them) | ✅ | Windows' Nearby Share only reaches other Windows PCs. Here it speaks the open LocalSend v2 protocol (`crates/oxi-share`), so phones (Android, iPhone) and Mac/Linux/Windows PCs with the free LocalSend app take part. Accept/decline prompt, text messages, folders, progress, cancel, drag files onto a device |
| Share when there is no common Wi-Fi | ✅ | Wi-Fi Direct group owned by the PC (`WiFiDirectAdvertisementPublisher`, legacy mode) with a QR code the phone's camera joins; the PC keeps its own Wi-Fi |
