# Task Manager parity checklist

Compared against Windows 11 Task Manager (24H2). Behaviour is matched by observation only.
Status: **done** · **partial** · **todo** · **won't** (with reason). Update this file in the same commit
as the feature.

## Global

| Feature | Status | Notes |
| --- | --- | --- |
| Tabs: Processes, Performance, App history, Startup apps, Users, Details, Services | partial | App history todo |
| Real-time update speed (High / Normal / Low / Paused) | done | 0.5 s / 1 s / 2 s / paused |
| Run as administrator | done | relaunch through UAC |
| Run new task (with "create with administrative privileges") | todo | |
| Search box | done | per tab |
| Status bar (process count, CPU, memory) | done | |
| Settings: default start page, always on top, minimize on use, hide when minimized | todo | |
| Settings: theme (light / dark / system) | todo | follows egui default (dark) today |
| Settings persisted between runs | todo | |
| Replace Ctrl+Shift+Esc / taskmgr.exe | todo | opt-in, needs admin |
| Notification-area icon with CPU usage | todo | |
| Keyboard: Delete = End task, Ctrl+Tab between tabs | partial | Delete done |

## Processes

| Feature | Status | Notes |
| --- | --- | --- |
| Name, PID, CPU, Memory, Disk columns | done | memory = private working set |
| Threads column | done | |
| Heat-map colouring | done | CPU, Memory |
| Sort by any column | done | |
| End task | done | button, Delete key, right-click |
| Groups: Apps / Background processes / Windows processes | todo | |
| Expand an app to its child processes and windows | todo | |
| Network column | todo | needs per-process ETW network accounting |
| GPU, GPU engine columns | todo | PDH `GPU Engine(pid_*)` |
| Status column (Suspended, Efficiency mode) | todo | |
| Publisher column | todo | version resource CompanyName |
| Power usage, Power usage trend | won't (for now) | no public per-process energy API |
| Type, Process name, Command line columns | partial | command line in Details |
| Efficiency mode | todo | `SetProcessInformation(ProcessPowerThrottling)` + low priority |
| Open file location, Copy command line | done | right-click |
| Set priority | done | right-click |
| Search online | todo | |
| Create memory dump file | todo | `MiniDumpWriteDump` |
| Go to details | todo | |
| Restart (for Windows Explorer) | todo | |
| Resource values as % or absolute | todo | |

## Performance

| Feature | Status | Notes |
| --- | --- | --- |
| CPU graph 60 s, per-logical-processor view | done | |
| CPU utilization, processes, threads, handles, up time | done | |
| CPU name, speed, base speed, sockets, cores, L1/L2/L3 cache, virtualization | todo | |
| Kernel times overlay | todo | |
| Memory graph, in use, available, committed | done | |
| Memory cached, paged / non-paged pool, composition bar | todo | `GetPerformanceInfo` |
| Memory speed, slots, form factor, hardware reserved | todo | SMBIOS |
| Disk: total active time, read / write speed | done | `_Total` instance |
| One entry per physical disk, response time, capacity, type, system disk | todo | |
| Network: total send / receive | done | |
| One entry per adapter (Ethernet, Wi-Fi) with name, IPv4 / IPv6, SSID | todo | |
| GPU 3D utilization | done | |
| GPU per adapter, engines (3D, Copy, Video), dedicated / shared memory, driver | todo | |
| NPU | todo | where present |
| Summary view, copy to clipboard | todo | |
| Open Resource Monitor | todo | launches `resmon` |

## App history

| Feature | Status | Notes |
| --- | --- | --- |
| CPU time, network, metered network per app since a date | todo | data lives in SRUM (`sru\SRUDB.dat`) |
| Delete usage history | todo | |

## Startup apps

| Feature | Status | Notes |
| --- | --- | --- |
| Name, Status, Command, Location | done | Run keys (HKCU, HKLM, WOW6432Node) + Startup folders |
| Enable / Disable | done | `StartupApproved`, same store as Windows |
| Publisher | todo | |
| Startup impact, Disk I/O and CPU at startup | won't (for now) | Windows computes these from boot traces it does not expose |
| Last BIOS time | todo | registry `Power\FwPOSTTime` |
| Scheduled-task and UWP startup entries | todo | |
| Open file location, Search online, Properties | todo | |

## Users

| Feature | Status | Notes |
| --- | --- | --- |
| Signed-in sessions with CPU, memory, process count | done | |
| Disk, network per user | todo | |
| Expand a user to their processes | todo | |
| Disconnect, Sign off | todo | `WTSDisconnectSession`, `WTSLogoffSession` |

## Details

| Feature | Status | Notes |
| --- | --- | --- |
| Name, PID, user, session, CPU, memory, threads, handles, priority, path, command line | done | |
| Choose columns (~40 in the original) | todo | |
| End task, Set priority, Open file location | done | |
| End process tree | todo | |
| Set affinity | todo | |
| Efficiency mode, UAC virtualization, Analyze wait chain | todo | |
| Create dump file, Search online, Properties, Go to service(s) | todo | |

## Services

| Feature | Status | Notes |
| --- | --- | --- |
| Name, PID, Description, Status | done | |
| Group column | todo | |
| Start, Stop, Restart | done | worker thread |
| Open Services, Search online, Go to details | todo | |
