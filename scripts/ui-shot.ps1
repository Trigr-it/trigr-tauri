<#
.SYNOPSIS
  Dev-only Keyfire UI driver: run bridge steps against the running dev app, then
  screenshot the Keyfire window. Pairs with scripts/vite-dev-bridge.mjs +
  src/devBridge.js. Requires `cargo tauri dev` (Vite on localhost:5173).

.EXAMPLE
  # Dark theme, radial view, screenshot, then restore light + keyboard
  .\scripts\ui-shot.ps1 -Steps "setTheme|dark","setArea|mapping|radial" -After "setTheme|light","setView|keyboard" -Out shot.png

  # Click a DOM element and read some text, no screenshot
  .\scripts\ui-shot.ps1 -Steps "click|.view-tab[title='Mouse']","text|.keyboard-label" -NoShot

  # Screenshot just one element, window forced to 1280x800 for the shot
  .\scripts\ui-shot.ps1 -Selector ".trig-header" -Size 1280x800 -Out header.png

  # List connected windows / run raw JS
  .\scripts\ui-shot.ps1 -Windows
  .\scripts\ui-shot.ps1 -Eval "window.__kf_dev.getState()" -NoShot

.NOTES
  Step format: "fn|arg1|arg2|..." — args are parsed as JSON when they look like
  JSON (numbers, true/false/null, {..}, [..], "quoted"), otherwise as strings.
  Steps run in order; a failing step aborts the run (After steps still run).
  Output is one JSON object per step, then the screenshot path.
