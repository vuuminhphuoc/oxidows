<#
.SYNOPSIS
  End-to-end drag and drop check for oxi-explorer on a real desktop: a drag out of the file list
  into another program (a plain WinForms window), and a drag from another program into the window.
  The mouse is driven with real input events, so the system's own drag loop runs.

.EXAMPLE
  ./scripts/dnd-test.ps1 -Exe target/release/oxi-explorer.exe -OutDir dnd
#>
param(
    [Parameter(Mandatory)] [string] $Exe,
    [string] $OutDir = "dnd"
)
$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
Add-Type -Namespace Dnd -Name Win -MemberDefinition @'
[DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
[DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
[DllImport("user32.dll")] public static extern void mouse_event(uint f, int dx, int dy, uint d, UIntPtr e);
[DllImport("user32.dll")] public static extern bool MoveWindow(IntPtr h, int x, int y, int w, int hh, bool repaint);
[DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
[StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
[DllImport("user32.dll")] public static extern IntPtr WindowFromPoint(POINT p);
[DllImport("user32.dll")] public static extern IntPtr GetAncestor(IntPtr h, uint flags);
[DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
[DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
[StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
[DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
[DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
[DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
[DllImport("user32.dll")] public static extern int GetWindowLong(IntPtr h, int i);
[DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr h, int attr, out int v, int size);
[DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int cx, int cy, uint flags);
'@
Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
public static class DndEnum {
    delegate bool EnumProc(IntPtr h, IntPtr l);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc p, IntPtr l);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] static extern int GetClassName(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] static extern int GetWindowLong(IntPtr h, int i);
    [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr h, out RECT r);
    [StructLayout(LayoutKind.Sequential)] struct RECT { public int Left, Top, Right, Bottom; }
    // Every top-level window of a process: "handle|class|title|style|exstyle|visible|l,t,r,b"
    public static string[] Of(uint pid) {
        var res = new List<string>();
        EnumWindows((h, l) => {
            uint p; GetWindowThreadProcessId(h, out p);
            if (p != pid) return true;
            var c = new StringBuilder(256); GetClassName(h, c, 256);
            var t = new StringBuilder(256); GetWindowText(h, t, 256);
            RECT r; GetWindowRect(h, out r);
            res.Add(string.Format("{0}|{1}|{2}|0x{3:x}|0x{4:x}|{5}|{6},{7},{8},{9}", h.ToInt64(), c, t,
                GetWindowLong(h, -16), GetWindowLong(h, -20), IsWindowVisible(h), r.Left, r.Top, r.Right, r.Bottom));
            return true;
        }, IntPtr.Zero);
        return res.ToArray();
    }
}
'@
[Dnd.Win]::SetProcessDPIAware() | Out-Null
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$OutDir = (Resolve-Path $OutDir).Path
$exePath = (Resolve-Path $Exe).Path

function Under($x, $y) {
    $pt = New-Object Dnd.Win+POINT
    $pt.X = [int]$x; $pt.Y = [int]$y
    $h = [Dnd.Win]::GetAncestor([Dnd.Win]::WindowFromPoint($pt), 2)
    $procId = [uint32]0
    [Dnd.Win]::GetWindowThreadProcessId($h, [ref]$procId) | Out-Null
    $name = (Get-Process -Id $procId -ErrorAction SilentlyContinue).ProcessName
    $r = New-Object Dnd.Win+RECT
    [Dnd.Win]::GetWindowRect($h, [ref]$r) | Out-Null
    "$name [$($r.Left),$($r.Top) $($r.Right),$($r.Bottom)]"
}
function Rect-Of($h) {
    $r = New-Object Dnd.Win+RECT
    [Dnd.Win]::GetWindowRect($h, [ref]$r) | Out-Null
    "[$($r.Left),$($r.Top) $($r.Right),$($r.Bottom)]"
}
# The runner's screenshots can't be downloaded; only dump one into the log when asked (large).
function Dump-Shot($name) {
    if (-not $env:DND_DUMP) { return }
    $b = [Windows.Forms.Screen]::PrimaryScreen.Bounds
    $bmp = New-Object Drawing.Bitmap $b.Width, $b.Height
    $g = [Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen($b.Location, [Drawing.Point]::Empty, $b.Size)
    $small = New-Object Drawing.Bitmap $bmp, ([int]($b.Width / 2)), ([int]($b.Height / 2))
    $codec = [Drawing.Imaging.ImageCodecInfo]::GetImageEncoders() | Where-Object MimeType -eq "image/jpeg"
    $ep = New-Object Drawing.Imaging.EncoderParameters 1
    $ep.Param[0] = New-Object Drawing.Imaging.EncoderParameter ([Drawing.Imaging.Encoder]::Quality), 40L
    $ms = New-Object IO.MemoryStream
    $small.Save($ms, $codec, $ep)
    $text = [Convert]::ToBase64String($ms.ToArray())
    Write-Host "SHOT-BEGIN $name"
    for ($i = 0; $i -lt $text.Length; $i += 3000) { Write-Host $text.Substring($i, [Math]::Min(3000, $text.Length - $i)) }
    Write-Host "SHOT-END $name"
    $g.Dispose(); $bmp.Dispose(); $small.Dispose()
}
function Shot($name) {
    $b = [Windows.Forms.Screen]::PrimaryScreen.Bounds
    $bmp = New-Object Drawing.Bitmap $b.Width, $b.Height
    $g = [Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen($b.Location, [Drawing.Point]::Empty, $b.Size)
    $bmp.Save((Join-Path $OutDir "$name.png"), [Drawing.Imaging.ImageFormat]::Png)
    $g.Dispose(); $bmp.Dispose()
}
function Mouse-To($x, $y) { [Dnd.Win]::SetCursorPos([int]$x, [int]$y) | Out-Null }
function Mouse-Down { [Dnd.Win]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero) }
function Mouse-Up { [Dnd.Win]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero) }
function Mouse-Glide($x0, $y0, $x1, $y1, $steps = 25) {
    for ($i = 1; $i -le $steps; $i++) {
        Mouse-To ($x0 + ($x1 - $x0) * $i / $steps) ($y0 + ($y1 - $y0) * $i / $steps)
        Start-Sleep -Milliseconds 25
    }
}
function Wait-Log($path, $pattern, $seconds) {
    $end = (Get-Date).AddSeconds($seconds)
    while ((Get-Date) -lt $end) {
        if ((Test-Path $path) -and (Select-String -Path $path -Pattern $pattern -Quiet)) { return $true }
        Start-Sleep -Milliseconds 100
    }
    return $false
}

$screen = [Windows.Forms.Screen]::PrimaryScreen.Bounds
Write-Host "Screen: $($screen.Width)x$($screen.Height)"
$root = Join-Path $env:RUNNER_TEMP "dnd"
if (-not $env:RUNNER_TEMP) { $root = Join-Path ([IO.Path]::GetTempPath()) "dnd" }
Remove-Item -Recurse -Force $root -ErrorAction SilentlyContinue
$src = Join-Path $root "src"
$inbox = Join-Path $root "inbox"
New-Item -ItemType Directory -Force -Path "$src\aaa-folder", $inbox | Out-Null
$names = 1..3 | ForEach-Object { "dnd-sample-file-number-$_.txt" }
foreach ($n in $names) { "hello $n" | Set-Content (Join-Path $src $n) }
$incoming = Join-Path $inbox "incoming-from-other-app.txt"
"incoming" | Set-Content $incoming

# Known window size and view, whatever settings the machine has.
$cfgDir = Join-Path $env:APPDATA "oxidows"
New-Item -ItemType Directory -Force -Path $cfgDir | Out-Null
"width=620`nheight=520`nmaximized=false`n" | Set-Content (Join-Path $cfgDir "explorer.ini")

$ow = 640; $oh = 560
$stderr = Join-Path $OutDir "explorer-stderr.txt"
$env:OXI_DND_DEBUG = "1"
$p = Start-Process -FilePath $exePath -ArgumentList "`"$src`"" -PassThru -RedirectStandardError $stderr
$end = (Get-Date).AddSeconds(30)
while ((Get-Date) -lt $end -and $p.MainWindowHandle -eq [IntPtr]::Zero) { Start-Sleep -Milliseconds 100; $p.Refresh() }
if ($p.MainWindowHandle -eq [IntPtr]::Zero) { throw "no window" }
# MainWindowHandle may be a helper window; the real one has the title.
function Oxi-Hwnd {
    foreach ($w in [DndEnum]::Of([uint32]$p.Id)) {
        $f = $w -split '\|'
        if ($f[2] -like "*File Explorer (oxidows)" -and $f[1] -eq "Window Class") { return [IntPtr][int64]$f[0] }
    }
    return $p.MainWindowHandle
}
$hw = Oxi-Hwnd
Start-Sleep -Seconds 3
$hw = Oxi-Hwnd
# The runner's own terminal windows are in the way: minimise everything, then keep ours on top.
(New-Object -ComObject Shell.Application).MinimizeAll()
foreach ($t in Get-Process WindowsTerminal, conhost, OpenConsole -ErrorAction SilentlyContinue) {
    if ($t.MainWindowHandle -ne [IntPtr]::Zero) {
        # SW_MINIMIZE = 6
        [Dnd.Win]::ShowWindow($t.MainWindowHandle, 6) | Out-Null
        Write-Host "minimised $($t.ProcessName) $($t.Id)"
    }
}
Start-Sleep -Seconds 1
# SW_RESTORE = 9: minimise-all also minimised our window.
[Dnd.Win]::ShowWindow($hw, 9) | Out-Null
[Dnd.Win]::MoveWindow($hw, 0, 0, $ow, $oh, $true) | Out-Null
# HWND_TOPMOST = -1; SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW
[Dnd.Win]::SetWindowPos($hw, [IntPtr]::new(-1), 0, 0, 0, 0, 0x0043) | Out-Null
[Dnd.Win]::SetForegroundWindow($hw) | Out-Null
Start-Sleep -Seconds 2
function Describe-Window($h) {
    $cloaked = 0
    [Dnd.Win]::DwmGetWindowAttribute($h, 14, [ref]$cloaked, 4) | Out-Null
    "visible=$([Dnd.Win]::IsWindowVisible($h)) iconic=$([Dnd.Win]::IsIconic($h)) style=0x{0:x} exstyle=0x{1:x} cloaked=$cloaked" -f [Dnd.Win]::GetWindowLong($h, -16), [Dnd.Win]::GetWindowLong($h, -20)
}
$p.Refresh()
foreach ($w in [DndEnum]::Of([uint32]$p.Id)) { Write-Host "  oxi window: $w" }
Write-Host "oxi: exited=$($p.HasExited) hwnd=$($hw) $(Describe-Window $hw)"
Get-Content $stderr -ErrorAction SilentlyContinue | Select-Object -First 20 | ForEach-Object { Write-Host "  oxi stderr: $_" }
Dump-Shot "oxi-placed"

$tx = $ow + 20; $tw = [Math]::Max(300, $screen.Width - $tx - 10)
$tlog = Join-Path $OutDir "target.log"
$slog = Join-Path $OutDir "source.log"
Remove-Item $tlog, $slog -ErrorAction SilentlyContinue
$ps = (Get-Command powershell.exe).Source
$target = Start-Process $ps -NoNewWindow -ArgumentList "-NoProfile -STA -File `"$PSScriptRoot\dnd\target.ps1`" -Log `"$tlog`" -X $tx -Y 20 -W $tw -H 300" -PassThru
if (-not (Wait-Log $tlog "READY" 30)) { throw "target window did not start" }
Start-Sleep -Seconds 1
Shot "before"
$target.Refresh()
Write-Host "oxi window $(Rect-Of $hw), target window $(Rect-Of $target.MainWindowHandle)"
foreach ($pt in @(@(330, 200), @(450, 470), @(($tx + 100), 100))) {
    Mouse-To $pt[0] $pt[1]
    Start-Sleep -Milliseconds 200
    Write-Host ("Point {0},{1}: cursor {2}, window under it: {3}" -f $pt[0], $pt[1], [Windows.Forms.Cursor]::Position, (Under $pt[0] $pt[1]))
}

$failures = @()

# --- Drag out of oxi-explorer into the other program -------------------------------------
$cx = $tx + $tw / 2; $cy = 160
$hit = $null
foreach ($y in 150..330 | Where-Object { $_ % 12 -eq 0 }) {
    foreach ($x in 330) {
        Remove-Item $tlog -ErrorAction SilentlyContinue
        Mouse-To $x $y; Start-Sleep -Milliseconds 150
        if (-not $script:loggedPress) { $script:loggedPress = $true; Write-Host "first press at $x,$y, window under it: $(Under $x $y)" }
        Mouse-Down; Start-Sleep -Milliseconds 150
        Mouse-Glide $x $y ($x + 30) $y 6
        Mouse-Glide ($x + 30) $y $cx $cy 40
        Start-Sleep -Milliseconds 500
        Mouse-Up
        if (Wait-Log $tlog "DROP" 2) { $hit = "$x,$y"; break }
        # The mouse went up outside any target or the drag never left the window; get clear.
        Start-Sleep -Milliseconds 300
    }
    if ($hit) { break }
}
Shot "after-drag-out"
Dump-Shot "after-drag-out"
Get-Content $stderr -ErrorAction SilentlyContinue | Where-Object { $_ -match "dnd|drop target" } | ForEach-Object { Write-Host "  app: $_" }
if ($hit) {
    Write-Host "Drag out worked, pressed at $hit"
    Get-Content $tlog | ForEach-Object { Write-Host "  $_" }
    $dropLine = (Get-Content $tlog | Where-Object { $_ -like "DROP*" })[0]
    if ($dropLine -notmatch "dnd-sample-file-number-\d\.txt") { $failures += "drag out delivered the wrong files: $dropLine" }
} else {
    $failures += "drag out: the other program never received a drop"
    if (Test-Path $tlog) { Get-Content $tlog | ForEach-Object { Write-Host "  target: $_" } }
}
Stop-Process -Id $target.Id -Force -ErrorAction SilentlyContinue

# --- Drag from another program into oxi-explorer ---------------------------------------
$sx = $ow + 20
$source = Start-Process $ps -NoNewWindow -ArgumentList "-NoProfile -STA -File `"$PSScriptRoot\dnd\source.ps1`" -Log `"$slog`" -File `"$incoming`" -X $sx -Y 20 -W 300 -H 200" -PassThru
if (-not (Wait-Log $slog "READY" 30)) { throw "source window did not start" }
Start-Sleep -Seconds 1
Mouse-To ($sx + 150) 120; Start-Sleep -Milliseconds 200
Write-Host "drag-in press, window under it: $(Under ($sx + 150) 120); release point under: $(Under 460 470)"
Mouse-Down; Start-Sleep -Milliseconds 150
Mouse-Glide ($sx + 150) 120 ($sx + 120) 130 6
Mouse-Glide ($sx + 120) 130 460 470 40
Start-Sleep -Milliseconds 600
Mouse-Up
Start-Sleep -Seconds 2
Shot "after-drag-in"
Dump-Shot "after-drag-in"
if (Test-Path $slog) { Get-Content $slog | ForEach-Object { Write-Host "  source: $_" } }
$arrived = Test-Path (Join-Path $src "incoming-from-other-app.txt")
Write-Host "Drag in: file arrived in the folder = $arrived"
if (-not $arrived) { $failures += "drag in: the dropped file never reached the folder" }
Stop-Process -Id $source.Id -Force -ErrorAction SilentlyContinue

Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue
Get-Content $stderr -ErrorAction SilentlyContinue | Where-Object { $_ -match "dnd|drop target" } | Select-Object -Last 40 | ForEach-Object { Write-Host "  app: $_" }
if ($failures) { $failures | ForEach-Object { Write-Host "FAIL: $_" }; exit 1 }
Write-Host "Drag and drop OK"
