# End-to-end checks of uucc against the demo app (no administrator rights, nothing sent to the laptop). Build first
# (cargo build --release). It starts the demo in the tray, runs each check, then closes the demo by posting its exit
# message to the demo's own tray window only, never a broadcast, which would reach a running app.
$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
$app = "$root\target\release\UnofficialUniwillControlCenter.exe"
$uucc = "$root\target\release\uucc.exe"
$env:UUCC_DEMO = "1"
$script:failed = 0

Add-Type @"
using System; using System.Runtime.InteropServices; using System.Text;
public static class DemoWindow {
    public delegate bool EnumProc(IntPtr h, IntPtr l);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc f, IntPtr l);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] public static extern int GetWindowThreadProcessId(IntPtr h, out int pid);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern uint RegisterWindowMessage(string s);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
    public static IntPtr Tray(int pid) {
        IntPtr found = IntPtr.Zero;
        EnumWindows((h, l) => { int p; GetWindowThreadProcessId(h, out p); var c = new StringBuilder(64); GetClassName(h, c, 64);
            if (p == pid && c.ToString() == "KeyboardRgbTray") { found = h; return false; } return true; }, IntPtr.Zero);
        return found;
    }
}
"@

# Runs uucc; returns its output (stdout and stderr) and exit code.
function U {
    $ErrorActionPreference = "Continue"  # stderr is part of the output here, not a script error
    $out = & $uucc @args 2>&1 | ForEach-Object { "$_" } | Out-String
    [pscustomobject]@{ Out = $out.Trim(); Code = $LASTEXITCODE }
}
function Json { (U @args --json).Out | ConvertFrom-Json }
function Check([string]$name, [scriptblock]$test) {
    try { $ok = & $test } catch { $ok = $false; $name += " ($_)" }
    if ($ok) { Write-Host "PASS  $name" } else { Write-Host "FAIL  $name" -ForegroundColor Red; $script:failed++ }
}

$demo = Start-Process -FilePath $app -ArgumentList "--demo", "--tray" -PassThru
try {
    $deadline = (Get-Date).AddSeconds(20)
    while ((U mode).Code -eq 2 -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 250 }

    # Task 4: performance, battery, setups, display, GPU mode.
    Check "mode gaming" { $r = U mode gaming; $r.Code -eq 0 -and $r.Out -match "Gaming" }
    Check "mode shows gaming" { (U mode).Out -match "Mode: Gaming" }
    Check "mode turbo --json" { (Json mode turbo).mode -eq "Turbo" }
    Check "mode x is code 1" { (U mode x).Code -eq 1 }
    Check "fan-boost on" { (U fan-boost on).Code -eq 0 -and (Json fan-boost).on -eq $true }
    Check "fan-boost off" { (U fan-boost off).Code -eq 0 -and (Json fan-boost).on -eq $false }
    Check "profile lists Standard" { $r = U profile; $r.Code -eq 0 -and $r.Out -match "Standard" }
    Check "profile nosuch is code 1" { (U profile nosuch).Code -eq 1 }
    Check "charge balanced" { (U charge balanced).Code -eq 0 -and (Json charge).charge -eq "balanced" }
    Check "charge full" { (Json charge full).charge -eq "full" }
    Check "setup lists" { (U setup).Code -eq 0 }
    Check "setup nosuch is code 1" { (U setup nosuch).Code -eq 1 }
    Check "display on --mode cinema" { (U display on --mode cinema).Code -eq 0 -and (Json display).mode -eq "Cinema" }
    Check "display off --mode is code 1" { (U display off --mode cinema).Code -eq 1 }
    Check "display off" { (U display off).Code -eq 0 -and (Json display).on -eq $false }
    Check "gpu-mode shows" { (U gpu-mode).Code -eq 0 }
    Check "gpu-mode intel is code 3 in the demo" { $r = U gpu-mode intel; $r.Code -eq 3 -and $r.Out -match "Demo mode" }
} finally {
    $tray = [DemoWindow]::Tray($demo.Id)
    if ($tray -ne [IntPtr]::Zero) { [DemoWindow]::PostMessage($tray, [DemoWindow]::RegisterWindowMessage("TulparControl.Exit"), [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null }
    if (-not $demo.WaitForExit(10000)) { $demo.Kill() }
}
Check "app gone is code 2" { (U mode).Code -eq 2 }

if ($script:failed) { Write-Host "$($script:failed) failed" -ForegroundColor Red; exit 1 }
Write-Host "All passed"
