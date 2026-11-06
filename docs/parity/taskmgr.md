# Task Manager parity checklist

Compared against Windows 11 Task Manager (24H2). Behaviour is matched by observation only.
Status: **done** · **partial** · **todo** · **won't** (with reason). Update this file in the same commit
as the feature.

## Global

| Feature | Status | Notes |
| --- | --- | --- |
| Pages: Processes, Performance, App history, Startup apps, Users, Details, Services, Settings | partial | App history todo |
| Real-time update speed (High / Normal / Low / Paused) | done | 0.5 s / 1 s / 2 s / paused |
| Run as administrator | done | relaunch through UAC |
| Run new task (with "create with administrative privileges") | done | `ShellExecuteW`, `runas` verb |
| Search box | done | per page |
| Status bar (process count, CPU, memory) | done | |
| Settings: default start page, update speed, always on top | done | |
| Settings: theme (light / dark / system) | done | |
| Settings: minimize on use, hide when minimized | todo | needs "Switch to" and a tray icon |
| Settings persisted between runs | done | `%APPDATA%\oxidows\taskmgr.ini` |
| Replace Ctrl+Shift+Esc / taskmgr.exe | done | opt-in on Settings, IFEO `Debugger`, needs admin |
| Notification-area icon with CPU usage | todo | |
| Keyboard: Delete = End task, Ctrl+Tab / Ctrl+Shift+Tab between pages | done | |

## Processes

| Feature | Status | Notes |
| --- | --- | --- |
| Name, PID, CPU, Memory, Disk, GPU columns | done | memory = private working set |
| Heat-map colouring | done | CPU, Memory, Disk, GPU |
| Sort by any column | done | |
| End task | done | button, Delete key, right-click |
| Groups: Apps / Background processes / Windows processes | done | collapsible, can be turned off |
| Friendly names (file description) | done | hover shows the image name |
| Expand an app to its child processes and windows | todo | |
| Network column | todo | needs per-process ETW network accounting |
| GPU engine column | todo | per-process engine type |
| Status column (Suspended, Efficiency mode) | todo | |
| Publisher column | done | version resource CompanyName |
| Power usage, Power usage trend | won't (for now) | no public per-process energy API |
| End process tree | done | children first, PID-reuse guard |
| Efficiency mode | done | EcoQoS + idle priority |
| Open file location, Copy command line, Search online | done | |
| Set priority, Set affinity | done | |
| Create memory dump file | done | `MiniDumpWriteDump` to `%TEMP%` |
| Go to details | done | |
| Restart (for Windows Explorer) | done | starts a new shell only if Windows did not |
| Resource values as % or absolute | todo | |

## Performance

| Feature | Status | Notes |
| --- | --- | --- |
| CPU graph 60 s, per-logical-processor view | done | |
| CPU utilization, speed, processes, threads, handles, up time | done | |
| CPU name, base speed, sockets, cores, logical processors, L1/L2/L3 cache, virtualization | done | |
| Kernel times overlay | todo | |
| Memory graph, in use, available, committed | done | |
| Memory cached, paged / non-paged pool, composition bar, hardware reserved | done | `GetPerformanceInfo` |
| Memory speed, slots, form factor | todo | SMBIOS |
| One entry per physical disk: active time, response time, read / write speed | done | PDH `PhysicalDisk(*)` |
| Disk capacity, type (SSD / HDD), system disk flag | todo | |
| One entry per adapter: send / receive, link speed | done | `GetIfTable2` |
| Adapter IPv4 / IPv6, SSID | todo | |
| GPU utilization, engines (3D, Copy, Video…), dedicated / shared memory | done | PDH `GPU Engine(*)`, `GPU Adapter Memory(*)` |
| One entry per GPU adapter, adapter name, driver version | todo | one combined GPU entry today |
| NPU | todo | where present |
| Copy to clipboard | done | |
| Summary view | todo | |
| Open Resource Monitor | done | launches `resmon` |

## App history

| Feature | Status | Notes |
| --- | --- | --- |
| CPU time, network, metered network per app since a date | todo | data lives in SRUM (`sru\SRUDB.dat`, ESE database) |
| Delete usage history | todo | |

## Startup apps

| Feature | Status | Notes |
| --- | --- | --- |
| Name, Publisher, Status, Command, Location | done | Run keys (HKCU, HKLM, WOW6432Node) + Startup folders |
| Enable / Disable | done | `StartupApproved`, same store as Windows |
| Startup impact, Disk I/O and CPU at startup | won't (for now) | Windows computes these from boot traces it does not expose |
| Last BIOS time | done | registry `Power\FwPOSTTime` |
| Scheduled-task and UWP startup entries | todo | |
| Open file location, Search online | done | |
| Properties | todo | |

## Users

| Feature | Status | Notes |
| --- | --- | --- |
| Signed-in sessions with status, CPU, memory, disk, process count | done | |
| Network per user | todo | needs per-process network |
| Expand a user to their processes | done | |
| Disconnect, Sign off | done | asks first |

## Details

| Feature | Status | Notes |
| --- | --- | --- |
| Name, PID, user, session, CPU, memory, threads, handles, priority, path, command line | done | |
| Choose columns (~40 in the original) | todo | |
| Same right-click actions as Processes | done | end, tree, efficiency, priority, affinity, dump, search |
| Go to service(s) | done | svchost.exe rows |
| UAC virtualization, Analyze wait chain | todo | |
| Properties | todo | |

## Services

| Feature | Status | Notes |
| --- | --- | --- |
| Name, PID, Description, Status | done | search by name, description or PID |
| Group column | todo | |
| Start, Stop, Restart | done | worker thread |
| Open Services, Search online, Go to details | done | |
