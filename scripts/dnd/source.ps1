# A plain WinForms window that starts a file drag, to see what the app does with files dropped on it.
param([string]$Log, [string]$File, [int]$X, [int]$Y, [int]$W, [int]$H)
Add-Type -AssemblyName System.Windows.Forms
$f = New-Object Windows.Forms.Form
$f.Text = "dnd-source"
$f.StartPosition = 'Manual'
$f.Location = New-Object Drawing.Point($X, $Y)
$f.Size = New-Object Drawing.Size($W, $H)
$f.TopMost = $true
$l = New-Object Windows.Forms.Label
$l.Text = "drag me"
$l.Dock = 'Fill'
$l.BackColor = [Drawing.Color]::LightGreen
$l.Add_MouseDown({
    $data = New-Object Windows.Forms.DataObject
    $data.SetData([Windows.Forms.DataFormats]::FileDrop, [string[]]@($File))
    $r = $l.DoDragDrop($data, [Windows.Forms.DragDropEffects]'Copy, Move, Link')
    "RESULT=$r" | Add-Content $Log
})
$f.Controls.Add($l)
$t = New-Object Windows.Forms.Timer
$t.Interval = 240000
$t.Add_Tick({ $f.Close() })
$t.Start()
"READY" | Add-Content $Log
[Windows.Forms.Application]::Run($f)
