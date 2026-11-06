<#
.SYNOPSIS
  Smoke-tests every target/release/oxi-*.exe. For an app with apps/<name>/smoke.json, also takes one
  screenshot per listed view, e.g. {"views": [{"name": "performance", "env": {"OXI_TASKMGR_TAB": "performance"}}]}.
#>
param([string] $OutDir = "smoke")
$ErrorActionPreference = "Stop"
$exes = Get-ChildItem target/release -Filter "oxi-*.exe"
if (-not $exes) { throw "no oxi-*.exe built" }
foreach ($e in $exes) {
    $name = $e.BaseName.Substring(4)
    & "$PSScriptRoot/smoke.ps1" -Exe $e.FullName -Name $name -OutDir $OutDir
    $cfg = Join-Path "apps/$name" "smoke.json"
    if (-not (Test-Path $cfg)) { continue }
    foreach ($v in (Get-Content $cfg -Raw | ConvertFrom-Json).views) {
        $settle = if ($v.settle) { [int]$v.settle } else { 3 }
        $saved = @{}
        foreach ($kv in $v.env.PSObject.Properties) {
            $saved[$kv.Name] = [Environment]::GetEnvironmentVariable($kv.Name)
            [Environment]::SetEnvironmentVariable($kv.Name, [string]$kv.Value)
        }
        try {
            & "$PSScriptRoot/smoke.ps1" -Exe $e.FullName -Name "$name-$($v.name)" -OutDir $OutDir -SettleSeconds $settle -ScreenshotOnly
        } finally {
            foreach ($k in $saved.Keys) { [Environment]::SetEnvironmentVariable($k, $saved[$k]) }
        }
    }
}
