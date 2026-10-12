# Test: let the laptop EC forward the keyboard-light Fn keys to software (like Control Center's GCUService does)
# and apply them instantly over HID. MUST be run from an elevated (admin) Windows PowerShell.
#
# What it does:
#   1. Refuses to run if Control Center's GCUService is running.
#   2. Sets "AP exist" bit 0 of EC register 0x741 via the BIOS WMI method AcpiTest_MULong.GetSetULong.
#   3. For $Seconds seconds, handles AcpiTest_EventULong events: 177 = down, 178 = up, 185 = cycle,
#      setting brightness with HID command 09h. Every event is logged with timings.
#   4. ALWAYS clears the bit again at the end (also on Ctrl+C / errors) and reads it back.
param(
    [int]$Seconds = 60,
    [string]$Log = "$env:USERPROFILE\Desktop\MonsterKeyboardRGB\tools\fn-ap-test.log"
)
$ErrorActionPreference = 'Stop'
$KeyboardPath = '\\?\HID#VID_048D&PID_CE00&MI_01#7&4ced108&0&0000#{4d1e55b2-f16f-11cf-88cb-001111000030}'
$ApOemByte = 0x741

function Write-Log([string]$text) {
    $line = "{0:HH:mm:ss.fff}  {1}" -f (Get-Date), $text
    Write-Host $line
    Add-Content -Path $Log -Value $line
}

Add-Type @"
using System; using System.Runtime.InteropServices; using Microsoft.Win32.SafeHandles;
public static class Kb {
    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    static extern SafeFileHandle CreateFileW(string p, uint a, uint s, IntPtr sec, uint c, uint f, IntPtr t);
    [DllImport("hid.dll", SetLastError = true)] static extern bool HidD_SetFeature(SafeFileHandle h, byte[] b, int n);
    [DllImport("hid.dll", SetLastError = true)] static extern bool HidD_GetFeature(SafeFileHandle h, byte[] b, int n);
    static SafeFileHandle handle;
    public static void Open(string path) { handle = CreateFileW(path, 0xC0000000, 3, IntPtr.Zero, 3, 0, IntPtr.Zero); if (handle.IsInvalid) throw new Exception("open failed " + Marshal.GetLastWin32Error()); }
    public static void Send(params byte[] cmd) { var r = new byte[9]; Array.Copy(cmd, 0, r, 1, cmd.Length); if (!HidD_SetFeature(handle, r, 9)) throw new Exception("set failed " + Marshal.GetLastWin32Error()); }
    public static byte Light() { Send(0x88); var r = new byte[9]; HidD_GetFeature(handle, r, 9); return r[5]; }
}
"@

$wmi = New-Object System.Management.ManagementObject 'root\WMI', "AcpiTest_MULong.InstanceName='ACPI\PNP0C14\1_1'", $null
function Read-Ec([int]$addr) {
    $p = $wmi.GetMethodParameters('GetSetULong'); $p['Data'] = [UInt64]((1L -shl 40) + $addr)
    return [byte]([UInt64]$wmi.InvokeMethod('GetSetULong', $p, $null)['Return'] -band 0xFF)
}
function Write-Ec([int]$addr, [byte]$value) {
    $p = $wmi.GetMethodParameters('GetSetULong'); $p['Data'] = [UInt64](([UInt64]$value -shl 16) + $addr)
    $wmi.InvokeMethod('GetSetULong', $p, $null) | Out-Null
}

$levels = [byte[]](0, 8, 22, 36, 50)
Remove-Item $Log -ErrorAction SilentlyContinue
if (Get-Process GCUService -ErrorAction SilentlyContinue) { Write-Log "GCUService is running - stop Control Center first."; exit 1 }

[Kb]::Open($KeyboardPath)
$level = [Array]::IndexOf($levels, ([Kb]::Light())); if ($level -lt 0) { $level = 4 }
$before = Read-Ec $ApOemByte
Write-Log ("EC 0x741 before = 0x{0:X2}; keyboard level = {1}" -f $before, $level)

$watcher = $null
try {
    Write-Ec $ApOemByte ([byte]($before -bor 1))
    Write-Log ("EC 0x741 after set = 0x{0:X2}  (bit 0 = app present)" -f (Read-Ec $ApOemByte))

    $scope = New-Object System.Management.ManagementScope '\\.\root\WMI'
    $query = New-Object System.Management.WqlEventQuery 'SELECT * FROM AcpiTest_EventULong'
    $watcher = New-Object System.Management.ManagementEventWatcher -ArgumentList $scope, $query
    $watcher.Options.Timeout = [TimeSpan]::FromMilliseconds(250)
    $watcher.Start()
    Write-Log "LISTENING for $Seconds s - press keyboard light down/up, then try your other Fn keys."

    $end = (Get-Date).AddSeconds($Seconds)
    while ((Get-Date) -lt $end) {
        try { $e = $watcher.WaitForNextEvent() } catch [System.Management.ManagementException] { continue }
        $code = [int]$e['ULong']; $t0 = Get-Date
        $fwLight = [Kb]::Light()  # did the EC/firmware also change it on its own?
        if ($code -notin 177, 178, 185) { Write-Log "event $code (not a keyboard-light key)"; continue }
        switch ($code) {
            177 { if ($level -gt 0) { $level-- } }
            178 { if ($level -lt 4) { $level++ } }
            185 { $level = ($level + 1) % 5 }
        }
        [Kb]::Send(0x09, 2, $levels[$level])
        Write-Log ("event {0} -> level {1}; firmware light before our write = {2}; applied in {3:N0} ms" -f $code, $level, $fwLight, ((Get-Date) - $t0).TotalMilliseconds)
    }
}
finally {
    if ($watcher) { $watcher.Stop() }
    $now = Read-Ec $ApOemByte
    Write-Ec $ApOemByte ([byte]($now -band 0xFE))
    Write-Log ("RESTORED: EC 0x741 = 0x{0:X2} (bit 0 cleared; firmware handles the Fn keys again)" -f (Read-Ec $ApOemByte))
}
