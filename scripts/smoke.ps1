<#
.SYNOPSIS
  Launches one oxidows app, waits for its window, and records startup time, memory, idle CPU
  and a screenshot. Works for any app in apps/: pass its exe.

.EXAMPLE
  ./scripts/smoke.ps1 -Exe target/release/oxi-taskmgr.exe -Name taskmgr -OutDir smoke
#>
param(
    [Parameter(Mandatory)] [string] $Exe,
    [Parameter(Mandatory)] [string] $Name,
    [string] $OutDir = "smoke",
    [int] $SettleSeconds = 5,
    [int] $IdleSeconds = 10,
    [int] $WindowTimeoutSeconds = 30,
    # Only wait, screenshot and close (extra views of an app; metrics come from the main run).
    [switch] $ScreenshotOnly
)

$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class SmokeWin32 {
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
}
"@
[SmokeWin32]::SetProcessDPIAware() | Out-Null

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$exePath = (Resolve-Path $Exe).Path
$stderr = Join-Path $OutDir "$Name-stderr.txt"

$sw = [Diagnostics.Stopwatch]::StartNew()
$p = Start-Process -FilePath $exePath -PassThru -RedirectStandardError $stderr
$windowMs = $null
while ($sw.Elapsed.TotalSeconds -lt $WindowTimeoutSeconds) {
    $p.Refresh()
    if ($p.HasExited) { break }
    if ($p.MainWindowHandle -ne [IntPtr]::Zero) { $windowMs = [int]$sw.Elapsed.TotalMilliseconds; break }
    Start-Sleep -Milliseconds 10
}
if ($p.HasExited) {
    Get-Content $stderr -ErrorAction SilentlyContinue | Write-Host
    throw "$Name exited during startup with code $($p.ExitCode)"
}
if ($null -eq $windowMs) { Stop-Process -Id $p.Id -Force; throw "$Name showed no window within $WindowTimeoutSeconds s" }

Start-Sleep -Seconds $SettleSeconds
$p.Refresh()
if ($p.HasExited) {
    Get-Content $stderr -ErrorAction SilentlyContinue | Write-Host
    throw "$Name crashed after its window appeared (exit code $($p.ExitCode))"
}

if ($ScreenshotOnly) { $IdleSeconds = 0 }
# Idle CPU: process CPU time over a quiet interval, as a share of the whole machine.
$cpu0 = $p.TotalProcessorTime
$threads0 = @{}
foreach ($t in $p.Threads) { try { $threads0[$t.Id] = $t.TotalProcessorTime.TotalMilliseconds } catch { } }
Start-Sleep -Seconds $IdleSeconds
$p.Refresh()
$cpu1 = $p.TotalProcessorTime
# Which threads used the idle CPU (named threads show their name), to tell rendering from sampling.
if ($IdleSeconds -gt 0) {
    if (-not ('Smoke.Native' -as [type])) {
        Add-Type -Namespace Smoke -Name Native -MemberDefinition @'
[DllImport("kernel32.dll")] public static extern IntPtr OpenThread(uint access, bool inherit, uint id);
[DllImport("kernel32.dll")] public static extern bool CloseHandle(IntPtr h);
[DllImport("kernel32.dll", CharSet = CharSet.Unicode)] public static extern int GetThreadDescription(IntPtr h, out IntPtr desc);
[DllImport("kernel32.dll")] public static extern IntPtr LocalFree(IntPtr p);
public static string Name(uint id) {
    IntPtr h = OpenThread(0x1000, false, id); // THREAD_QUERY_LIMITED_INFORMATION
    if (h == IntPtr.Zero) return "";
    IntPtr d; string s = "";
    if (GetThreadDescription(h, out d) >= 0 && d != IntPtr.Zero) { s = Marshal.PtrToStringUni(d); LocalFree(d); }
    CloseHandle(h); return s;
}
'@
    }
    $busy = foreach ($t in $p.Threads) {
        try {
            $ms = $t.TotalProcessorTime.TotalMilliseconds - ($threads0[$t.Id] ?? 0)
            [pscustomobject]@{ id = $t.Id; ms = [math]::Round($ms); name = [Smoke.Native]::Name([uint32]$t.Id) }
        } catch { }
    }
    Write-Host "Busiest threads over the idle interval (ms of CPU):"
    $busy | Sort-Object ms -Descending | Select-Object -First 6 | ForEach-Object { Write-Host ("  {0,7} ms  tid {1}  {2}" -f $_.ms, $_.id, $_.name) }
    Write-Host "Logical processors: $([Environment]::ProcessorCount)"
}
$idleCpuPct = if ($IdleSeconds -gt 0) { [math]::Round(($cpu1 - $cpu0).TotalMilliseconds / ($IdleSeconds * 1000 * [Environment]::ProcessorCount) * 100, 3) } else { $null }

# "Memory" in Task Manager = private working set.
$privateWs = $null
try {
    $privateWs = (Get-CimInstance Win32_PerfFormattedData_PerfProc_Process -Filter "IDProcess=$($p.Id)").WorkingSetPrivate
} catch { }
$p.Refresh()

# Screenshot of the window.
$shot = Join-Path $OutDir "$Name.png"
try {
    [SmokeWin32]::SetForegroundWindow($p.MainWindowHandle) | Out-Null
    Start-Sleep -Milliseconds 500
    $r = New-Object SmokeWin32+RECT
    [SmokeWin32]::GetWindowRect($p.MainWindowHandle, [ref]$r) | Out-Null
    $w = [math]::Max(1, $r.Right - $r.Left); $h = [math]::Max(1, $r.Bottom - $r.Top)
    $bmp = New-Object System.Drawing.Bitmap $w, $h
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen($r.Left, $r.Top, 0, 0, $bmp.Size)
    $bmp.Save($shot, [System.Drawing.Imaging.ImageFormat]::Png)
    $g.Dispose(); $bmp.Dispose()
} catch { Write-Warning "screenshot failed: $_"; $shot = $null }

$result = [ordered]@{
    app                 = $Name
    window_ms           = $windowMs
    private_ws_mb       = if ($privateWs) { [math]::Round($privateWs / 1MB, 1) } else { $null }
    working_set_mb      = [math]::Round($p.WorkingSet64 / 1MB, 1)
    private_bytes_mb    = [math]::Round($p.PrivateMemorySize64 / 1MB, 1)
    idle_cpu_pct        = $idleCpuPct
    threads             = $p.Threads.Count
    handles             = $p.HandleCount
    exe_mb              = [math]::Round((Get-Item $exePath).Length / 1MB, 2)
    os                  = (Get-CimInstance Win32_OperatingSystem).Caption
}
Stop-Process -Id $p.Id -Force
if ($ScreenshotOnly) {
    if (-not $shot) { throw "${Name}: screenshot failed" }
    Write-Host "${Name}: screenshot saved"
    return
}

$result | ConvertTo-Json | Set-Content (Join-Path $OutDir "$Name.json")
$table = "| Metric | Value |`n| --- | --- |`n" + (($result.GetEnumerator() | ForEach-Object { "| $($_.Key) | $($_.Value) |" }) -join "`n")
$extra = Get-Content $stderr -ErrorAction SilentlyContinue
$md = "### Smoke test: $Name`n`n$table`n"
if ($extra) { $md += "`nApp output:`n``````n$($extra -join "`n")`n```````n" }
$md | Set-Content (Join-Path $OutDir "$Name.md")
if ($env:GITHUB_STEP_SUMMARY) { $md | Add-Content $env:GITHUB_STEP_SUMMARY }
Write-Host $md
