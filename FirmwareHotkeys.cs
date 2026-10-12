using System.Management;

namespace MonsterKeyboardRGB;

/// <summary>
/// Takes over the laptop's Fn hotkeys the way Control Center's GCUService does (App.InitalizeStartPerparation):
/// setting the "AP exist" bit in the EC makes the firmware stop handling keyboard-light keys itself and report
/// every hotkey as an AcpiTest_EventULong WMI event instead, so software can react instantly.
/// EC access goes through the BIOS WMI method AcpiTest_MULong.GetSetULong, which requires administrator rights.
/// </summary>
internal sealed class FirmwareHotkeys : IDisposable
{
    // Define.ECSpec
    const ushort ApOemByte = 0x741;   // ADDR_AP_OEM_BYTE
    const byte ApExistBit = 0x01;     // MyEcCtrl.Set_APExistToEC
    const ushort TriggerByte = 0x767;       // TriggerByteFlag.WinLock_Trigger = bit 0 (MySettingManager.WinKeyLock_Trigger)
    const ushort StatusByte = 0x768;        // StatusByteOneFlag.WinLock = bit 0 (MySettingManager.SetWinKey)
    public const int KeyboardLightDown = 177;
    public const int KeyboardLightUp = 178;
    public const int KeyboardLightCycle = 185;
    public const int LightBarToggle = 175;  // HIDLightbar.SetBrinessByScanCode
    public const int WinKeyLocked = 64;    // OSD_WINKEY_LOCK
    public const int WinKeyUnlocked = 65;  // OSD_WINKEY_UNLOCK

    readonly ManagementObject ec;
    readonly ManagementEventWatcher watcher;
    readonly Lock ecLock = new();
    bool active;

    /// <summary>Raised on a thread-pool thread with the firmware's hotkey code.</summary>
    public event Action<int>? HotkeyPressed;

    /// <summary>
    /// The active instance, so crash handlers can give the hotkeys back. If the bit stayed set with nothing
    /// listening, the keyboard-light keys would do nothing until this app ran again.
    /// </summary>
    public static FirmwareHotkeys? Current { get; private set; }

    FirmwareHotkeys()
    {
        ec = new ManagementObject(@"root\WMI", @"AcpiTest_MULong.InstanceName='ACPI\PNP0C14\1_1'", null);
        watcher = new ManagementEventWatcher(new ManagementScope(@"\\.\root\WMI"), new WqlEventQuery("SELECT * FROM AcpiTest_EventULong"));
        watcher.EventArrived += (_, e) =>
        {
            if (active && e.NewEvent["ULong"] is { } code) HotkeyPressed?.Invoke(Convert.ToInt32(code));
        };
    }

    /// <summary>Takes over the hotkeys, or returns null with a reason if this machine or process can't.</summary>
    public static FirmwareHotkeys? TryStart(out string? failure)
    {
        FirmwareHotkeys? hotkeys = null;
        try
        {
            hotkeys = new FirmwareHotkeys();
            hotkeys.watcher.Start();
            hotkeys.active = true;
            hotkeys.EnsureApExist();
            Current = hotkeys;
            failure = null;
            return hotkeys;
        }
        catch (Exception e) when (e is ManagementException or UnauthorizedAccessException or System.Runtime.InteropServices.COMException)
        {
            failure = e.Message.Trim();
            hotkeys?.Dispose();
            return null;
        }
    }

    /// <summary>
    /// Sets the bit if something cleared it. GCUService re-sets it after resume from sleep, so call this
    /// periodically and on resume.
    /// </summary>
    public void EnsureApExist()
    {
        lock (ecLock)
        {
            if (!active) return;
            byte value = ReadEc(ApOemByte);
            if ((value & ApExistBit) == 0) WriteEc(ApOemByte, (byte)(value | ApExistBit));
        }
    }

    public bool IsWinKeyLocked()
    {
        lock (ecLock) return (ReadEc(StatusByte) & 0x01) != 0;
    }

    /// <summary>Asks the EC to flip the Windows key lock; it reports the result as a 64/65 hotkey event.</summary>
    public void ToggleWinKeyLock()
    {
        lock (ecLock)
        {
            if (!active) return;
            byte value = ReadEc(TriggerByte);
            WriteEc(TriggerByte, (byte)(value | 0x01));
        }
    }

    /// <summary>
    /// Stops handling hotkeys without touching the EC, for when Control Center's service takes over the bit.
    /// </summary>
    public void Abandon()
    {
        lock (ecLock) active = false;
    }

    /// <summary>Hands the hotkeys back to the firmware. Safe to call more than once.</summary>
    public void Release()
    {
        lock (ecLock)
        {
            if (!active) return;
            active = false;
            try
            {
                byte value = ReadEc(ApOemByte);
                WriteEc(ApOemByte, (byte)(value & ~ApExistBit));
            }
            catch (Exception) { } // best effort: we may be shutting down
        }
    }

    // WMIEC.WMIReadECRAM / WMIWriteECRAM: bit 40 selects a read; a write packs the value above the address.
    byte ReadEc(ushort address) => (byte)(Invoke((1UL << 40) + address) & 0xFF);

    void WriteEc(ushort address, byte value) => Invoke(((ulong)value << 16) + address);

    ulong Invoke(ulong data)
    {
        using var parameters = ec.GetMethodParameters("GetSetULong");
        parameters["Data"] = data;
        using var result = ec.InvokeMethod("GetSetULong", parameters, null);
        return Convert.ToUInt64(result["Return"]);
    }

    public void Dispose()
    {
        Release();
        if (Current == this) Current = null;
        try { watcher.Stop(); } catch (Exception) { }
        watcher.Dispose();
        ec.Dispose();
    }
}
