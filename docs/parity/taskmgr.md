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
| Run new task (with "create with administrative privileges") | done | `ShellExecuteW`, `runas` verb |
| Search box | done | per page |
| Status bar (process count, CPU, memory) | done | |
| Settings: default start page, update speed, always on top | done | |
| Settings: theme (light / dark / system) | done | |
| Settings: hide when minimized | done | the tray icon brings the window back |
| Settings: minimize on use | done | after Switch to |
| Settings persisted between runs | done | `%APPDATA%\oxidows\taskmgr.ini` |
| Replace Ctrl+Shift+Esc / taskmgr.exe | done | Settings > Default app: IFEO `Debugger`, asks for admin (UAC) itself; restore gives back Task Manager or the tool that had it; `--restore-default`; `taskmgr /startup` opens Startup |
| One instance: starting it again brings the open window forward | done | named mutex; also from a non-elevated start to an elevated window, and out of the tray; Run as administrator waits for the old one |
| Notification-area icon with CPU usage | done | left click shows, right click Show / Exit |
| Keyboard: Delete = End task, Ctrl+Tab / Ctrl+Shift+Tab between pages | done | |
| Sort every table by any column: click a header, click again to reverse, chevron above the sorted column | done | Processes, App history, Startup apps, Users, Details, Services; each page's sort is saved between runs |

## Processes

| Feature | Status | Notes |
| --- | --- | --- |
| Name, PID, CPU, Memory, Disk, GPU columns | done | memory = private working set |
| Heat-map colouring | done | CPU, Memory, Disk, GPU |
| Sort by any column | done | |
| End task | done | button, Delete key, right-click |
| Groups: Apps / Background processes / Windows processes | done | collapsible, can be turned off |
| Friendly names (file description) | done | hover shows the image name |
| Expand an app to its child processes | done | children fold into the app row; windows are not listed |
| Network column | done | ETW Microsoft-Windows-Kernel-Network; needs administrator (Task Manager uses a system service) |
| GPU engine column | done | busiest engine per process, from PDH `GPU Engine(*)` |
| Status column | done | Suspended, Efficiency |
| Publisher column | done | version resource CompanyName |
| Power usage, Power usage trend | done | estimated from CPU, GPU, disk and network (Windows' column is an estimate too); trend is a two-minute average |
| End process tree | done | children first, PID-reuse guard |
| Efficiency mode | done | EcoQoS + idle priority |
| Open file location, Copy command line, Search online | done | |
| Set priority, Set affinity | done | |
| Create memory dump file | done | `MiniDumpWriteDump` to `%TEMP%` |
| Go to details | done | |
| Restart (for Windows Explorer) | done | starts a new shell only if Windows did not |
| Resource values as % or absolute | done | Settings; Memory, Disk, Network |

## Performance

| Feature | Status | Notes |
| --- | --- | --- |
| CPU graph 60 s, per-logical-processor view | done | choice saved in taskmgr.ini (`cpu_logical`) |
| CPU utilization, speed, processes, threads, handles, up time | done | |
| CPU name, base speed, sockets, cores, logical processors, L1/L2/L3 cache, virtualization | done | |
| Kernel times overlay | done | checkbox on the CPU page, also per processor; saved (`cpu_kernel`) |
| Memory graph, in use, available, committed | done | |
| Memory cached, paged / non-paged pool, composition bar, hardware reserved | done | `GetPerformanceInfo` |
| Memory speed, slots, form factor | done | SMBIOS type 17 |
| One entry per physical disk: active time, response time, read / write speed, transfer-rate graph | done | PDH `PhysicalDisk(*)`; titled "Disk 0 (C:)" |
| Disk model, capacity, type (SSD / HDD), bus, system disk, page file | done | `IOCTL_STORAGE_QUERY_PROPERTY`, `IOCTL_DISK_GET_DRIVE_GEOMETRY_EX` (no admin) |
| One entry per adapter: send / receive, link speed | done | `GetIfTable2` |
| Adapter IPv4 / IPv6, DNS name, SSID | done | `GetAdaptersAddresses`, WLAN API |
| GPU utilization, engines (3D, Copy, Video…), dedicated / shared memory | done | PDH `GPU Engine(*)`, `GPU Adapter Memory(*)` |
| One entry per GPU adapter with name, dedicated / shared memory used of total | done | DXGI + PDH split by LUID |
| GPU driver version, driver date, DirectX version | done | DXGI `CheckInterfaceSupport`, the display driver key, D3D12 feature-level probe |
| GPU physical location | done | SetupAPI `SPDRP_LOCATION_INFORMATION` |
| GPU temperature | done | D3DKMT adapter perf data; shown where the driver reports it |
| Disk temperature | done | `StorageDeviceTemperatureProperty`; shown where the drive reports it |
| NPU | done | DXCore machine-learning adapters without graphics, numbered NPU 0, 1... |
| Copy to clipboard | done | |
| Summary view | done | button or double-click; double-click again to come back |
| Open Resource Monitor | done | launches `resmon` |

## App history

| Feature | Status | Notes |
| --- | --- | --- |
| CPU time, network, uploads, downloads per app since a date | done | SRUM (`sru\SRUDB.dat`) copied with `esentutl /vss`, read with the ESE API; needs administrator |
| Metered network | done | the user's per-connection choice (`DusmSvc\Profiles`), else `DefaultMediaCost` for the interface type |
| Delete usage history | done | keeps a baseline and subtracts it (Windows' own SRUM is left alone) |

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

## Users

| Feature | Status | Notes |
| --- | --- | --- |
| Signed-in sessions with status, CPU, memory, disk, process count | done | |
| Network per user | done | needs administrator, like the Network column |
| Expand a user to their processes | done | |
| Disconnect, Sign off | done | asks first |

## Details

| Feature | Status | Notes |
| --- | --- | --- |
| Name, PID, user, session, CPU, memory, threads, handles, priority, path, command line | done | |
| Choose columns | done | 34 columns, saved between runs |
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
