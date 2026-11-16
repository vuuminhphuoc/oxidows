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
[DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int cx, int cy, uint flags);
'@
[Dnd.Win]::SetProcessDPIAware() | Out-Null
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$OutDir = (Resolve-Path $OutDir).Path
$exePath = (Resolve-Path $Exe).Path

# The runner's screenshots can't be downloaded, so print a small one into the log.
function Dump-Shot($name) {
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
Start-Sleep -Seconds 3
# The runner's own terminal windows are in the way: minimise everything, then keep ours on top.
(New-Object -ComObject Shell.Application).MinimizeAll()
Start-Sleep -Seconds 1
[Dnd.Win]::MoveWindow($p.MainWindowHandle, 0, 0, $ow, $oh, $true) | Out-Null
# HWND_TOPMOST = -1; SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW
[Dnd.Win]::SetWindowPos($p.MainWindowHandle, [IntPtr]::new(-1), 0, 0, 0, 0, 0x0043) | Out-Null
[Dnd.Win]::SetForegroundWindow($p.MainWindowHandle) | Out-Null
Start-Sleep -Seconds 2

$tx = $ow + 20; $tw = [Math]::Max(300, $screen.Width - $tx - 10)
$tlog = Join-Path $OutDir "target.log"
$slog = Join-Path $OutDir "source.log"
Remove-Item $tlog, $slog -ErrorAction SilentlyContinue
$ps = (Get-Command powershell.exe).Source
$target = Start-Process $ps -ArgumentList "-NoProfile -STA -File `"$PSScriptRoot\dnd\target.ps1`" -Log `"$tlog`" -X $tx -Y 20 -W $tw -H 300" -PassThru
if (-not (Wait-Log $tlog "READY" 30)) { throw "target window did not start" }
Start-Sleep -Seconds 1
Shot "before"
Dump-Shot "before"
Mouse-To 5 5; Write-Host "Cursor after SetCursorPos: $([Windows.Forms.Cursor]::Position)"

$failures = @()

# --- Drag out of oxi-explorer into the other program -------------------------------------
$cx = $tx + $tw / 2; $cy = 160
$hit = $null
foreach ($y in 150..330 | Where-Object { $_ % 12 -eq 0 }) {
    foreach ($x in 330) {
        Remove-Item $tlog -ErrorAction SilentlyContinue
        Mouse-To $x $y; Start-Sleep -Milliseconds 150
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
$source = Start-Process $ps -ArgumentList "-NoProfile -STA -File `"$PSScriptRoot\dnd\source.ps1`" -Log `"$slog`" -File `"$incoming`" -X $sx -Y 20 -W 300 -H 200" -PassThru
if (-not (Wait-Log $slog "READY" 30)) { throw "source window did not start" }
Start-Sleep -Seconds 1
Mouse-To ($sx + 150) 120; Start-Sleep -Milliseconds 200
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
