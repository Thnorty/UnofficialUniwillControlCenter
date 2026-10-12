# Diagnoses why the keyboard light goes dark when software hands the Fn keys back to the EC.
# Run from an elevated (admin) Windows PowerShell, with Keyboard RGB and Control Center NOT running.
# It ends with the bit cleared (the firmware's normal state) and the light at level 4.
param([string]$Log = "$env:USERPROFILE\Desktop\MonsterKeyboardRGB\tools\exit-dark-diagnostic.log")
$ErrorActionPreference = 'Stop'
$KeyboardPath = '\\?\HID#VID_048D&PID_CE00&MI_01#7&4ced108&0&0000#{4d1e55b2-f16f-11cf-88cb-001111000030}'

function Write-Log([string]$text) {
    $line = "{0:HH:mm:ss.fff}  {1}" -f (Get-Date), $text
    Write-Host $line; Add-Content -Path $Log -Value $line
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
    public static string State() { Send(0x88); var r = new byte[9]; HidD_GetFeature(handle, r, 9); return string.Format("control={0} effect={1} speed={2} light={3} colorIdx={4} dir={5}", r[2], r[3], r[4], r[5], r[6], r[7]); }
}
"@
$wmi = New-Object System.Management.ManagementObject 'root\WMI', "AcpiTest_MULong.InstanceName='ACPI\PNP0C14\1_1'", $null
function Invoke-Ec([UInt64]$data) { $p = $wmi.GetMethodParameters('GetSetULong'); $p['Data'] = $data; [UInt64]$wmi.InvokeMethod('GetSetULong', $p, $null)['Return'] }
function Read-Ec([int]$a) { [byte]((Invoke-Ec ((1L -shl 40) + $a)) -band 0xFF) }
function Write-Ec([int]$a, [byte]$v) { Invoke-Ec ([UInt64](([UInt64]$v -shl 16) + $a)) | Out-Null }
function Dump-Ec { $d = @{}; foreach ($a in 0x700..0x7FF) { $d[$a] = Read-Ec $a }; $d }

Remove-Item $Log -ErrorAction SilentlyContinue
if (Get-Process MonsterKeyboardRGB, GCUService -ErrorAction SilentlyContinue) { Write-Log "Close Keyboard RGB and stop Control Center first."; exit 1 }
[Kb]::Open($KeyboardPath)

try {
    [Kb]::Send(0x09, 2, 50)
    Write-Log "keyboard set to level 4: $([Kb]::State())"
    Write-Log ("before: 0x741=0x{0:X2} 0x78C=0x{1:X2}" -f (Read-Ec 0x741), (Read-Ec 0x78C))

    Write-Ec 0x741 ((Read-Ec 0x741) -bor 1)
    Write-Ec 0x78C (((Read-Ec 0x78C) -band 0x1F) -bor (4 -shl 5))   # what the app does: level 4 in bits 7..5
    Start-Sleep -Milliseconds 300
    Write-Log ("app present + level synced: 0x741=0x{0:X2} 0x78C=0x{1:X2}  keyboard: {2}" -f (Read-Ec 0x741), (Read-Ec 0x78C), [Kb]::State())

    Write-Log "dumping EC 0x700-0x7FF (bit set)..."
    $before = Dump-Ec

    Write-Log "CLEARING the app-present bit now; watching the keyboard for 2 s"
    Write-Ec 0x741 ((Read-Ec 0x741) -band 0xFE)
    $t0 = Get-Date; $last = ''
    while (((Get-Date) - $t0).TotalMilliseconds -lt 2000) {
        $s = [Kb]::State()
        if ($s -ne $last) { Write-Log ("+{0,5:N0} ms  {1}" -f ((Get-Date) - $t0).TotalMilliseconds, $s); $last = $s }
        Start-Sleep -Milliseconds 20
    }

    Write-Log "dumping EC 0x700-0x7FF (bit cleared)..."
    $after = Dump-Ec
    foreach ($a in 0x700..0x7FF) {
        if ($a -ne 0x741 -and $before[$a] -ne $after[$a]) { Write-Log ("EC 0x{0:X3}: 0x{1:X2} -> 0x{2:X2}" -f $a, $before[$a], $after[$a]) }
    }
    Write-Log "(registers not listed did not change)"
}
finally {
    if (((Read-Ec 0x741) -band 1) -ne 0) { Write-Ec 0x741 ((Read-Ec 0x741) -band 0xFE) }
    [Kb]::Send(0x09, 2, 50)
    Write-Log ("done: 0x741=0x{0:X2}, keyboard back to level 4: {1}" -f (Read-Ec 0x741), [Kb]::State())
}
