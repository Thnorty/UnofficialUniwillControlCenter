# Shows (and optionally clears) the EC "app present" bit that decides who handles the keyboard-light Fn keys.
# Run from an elevated (admin) PowerShell:
#   powershell -ExecutionPolicy Bypass -File ec-ap-bit.ps1          # just show
#   powershell -ExecutionPolicy Bypass -File ec-ap-bit.ps1 -Clear   # hand the keys back to the firmware
param([switch]$Clear)
$ErrorActionPreference = 'Stop'
$wmi = New-Object System.Management.ManagementObject 'root\WMI', "AcpiTest_MULong.InstanceName='ACPI\PNP0C14\1_1'", $null
function Invoke-Ec([UInt64]$data) {
    $p = $wmi.GetMethodParameters('GetSetULong'); $p['Data'] = $data
    return [UInt64]$wmi.InvokeMethod('GetSetULong', $p, $null)['Return']
}
function Read-ApByte { [byte]((Invoke-Ec ((1L -shl 40) + 0x741)) -band 0xFF) }

$value = Read-ApByte
$owner = if ($value -band 1) { 'software (an app must be listening, or the keyboard-light keys do nothing)' } else { 'firmware' }
"EC 0x741 = 0x{0:X2} -> app-present bit = {1}: keyboard-light keys handled by {2}" -f $value, ($value -band 1), $owner
$procs = (Get-Process MonsterKeyboardRGB, GCUService -ErrorAction SilentlyContinue | ForEach-Object Name | Sort-Object -Unique) -join ', '
"Running: " + $(if ($procs) { $procs } else { 'neither Keyboard RGB nor Control Center' })

if ($Clear) {
    Invoke-Ec ([UInt64](([UInt64]($value -band 0xFE) -shl 16) + 0x741)) | Out-Null
    "Cleared. EC 0x741 = 0x{0:X2}" -f (Read-ApByte)
}