#>
[CmdletBinding()]
param(
  [string[]] $Steps = @(),
  [string[]] $After = @(),
  [string]   $Eval,
  [string]   $Target = 'main',
  [string]   $Out,
  [string]   $Selector,
  [string]   $Size,
  [switch]   $NoShot,
  [switch]   $NoFocus,
  [switch]   $Windows,
  [int]      $SettleMs = 350,
  [string]   $Bridge = 'http://localhost:5173/__kf_dev'  # Vite binds ::1 on this box; localhost resolves either way
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
if (-not ('KfUi' -as [type])) {
Add-Type @"
using System; using System.Runtime.InteropServices;
public struct KfRect { public int L, T, R, B; }
public struct KfPoint { public int X, Y; }
public static class KfUi {
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out KfRect r);
  [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, out KfRect r);
  [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr h, ref KfPoint p);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
  [DllImport("user32.dll")] public static extern bool MoveWindow(IntPtr h, int x, int y, int w, int hh, bool repaint);
  [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
  [DllImport("user32.dll")] public static extern bool IsZoomed(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr ctx);
}
"@
}
# Per-monitor-v2 DPI awareness for this thread so GetWindowRect / CopyFromScreen
# work in physical pixels on scaled displays (ignored on old Windows).
try { [KfUi]::SetThreadDpiAwarenessContext([IntPtr](-4)) | Out-Null } catch {}

function Parse-Arg([string] $s) {
  if ($s -match '^(-?\d+(\.\d+)?|true|false|null|\{.*\}|\[.*\]|".*")$') {
    try { return (ConvertFrom-Json -InputObject $s) } catch { return $s }
  }
  return $s
}

function Invoke-Bridge([string] $fn, [object[]] $fnArgs, [string] $tgt = $Target) {
  $body = @{ target = $tgt; fn = $fn; args = @($fnArgs) } | ConvertTo-Json -Compress -Depth 10
  try {
    $resp = Invoke-RestMethod -Method Post -Uri $Bridge -ContentType 'application/json' -Body $body -TimeoutSec 15
  } catch {
    throw "bridge unreachable at $Bridge - is 'cargo tauri dev' running? ($($_.Exception.Message))"
  }
  return $resp
}

function Run-Step([string] $step) {
  $parts = $step -split '\|'
  $fn = $parts[0].Trim()
  $fnArgs = @()
  if ($parts.Length -gt 1) { $fnArgs = @($parts[1..($parts.Length - 1)] | ForEach-Object { Parse-Arg $_ }) }
  $r = Invoke-Bridge $fn $fnArgs
  $line = [ordered]@{ step = $step; ok = $r.ok }
  if ($r.ok) { $line.result = $r.result } else { $line.error = $r.error }
  [Console]::Out.WriteLine(($line | ConvertTo-Json -Compress -Depth 10))
  if (-not $r.ok) { throw "step failed: $step -> $($r.error)" }
  return $r.result
}

if ($Windows) {
  $r = Invoke-Bridge '__windows' @() '*'
  [Console]::Out.WriteLine(($r | ConvertTo-Json -Depth 10))
  return
}

$hwnd = [IntPtr]::Zero
$origRect = $null
$wasMaximised = $false
if (-not $NoShot -or $Size) {
  $p = Get-Process keyfire -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1
  if (-not $p) { throw "keyfire.exe has no main window (not running, or hidden in the tray)" }
  $hwnd = $p.MainWindowHandle
  if ([KfUi]::IsIconic($hwnd)) { [KfUi]::ShowWindow($hwnd, 9) | Out-Null }
  if (-not $NoFocus) { [KfUi]::SetForegroundWindow($hwnd) | Out-Null }
  if ($Size) {
    if ($Size -notmatch '^(\d+)x(\d+)$') { throw "-Size must look like 1280x800" }
    $wasMaximised = [KfUi]::IsZoomed($hwnd)
    if ($wasMaximised) { [KfUi]::ShowWindow($hwnd, 9) | Out-Null; Start-Sleep -Milliseconds 200 }  # un-maximise so MoveWindow sticks
    $origRect = New-Object KfRect; [KfUi]::GetWindowRect($hwnd, [ref]$origRect) | Out-Null
    [KfUi]::MoveWindow($hwnd, $origRect.L, $origRect.T, [int]$Matches[1], [int]$Matches[2], $true) | Out-Null
    Start-Sleep -Milliseconds 400
  }
}

$failed = $null
try {
  foreach ($s in $Steps) { Run-Step $s | Out-Null }
  if ($Eval) { Run-Step ("eval|" + $Eval) | Out-Null }

  if (-not $NoShot) {
    Start-Sleep -Milliseconds $SettleMs
    $r = New-Object KfRect; [KfUi]::GetWindowRect($hwnd, [ref]$r) | Out-Null
    $x = $r.L; $y = $r.T; $w = $r.R - $r.L; $h = $r.B - $r.T
    if ($Selector) {
      $rect = Run-Step ("rect|" + $Selector)
      $origin = New-Object KfPoint; [KfUi]::ClientToScreen($hwnd, [ref]$origin) | Out-Null
      $dpr = [double]$rect.dpr
      $x = $origin.X + [int][math]::Floor($rect.x * $dpr) - 2
      $y = $origin.Y + [int][math]::Floor($rect.y * $dpr) - 2
      $w = [int][math]::Ceiling($rect.width * $dpr) + 4
      $h = [int][math]::Ceiling($rect.height * $dpr) + 4
    }
    if ($w -le 0 -or $h -le 0) { throw "empty capture rect ($w x $h)" }
    if (-not $Out) {
      $stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
      $dir = if ($env:CLAUDE_SCRATCHPAD) { $env:CLAUDE_SCRATCHPAD } else { Join-Path $env:TEMP 'keyfire-ui-shots' }
      New-Item -ItemType Directory -Force $dir | Out-Null
      $Out = Join-Path $dir "ui-shot-$stamp.png"
    }
    $bmp = New-Object System.Drawing.Bitmap $w, $h
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen($x, $y, 0, 0, $bmp.Size)
    $bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
    $g.Dispose(); $bmp.Dispose()
    [Console]::Out.WriteLine((@{ shot = (Resolve-Path $Out).Path; width = $w; height = $h } | ConvertTo-Json -Compress))
  }
} catch {
  $failed = $_
} finally {
  foreach ($s in $After) { try { Run-Step $s | Out-Null } catch { [Console]::Out.WriteLine((@{ after = $s; error = "$_" } | ConvertTo-Json -Compress)) } }
  if ($origRect) {
    [KfUi]::MoveWindow($hwnd, $origRect.L, $origRect.T, $origRect.R - $origRect.L, $origRect.B - $origRect.T, $true) | Out-Null
    if ($wasMaximised) { [KfUi]::ShowWindow($hwnd, 3) | Out-Null }
  }
}
if ($failed) { throw $failed }
