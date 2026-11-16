# A plain WinForms window that accepts files, to see what a drag out of an app really delivers.
param([string]$Log, [int]$X, [int]$Y, [int]$W, [int]$H)
Add-Type -AssemblyName System.Windows.Forms
$f = New-Object Windows.Forms.Form
$f.Text = "dnd-target"
$f.StartPosition = 'Manual'
$f.Location = New-Object Drawing.Point($X, $Y)
$f.Size = New-Object Drawing.Size($W, $H)
$f.TopMost = $true
$f.AllowDrop = $true
$f.Add_DragEnter({
    param($s, $e)
    "ENTER formats=$($e.Data.GetFormats() -join ',') allowed=$($e.AllowedEffect)" | Add-Content $Log
    $e.Effect = [Windows.Forms.DragDropEffects]::Copy
})
$f.Add_DragOver({ param($s, $e) $e.Effect = [Windows.Forms.DragDropEffects]::Copy })
$f.Add_DragDrop({
    param($s, $e)
    $files = $e.Data.GetData([Windows.Forms.DataFormats]::FileDrop)
    "DROP files=$($files -join '|')" | Add-Content $Log
})
$t = New-Object Windows.Forms.Timer
$t.Interval = 240000
$t.Add_Tick({ $f.Close() })
$t.Start()
"READY" | Add-Content $Log
[Windows.Forms.Application]::Run($f)
