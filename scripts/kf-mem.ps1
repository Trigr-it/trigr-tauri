# Keyfire process-tree memory snapshot (dev tool, RAM wave 2).
#
# Walks every keyfire.exe and the WebView2 processes it owns, classifies each
# by Chromium --type (browser, gpu-process, renderer, utility, crashpad) and
# prints private commit + working set per process, then totals. Private is
# what commits RAM; working set is what Task Manager shows.
#
#   .\scripts\kf-mem.ps1                 # one snapshot
#   .\scripts\kf-mem.ps1 -Label "tray 10 min"   # tag the row for a log
#   .\scripts\kf-mem.ps1 -Csv mem.csv    # append one summary line per run
#
# Debug builds run ~20-30 % heavier than release; compare like with like.

param(
    [string]$Label = "",
    [string]$Csv = ""
)

$all = Get-CimInstance Win32_Process
$roots = $all | Where-Object { $_.Name -ieq 'keyfire.exe' }
if (-not $roots) { Write-Host "No keyfire.exe running."; exit 1 }

$byParent = @{}
foreach ($p in $all) {
    if (-not $byParent.ContainsKey($p.ParentProcessId)) { $byParent[$p.ParentProcessId] = @() }
    $byParent[$p.ParentProcessId] += $p
}

function Get-Descendants([uint32]$procId) {
    $out = @()
    $stack = New-Object System.Collections.Stack
    $stack.Push($procId)
    while ($stack.Count) {
        $cur = $stack.Pop()
        if ($byParent.ContainsKey($cur)) {
            foreach ($c in $byParent[$cur]) { $out += $c; $stack.Push($c.ProcessId) }
        }
    }
    return $out
}

$rows = @()
foreach ($root in $roots) {
    $tree = @($root) + (Get-Descendants $root.ProcessId)
    foreach ($p in $tree) {
        $type = if ($p.Name -ieq 'keyfire.exe') { 'keyfire.exe' }
                elseif ($p.CommandLine -match '--type=([a-z-]+)') { $matches[1] }
                else { 'browser' }
        $rows += [pscustomobject]@{
            Type      = $type
            PID       = $p.ProcessId
            PrivateMB = [math]::Round($p.PrivatePageCount / 1MB, 1)
            WSMB      = [math]::Round($p.WorkingSetSize / 1MB, 1)
        }
    }
}

$rows | Sort-Object Type, PID | Format-Table -AutoSize | Out-String | Write-Host

$byType = $rows | Group-Object Type | ForEach-Object {
    [pscustomobject]@{
        Type      = $_.Name
        Count     = $_.Count
        PrivateMB = [math]::Round(($_.Group | Measure-Object PrivateMB -Sum).Sum, 1)
        WSMB      = [math]::Round(($_.Group | Measure-Object WSMB -Sum).Sum, 1)
    }
}
$byType | Sort-Object Type | Format-Table -AutoSize | Out-String | Write-Host

$totPriv = [math]::Round(($rows | Measure-Object PrivateMB -Sum).Sum, 1)
$totWS   = [math]::Round(($rows | Measure-Object WSMB -Sum).Sum, 1)
$stamp = Get-Date -Format 'yyyy-MM-dd HH:mm:ss'
Write-Host ("TOTAL  {0} processes  private {1} MB  working set {2} MB  {3} {4}" -f $rows.Count, $totPriv, $totWS, $stamp, $Label)

if ($Csv) {
    $line = "{0},{1},{2},{3},{4}" -f $stamp, $Label, $rows.Count, $totPriv, $totWS
    if (-not (Test-Path $Csv)) { "time,label,processes,private_mb,ws_mb" | Out-File -FilePath $Csv -Encoding utf8 }
    $line | Out-File -FilePath $Csv -Encoding utf8 -Append
}
