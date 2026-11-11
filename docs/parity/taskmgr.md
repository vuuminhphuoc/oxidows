# Task Manager parity checklist

Compared against Windows 11 Task Manager (24H2). Behaviour is matched by observation only.
Status: **done** · **partial** · **todo** · **won't** (with reason). Update this file in the same commit
as the feature.

## Global

| Feature | Status | Notes |
| --- | --- | --- |
| Pages: Processes, Performance, App history, Startup apps, Users, Details, Services, Settings | done | |
| Real-time update speed (High / Normal / Low / Paused) | done | 0.5 s / 1 s / 2 s / paused |
| Run as administrator | done | relaunch through UAC |
| Starts elevated without a prompt for an administrator (Windows' Task Manager auto-elevates) | done | Settings > Run as administrator: "Always, without a prompt" adds a scheduled task (`HighestAvailable`, interactive token) that a normal start hands over to; "Always, with a UAC prompt" relaunches through `runas`; a missing task falls back to the prompt and is put back |
| Run new task (with "create with administrative privileges") | done | `ShellExecuteW`, `runas` verb |
| Search box at the top (name, publisher or PID), applies to every list page | done | |
| Per-page command bar | done | |
| No status bar; messages in an info bar | done | |
| Settings: default start page, update speed, always on top | done | |
| Settings: theme (light / dark / system) | done | |
| Settings: other options | done | Show full account name, Show history for all processes, Ask me before applying Efficiency mode |
| Settings: hide when minimized | done | the tray icon brings the window back |
| Settings: minimize on use | done | after Switch to |
| Settings persisted between runs | done | `%APPDATA%\oxidows\taskmgr.ini` |
| Replace Ctrl+Shift+Esc / taskmgr.exe | done | Settings > Default app: IFEO `Debugger`, asks for admin (UAC) itself; restore gives back Task Manager or the tool that had it; `--restore-default`; `taskmgr /startup` opens Startup |
| One instance: starting it again brings the open window forward | done | named mutex; also from a non-elevated start to an elevated window, and out of the tray; Run as administrator waits for the old one |
| Notification-area icon with CPU usage | done | left click shows, right click Show / Exit |
| Keyboard navigation in every table | done | Arrow Up/Down, Page Up/Down, Home/End, type-ahead (900 ms wrapping), Enter toggles apps in Processes, Delete ends task in Processes and Details |
| Keyboard: Delete = End task, Ctrl+Tab / Ctrl+Shift+Tab between pages | done | |
| Sort every table by any column: click a header, click again to reverse, chevron above the sorted column | done | Processes, App history, Startup apps, Users, Details, Services; each page's sort is saved between runs |

## Hover and look

| Feature | Status | Notes |
| --- | --- | --- |
| No zebra stripes; hover fill on every table (App history included) | done | Heat-map cells (CPU, Memory, Disk, Network, GPU) preserve row hover highlight |
| Arrow pointer over column headers (no hand) | done | |
| Title-bar search box, per-page command bar, no status bar | done | |
| Keyboard focus ring on table rows | won't | No shared table wrapper in ui-common (pages construct TableBuilder directly) |

## Processes

| Feature | Status | Notes |
| --- | --- | --- |
| Name, PID, CPU, Memory, Disk, GPU columns | done | memory = private working set |
| Heat-map colouring | done | CPU, Memory, Disk, GPU |
| Sort by any column | done | |
| End task | done | button, Delete key, right-click |
| Groups: Apps / Background processes / Windows processes | done | collapsible, can be turned off |
| Friendly names (file description) | done | hover shows the image name |
| Program icon left of each name | done | shell small icon of the exe, read on a background thread and cached by path; default executable icon via `SHGetStockIconInfo(SIID_APPLICATION)` when there is no path (System, Registry) or icon fails to load |
| Machine totals above CPU, Memory, Disk, Network, GPU headers | done | Network total = all adapters' throughput over their link speed, shown even when per-process network is off |
| Expand an app to its child processes | done | children fold into the app row; windows are not listed |
| Network column | done | ETW Microsoft-Windows-Kernel-Network; needs administrator (Task Manager uses a system service) |
| GPU engine column | done | busiest engine per process, from PDH `GPU Engine(*)` |
| Status column | done | Suspended (pause icon), Efficiency (leaf icon); icons only, tooltip carries words |
| Publisher column | done | version resource CompanyName |
| Power usage, Power usage trend | done | estimated from CPU, GPU, disk and network (Windows' column is an estimate too); trend is a two-minute average |
| End process tree | done | children first, PID-reuse guard; confirms with "End process tree" / "Cancel" |
| Efficiency mode | done | EcoQoS + idle priority; confirms before turning on when "Ask me before applying Efficiency mode" is on |
| Critical process protection | done | confirms before ending critical Windows processes ("Shut down" / "Cancel") |
| Choose columns (right-click header) | done | 14 choices matching Windows 11, saved as processes_columns |
| Open file location, Copy command line, Search online | done | |
| Set priority, Set affinity | done | |
| Create memory dump file | done | `MiniDumpWriteDump` to `%TEMP%` |
| Go to details | done | |
| Restart (for Windows Explorer) | done | starts a new shell only if Windows did not |
| Resource values as % or absolute | done | Settings; Memory, Disk, Network |
| Expand arrows | done | ChevronDown / ChevronRight icons for group headers and app rows |

## Performance

| Feature | Status | Notes |
| --- | --- | --- |
| CPU graph 60 s, per-logical-processor view, NUMA nodes view | done | choice saved in taskmgr.ini (`cpu_logical`); NUMA nodes view with per-node average utilization, disabled on single-node machines |
| CPU utilization, speed, processes, threads, handles, up time | done | |
| CPU name, base speed, sockets, cores, logical processors, L1/L2/L3 cache, virtualization | done | |
| Kernel times overlay | done | "Show kernel times" in the CPU graph's right-click menu (as in Windows), also per processor; saved (`cpu_kernel`) |
| Memory graph, in use, available, committed | done | |
| Memory cached, compressed, paged / non-paged pool, composition bar, hardware reserved | done | `GetPerformanceInfo`, working set of Memory Compression process |
| Memory speed, slots, form factor | done | SMBIOS type 17 |
| One entry per physical disk: active time, response time, read / write speed, transfer-rate graph | done | PDH `PhysicalDisk(*)`; titled "Disk 0 (C:)" |
| Disk model, capacity, type (SSD / HDD), bus, system disk, page file | done | `IOCTL_STORAGE_QUERY_PROPERTY`, `IOCTL_DISK_GET_DRIVE_GEOMETRY_EX` (no admin) |
| One entry per adapter: send / receive, link speed | done | `GetIfTable2` |
| Adapter IPv4 / IPv6, DNS name, SSID, connection type, Wi-Fi signal strength | done | `GetAdaptersAddresses`, WLAN API |
| GPU 2x2 engine grid (default 3D, Copy, Video Encode, Video Decode with per-slot dropdown) and Dedicated / Shared memory graphs | done | PDH `GPU Engine(*)`, `GPU Adapter Memory(*)`; 60 s history ring per engine and for memory; axis labels and total titles |
| One entry per GPU adapter with name, dedicated / shared memory used of total | done | DXGI + PDH split by LUID |
| GPU driver version, driver date, DirectX version | done | DXGI `CheckInterfaceSupport`, the display driver key, D3D12 feature-level probe |
| GPU physical location | done | SetupAPI `SPDRP_LOCATION_INFORMATION` |
| GPU temperature | done | D3DKMT adapter perf data; shown where the driver reports it |
| Disk temperature | done | `StorageDeviceTemperatureProperty`; shown where the drive reports it |
| NPU | done | DXCore machine-learning adapters without graphics, numbered NPU 0, 1... |
| Copy to clipboard | done | |
| Summary view | done | button or double-click; double-click again to come back |
| Graph context menu and layout | done | Right-click graph menu (Graph summary view, Change graph to, Show kernel times, Copy); 4 axis labels (top-left caption, top-right 100%, bottom-left 60s, bottom-right 0); stats block with big value on left and 2 columns on right |
| Open Resource Monitor | done | launches `resmon` |
| FPS (extra, not in Windows' Task Manager) | done | DXGI / D3D9 `Present_Start` ETW events per swap chain; FPS, frame time, 1% low, 60 s graph, every app drawing frames; needs administrator; recording starts when the Performance tab first opens |

## App history

| Feature | Status | Notes |
| --- | --- | --- |
| CPU time, network, uploads, downloads per app since a date | done | SRUM (`sru\SRUDB.dat`) copied with `esentutl /vss`, read with the ESE API; needs administrator |
| Metered network | done | the user's per-connection choice (`DusmSvc\Profiles`), else `DefaultMediaCost` for the interface type |
| Delete usage history | done | keeps a baseline and subtracts it (Windows' own SRUM is left alone) |
| Choose columns (right-click header) | done | 6 columns matching Windows 11, saved as `history_columns` |
| Show history for all processes | done | includes classic win32 / background processes when enabled in Settings |

## Startup apps

| Feature | Status | Notes |
| --- | --- | --- |
| Name, Publisher, Status, Command, Location | done | Run keys (HKCU, HKLM, WOW6432Node) + Startup folders |
| Enable / Disable | done | `StartupApproved`, same store as Windows |
| Startup impact, Disk I/O and CPU at startup | done | Microsoft's thresholds (1 s / 3 MB, 300 ms / 300 KB) applied to the app's own CPU and I/O when it started within 5 minutes of boot; Windows uses boot traces, so values can differ |
| Last BIOS time | done | registry `Power\FwPOSTTime` |
| Packaged (Store) app startup tasks | done | `AppModel\SystemAppData`, enable / disable |
| Scheduled-task startup entries | done (n/a) | Task Manager does not list them either |
| Open file location, Search online | done | |
| Properties | done | shell Properties sheet |
| Choose columns (right-click header) | done | includes Startup type, Running now, Disabled time; saved as `startup_columns` |

## Users

| Feature | Status | Notes |
| --- | --- | --- |
| Signed-in sessions with status, CPU, memory, disk, process count | done | |
| Network per user | done | needs administrator, like the Network column |
| Expand a user to their processes | done | with program icons |
| Totals above CPU, Memory, Disk, Network, GPU headers | done | |
| GPU column | done | per-user sum of process GPU % |
| Choose columns (right-click header) | done | ID, Session, Client name; saved as `users_columns` |
| Show full account name | done | shows DOMAIN\user when enabled in Settings, short name when disabled |
| Manage user accounts | done | command bar button opens `ms-settings:otherusers` |
| Disconnect, Sign off | done | asks first |

## Details

| Feature | Status | Notes |
| --- | --- | --- |
| Name, PID, user, session, CPU, memory, threads, handles, priority, path, command line | done | |
| Choose columns | done | 39 columns, saved between runs; includes GDI objects, User objects, Architecture, Power throttling, Package name |
| Reorder columns | done | drag a column header to reorder with drop marker indicator |
| Program icon left of the name | done | same cache as Processes |
| Same right-click actions as Processes | done | end, tree, efficiency, priority, affinity, dump, search |
| Go to service(s) | done | svchost.exe rows |
| UAC virtualization (column and toggle), Analyze wait chain | done | Wait Chain Traversal; offers to end the blocking process |
| Properties | done | |

## Services

| Feature | Status | Notes |
| --- | --- | --- |
| Name, PID, Description, Status | done | search by name, description or PID |
| Group column | done | svchost `-k` group from the image path |
| Start, Stop, Restart | done | worker thread |
| Open Services, Search online, Go to details | done | |
