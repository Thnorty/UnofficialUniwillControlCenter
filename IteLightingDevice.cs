using System.ComponentModel;
using System.Runtime.InteropServices;
using Microsoft.Win32.SafeHandles;

namespace MonsterKeyboardRGB;

internal enum LightingDeviceKind { Keyboard, LightBar }

/// <summary>
/// Drives an ITE lighting controller directly over HID feature reports, using the same commands Control Center's
/// GCUService sends (recovered from its decompiled LightingModel.LM_ITE_RGB): the four-zone keyboard (ITE 8291) or
/// the light bar on the front edge (LM_ITE_RGB type MEZone_Lighbar). Every command is a 9-byte feature report:
/// report ID 0, then 8 bytes.
/// </summary>
internal sealed class IteLightingDevice : IDisposable
{
    const ushort IteVendorId = 0x048D;
    // ITE_SPEC.PIDList and LBPIDList.
    static readonly ushort[] KeyboardProductIds = [0xCE00, 0x6000, 0x6001, 0x6002, 0x6003, 0x6004, 0x6006, 0x6007, 0x600A];
    static readonly ushort[] LightBarProductIds = [0x6005, 0x6008, 0x6010];
    const ushort FourZoneUsagePage = 0xFF12; // ITE_SPEC.USAGE_PAGE_4Zone
    const ushort LightBarUsagePage = 0xFF03; // ITE_SPEC.USAGE_PAGE_Ligbar
    const ushort ControlUsage = 0x0001;

    const byte CmdSetEffect = 0x08;
    const byte CmdSetBrightness = 0x09;
    const byte CmdSetColor = 0x14;
    const byte CmdGetFirmwareVersion = 0x80;
    const byte CmdGetEffect = 0x88;

    const byte ControlOff = 1;
    const byte ControlLighting = 2;
    const byte ControlReset = 5;
    const byte ColorIndexSlot1 = 1;   // effect uses the single color in slot 1
    const byte ColorIndexPalette = 8; // effect reads the circular palette loaded into slots 1..n
    const int PaletteSlots = 7;

    /// <summary>Highest firmware brightness. Control Center's four levels are 8, 22, 36 and 50.</summary>
    public const byte MaxLight = 50;

    /// <summary>LM_ITE_RGB.Translate_ITE_LightValue: Control Center's brightness levels 0-4 as firmware values.</summary>
    public static readonly byte[] StandardLights = [0, 8, 22, 36, 50];

    // LM_ITE_RGB.Translate_ITE_SpeedValue (slider 0-4 -> firmware value).
    static readonly byte[] SpeedValues = [10, 7, 5, 3, 1];

    // HIDLightbar.RunEffct: the light bar's Rainbow is a seven-color circular palette.
    static readonly Color[] LightBarRainbow =
    [
        Color.FromArgb(255, 0, 0), Color.FromArgb(255, 165, 0), Color.FromArgb(255, 255, 0), Color.FromArgb(0, 255, 0),
        Color.FromArgb(0, 255, 255), Color.FromArgb(0, 0, 255), Color.FromArgb(139, 0, 255),
    ];

    readonly SafeFileHandle handle;
    readonly int featureLength;
    // Commands come from the UI thread, the brightness watcher and the music loop; a query must not interleave
    // with a command sequence.
    readonly Lock io = new();
    int lastKnownLight = -1;
    bool isOff; // switched off with control 1, which 09h can't undo

    IteLightingDevice(LightingDeviceKind kind, SafeFileHandle handle, int featureLength)
    {
        Kind = kind;
        this.handle = handle;
        this.featureLength = featureLength;
    }

    public LightingDeviceKind Kind { get; }

    public (byte Major, byte Minor) FirmwareVersion { get; private set; }

    /// <summary>Finds and opens the controller, or returns null if this laptop doesn't have one.</summary>
    public static IteLightingDevice? Open(LightingDeviceKind kind)
    {
        var (productIds, usagePage) = kind == LightingDeviceKind.Keyboard
            ? (KeyboardProductIds, FourZoneUsagePage)
            : (LightBarProductIds, LightBarUsagePage);
        foreach (var path in Hid.EnumerateDevicePaths())
        {
            var h = Hid.OpenDevice(path);
            if (h.IsInvalid) continue;
            var attributes = new Hid.Attributes { Size = Marshal.SizeOf<Hid.Attributes>() };
            if (Hid.HidD_GetAttributes(h, ref attributes)
                && attributes.VendorId == IteVendorId
                && productIds.Contains(attributes.ProductId)
                && Hid.TryGetCaps(h, out var caps)
                && caps.UsagePage == usagePage && caps.Usage == ControlUsage)
            {
                var device = new IteLightingDevice(kind, h, caps.FeatureReportByteLength);
                try { device.ReadFirmwareVersion(); }
                catch { device.Dispose(); throw; }
                return device;
            }
            h.Dispose();
        }
        return null;
    }

    void ReadFirmwareVersion()
    {
        var reply = Query(CmdGetFirmwareVersion);
        FirmwareVersion = (reply[2], reply[3]);
    }

    // FourZoneKeyboard.SetBrightnessStrategy: keyboard firmware >= 0x11.03 takes brightness through command 09h.
    // The light bar's driver (HIDKeyboard.SetBrightnessStrategy) always uses it.
    bool HasBrightnessCommand => Kind == LightingDeviceKind.LightBar
        || (FirmwareVersion.Major >= 0x11 && FirmwareVersion.Minor >= 0x03);

    /// <summary>
    /// Shows an effect. <paramref name="light"/> is the firmware brightness, 0-<see cref="MaxLight"/> (0 keeps the
    /// effect running but dark, as Control Center's brightness 0 does); speed is 0-4 like Control Center's slider.
    /// </summary>
    public void SetEffect(LightingEffect effect, IReadOnlyList<Color> colors, byte direction, byte light, int speed, bool saveToFirmware)
    {
        lock (io)
        {
            light = Math.Min(light, MaxLight);
            SetEffectLocked(effect, colors, direction, light, speed, saveToFirmware);
            lastKnownLight = light;
        }
    }

    /// <summary>Switches the lights off (ILM_RGBKB_SetPower Off). Any later effect switches them back on.</summary>
    public void SwitchOff()
    {
        lock (io)
        {
            Send(CmdSetEffect, ControlReset);
            Send(CmdSetEffect, ControlOff);
            isOff = true;
            lastKnownLight = 0;
        }
    }

    /// <summary>The controller's state: whether it's switched on, and its effect ID.</summary>
    public (bool On, byte Effect) ReadState()
    {
        lock (io)
        {
            var reply = Query(CmdGetEffect);
            return (reply[2] != ControlOff, reply[3]);
        }
    }

    /// <summary>
    /// Changes only the brightness (command 09h, as GCUService does for Fn keys). Returns false when that isn't
    /// possible because the firmware lacks 09h or the lights were switched off, so the caller must resend the effect.
    /// </summary>
    public bool TrySetLight(byte light)
    {
        lock (io)
        {
            if (!HasBrightnessCommand || isOff) return false;
            light = Math.Min(light, MaxLight);
            Send(CmdSetBrightness, ControlLighting, light);
            lastKnownLight = light;
            return true;
        }
    }

    /// <summary>
    /// Sets the firmware brightness without changing the value this app tracks, for software-driven changes
    /// such as Music and fades. Does nothing while the lights are switched off.
    /// </summary>
    public void SetRawLight(byte light)
    {
        lock (io)
        {
            if (HasBrightnessCommand && !isOff) Send(CmdSetBrightness, ControlLighting, Math.Min(light, MaxLight));
        }
    }

    /// <summary>The firmware brightness (0-<see cref="MaxLight"/>) the controller reports; 0 if it's switched off.</summary>
    public byte ReadLight()
    {
        lock (io)
        {
            var reply = Query(CmdGetEffect); // control, effect, speed, light, color index, direction
            isOff = reply[2] == ControlOff;
            byte light = isOff ? (byte)0 : Math.Min(reply[5], MaxLight);
            lastKnownLight = light;
            return light;
        }
    }

    /// <summary>
    /// Returns the new firmware brightness if something other than this app changed it (the Fn keys are handled
    /// by the laptop's firmware when this app doesn't own them), otherwise null.
    /// </summary>
    public byte? PollExternalLightChange()
    {
        lock (io)
        {
            int previous = lastKnownLight;
            byte light = ReadLight();
            return previous >= 0 && light != previous ? light : null;
        }
    }

    void SetEffectLocked(LightingEffect effect, IReadOnlyList<Color> colors, byte direction, byte light, int speed, bool saveToFirmware)
    {
        isOff = false;
        Send(CmdSetEffect, ControlReset);
        if (effect.Id == LightingEffect.RainbowId)
        {
            colors = Kind == LightingDeviceKind.LightBar ? LightBarRainbow : [];
            if (Kind == LightingDeviceKind.Keyboard)
            {
                // Set_Lighting_Effect loads these fixed slots for the four-zone Rainbow, then starts it with no palette.
                SetColor(1, Color.FromArgb(255, 0, 0));
                SetColor(2, Color.FromArgb(0, 255, 0));
                SetColor(3, Color.FromArgb(0, 0, 255));
                SetColor(4, Color.FromArgb(255, 0, 255));
            }
        }
        byte colorIndex;
        if (colors.Count == 0)
        {
            colorIndex = 0;
        }
        else if (colors.Count == 1 && Kind == LightingDeviceKind.Keyboard)
        {
            // Set_ITE_Effect_Type_FwMode with a non-circular color: one color in slot 1.
            SetColor(1, colors[0]);
            colorIndex = ColorIndexSlot1;
        }
        else
        {
            // A circular palette. Effects that cycle colors read all the slots, so a shorter palette is repeated
            // (the light bar's driver always sends circular palettes, even for one color).
            int slots = effect.Colors == EffectColors.Palette ? Math.Max(PaletteSlots, colors.Count) : colors.Count;
            for (int i = 0; i < slots; i++) SetColor((byte)(i + 1), colors[i % colors.Count]);
            colorIndex = ColorIndexPalette;
        }
        Send(CmdSetEffect, ControlLighting, effect.Id, SpeedValues[Math.Clamp(speed, 0, 4)],
            light, colorIndex, effect.HasDirection ? direction : (byte)0, (byte)(saveToFirmware ? 1 : 0));
        if (HasBrightnessCommand) Send(CmdSetBrightness, ControlLighting, light);
    }

    void SetColor(byte slot, Color color)
    {
        var (r, g, b) = Kind == LightingDeviceKind.Keyboard
            ? KeyboardWhiteBalance(color.R, color.G, color.B)
            : LightBarWhiteBalance(color.R, color.G, color.B);
        Send(CmdSetColor, 0, slot, r, g, b);
    }

    /// <summary>
    /// WKDColor.cheatRGB_4Zone: the LEDs' green and blue are much stronger than red, so the service scales
    /// them down. This is the branch for every project except IDs 6 and 7 (this laptop reports 15).
    /// </summary>
    static (byte R, byte G, byte B) KeyboardWhiteBalance(byte r, byte g, byte b) => (r, g, b) switch
    {
        (255, 100, 0) => (255, 40, 0),
        (255, 165, 0) => (255, 70, 0),
        _ => (r, (byte)(100 * g / 255), (byte)(200 * b / 255)),
    };

    /// <summary>WKDColor.cheatRGB_HIDLightbar: the light bar's own color correction.</summary>
    static (byte R, byte G, byte B) LightBarWhiteBalance(byte r, byte g, byte b) => (r, g, b) switch
    {
        (255, 255, 255) => (220, 120, 110),
        (255, 165, 0) => (220, 30, 0),
        (255, 255, 0) => (220, 120, 0),
        (0, 255, 255) => (0, 60, 110),
        (139, 0, 255) => (220, 0, 110),
        _ => ((byte)(220 * r / 255), (byte)(120 * g / 255), (byte)(110 * b / 255)),
    };

    void Send(params byte[] command)
    {
        var report = new byte[Math.Max(featureLength, 9)];
        command.CopyTo(report, 1); // report[0] is the report ID (0)
        if (!Hid.HidD_SetFeature(handle, report, report.Length))
            throw new Win32Exception(Marshal.GetLastWin32Error(), "Keyboard did not accept the command");
        Thread.Sleep(1); // the service waits 1 ms between reports too
    }

    byte[] Query(byte command)
    {
        Send(command);
        var reply = new byte[Math.Max(featureLength, 9)];
        if (!Hid.HidD_GetFeature(handle, reply, reply.Length))
            throw new Win32Exception(Marshal.GetLastWin32Error(), "Keyboard did not answer");
        return reply;
    }

    public void Dispose() => handle.Dispose();

    static class Hid
    {
        [StructLayout(LayoutKind.Sequential)]
        public struct Attributes
        {
            public int Size;
            public ushort VendorId;
            public ushort ProductId;
            public ushort VersionNumber;
        }

        [StructLayout(LayoutKind.Sequential)]
        public struct Caps
        {
            public ushort Usage;
            public ushort UsagePage;
            public ushort InputReportByteLength;
            public ushort OutputReportByteLength;
            public ushort FeatureReportByteLength;
            [MarshalAs(UnmanagedType.ByValArray, SizeConst = 17)] public ushort[] Reserved;
            public ushort NumberLinkCollectionNodes, NumberInputButtonCaps, NumberInputValueCaps, NumberInputDataIndices,
                NumberOutputButtonCaps, NumberOutputValueCaps, NumberOutputDataIndices,
                NumberFeatureButtonCaps, NumberFeatureValueCaps, NumberFeatureDataIndices;
        }

        [StructLayout(LayoutKind.Sequential)]
        struct DeviceInterfaceData
        {
            public int Size;
            public Guid InterfaceClassGuid;
            public int Flags;
            public IntPtr Reserved;
        }

        const int DIGCF_PRESENT = 0x02, DIGCF_DEVICEINTERFACE = 0x10;
        const int HIDP_STATUS_SUCCESS = 0x00110000;

        [DllImport("hid.dll")] static extern void HidD_GetHidGuid(out Guid guid);
        [DllImport("hid.dll", SetLastError = true)] public static extern bool HidD_GetAttributes(SafeFileHandle h, ref Attributes attributes);
        [DllImport("hid.dll", SetLastError = true)] static extern bool HidD_GetPreparsedData(SafeFileHandle h, out IntPtr data);
        [DllImport("hid.dll")] static extern bool HidD_FreePreparsedData(IntPtr data);
        [DllImport("hid.dll")] static extern int HidP_GetCaps(IntPtr data, out Caps caps);
        [DllImport("hid.dll", SetLastError = true)] public static extern bool HidD_SetFeature(SafeFileHandle h, byte[] buffer, int length);
        [DllImport("hid.dll", SetLastError = true)] public static extern bool HidD_GetFeature(SafeFileHandle h, byte[] buffer, int length);

        [DllImport("setupapi.dll", SetLastError = true)]
        static extern IntPtr SetupDiGetClassDevsW(ref Guid classGuid, IntPtr enumerator, IntPtr parent, int flags);
        [DllImport("setupapi.dll", SetLastError = true)]
        static extern bool SetupDiEnumDeviceInterfaces(IntPtr set, IntPtr devInfo, ref Guid guid, int index, ref DeviceInterfaceData data);
        [DllImport("setupapi.dll", SetLastError = true, CharSet = CharSet.Unicode)]
        static extern bool SetupDiGetDeviceInterfaceDetailW(IntPtr set, ref DeviceInterfaceData data, IntPtr detail, int detailSize, out int requiredSize, IntPtr devInfo);
        [DllImport("setupapi.dll")] static extern bool SetupDiDestroyDeviceInfoList(IntPtr set);

        [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
        static extern SafeFileHandle CreateFileW(string path, uint access, uint share, IntPtr security, uint creation, uint flags, IntPtr template);

        public static IEnumerable<string> EnumerateDevicePaths()
        {
            HidD_GetHidGuid(out var guid);
            var set = SetupDiGetClassDevsW(ref guid, IntPtr.Zero, IntPtr.Zero, DIGCF_PRESENT | DIGCF_DEVICEINTERFACE);
            if (set == new IntPtr(-1)) yield break;
            try
            {
                var data = new DeviceInterfaceData { Size = Marshal.SizeOf<DeviceInterfaceData>() };
                for (int i = 0; SetupDiEnumDeviceInterfaces(set, IntPtr.Zero, ref guid, i, ref data); i++)
                {
                    SetupDiGetDeviceInterfaceDetailW(set, ref data, IntPtr.Zero, 0, out int size, IntPtr.Zero);
                    var detail = Marshal.AllocHGlobal(size);
                    try
                    {
                        // cbSize of SP_DEVICE_INTERFACE_DETAIL_DATA_W: 8 on x64, 6 on x86.
                        Marshal.WriteInt32(detail, IntPtr.Size == 8 ? 8 : 6);
                        if (SetupDiGetDeviceInterfaceDetailW(set, ref data, detail, size, out _, IntPtr.Zero))
                            yield return Marshal.PtrToStringUni(detail + 4)!;
                    }
                    finally
                    {
                        Marshal.FreeHGlobal(detail);
                    }
                }
            }
            finally
            {
                SetupDiDestroyDeviceInfoList(set);
            }
        }

        public static SafeFileHandle OpenDevice(string path)
        {
            const uint GenericReadWrite = 0xC0000000, ShareReadWrite = 3, OpenExisting = 3;
            return CreateFileW(path, GenericReadWrite, ShareReadWrite, IntPtr.Zero, OpenExisting, 0, IntPtr.Zero);
        }

        public static bool TryGetCaps(SafeFileHandle h, out Caps caps)
        {
            caps = default;
            if (!HidD_GetPreparsedData(h, out var data)) return false;
            try { return HidP_GetCaps(data, out caps) == HIDP_STATUS_SUCCESS; }
            finally { HidD_FreePreparsedData(data); }
        }
    }
}

internal enum EffectColors { None, Zones, Palette }

/// <summary>
/// A lighting effect from one of Control Center's lighting pages, with the firmware effect ID it sends
/// (LM_ITE_RGB.Translate_ITE_EffectIndex, and HIDLightbar.RunEffct for the light bar).
/// </summary>
internal sealed record LightingEffect(string Key, string DisplayName, byte Id, EffectColors Colors, bool HasSpeed,
    bool HasDirection = false, int MaxColors = 7, string? Hint = null)
{
    public const byte StaticId = 1;
    public const byte RainbowId = 5;
    public const byte MusicId = 34;

    // Control Center's default 7-color palette for animated effects (from its RGBKeyboard.reg defaults).
    public static readonly Color[] DefaultPalette =
    [
        Color.FromArgb(255, 0, 0), Color.FromArgb(255, 165, 0), Color.FromArgb(255, 255, 0), Color.FromArgb(0, 255, 0),
        Color.FromArgb(0, 0, 255), Color.FromArgb(0, 255, 255), Color.FromArgb(139, 0, 255),
    ];

    /// <summary>Music is driven by this app: the controller shows the static zones and we vary the brightness.</summary>
    public bool IsMusic => Id == MusicId;

    /// <summary>The colors a palette effect starts with.</summary>
    public IReadOnlyList<Color> DefaultColors { get; init; } = DefaultPalette;

    public static readonly LightingEffect[] Keyboard =
    [
        new("static", "Static (4 zones)", StaticId, EffectColors.Zones, HasSpeed: false),
        new("breathing", "Breathing", 2, EffectColors.Palette, HasSpeed: true, Hint: "Fades in and out, one color at a time."),
        new("wave", "Wave", 3, EffectColors.Palette, HasSpeed: true, HasDirection: true, Hint: "The colors flow across the zones."),
        new("rainbow", "Rainbow", RainbowId, EffectColors.None, HasSpeed: true),
        new("flash", "Flash", 18, EffectColors.Palette, HasSpeed: true, Hint: "Flashes on and off, one color at a time."),
        new("mix", "Mix", 19, EffectColors.Palette, HasSpeed: true, Hint: "Mixes the colors across the zones."),
        new("music", "Music", MusicId, EffectColors.Zones, HasSpeed: true,
            Hint: "Pulses with the sound this PC plays. Brightness sets the peak; speed sets how fast it falls back."),
    ];

    // The firmware effects HIDLightbar.RunEffct handles for the light bar (Wave is its "Colorwave", 32).
    public static readonly LightingEffect[] LightBar =
    [
        new("static", "Static color", StaticId, EffectColors.Palette, HasSpeed: false, MaxColors: 1)
        {
            DefaultColors = [Color.FromArgb(0, 255, 255)],
        },
        new("breathing", "Breathing", 2, EffectColors.Palette, HasSpeed: true, Hint: "Fades in and out, one color at a time."),
        new("wave", "Wave", 32, EffectColors.Palette, HasSpeed: true, HasDirection: true, Hint: "The colors flow along the bar."),
        new("rainbow", "Rainbow", RainbowId, EffectColors.None, HasSpeed: true),
        new("flash", "Flash", 18, EffectColors.Palette, HasSpeed: true, Hint: "Flashes on and off, one color at a time."),
        new("mix", "Mix", 19, EffectColors.Palette, HasSpeed: true),
        new("thinking", "Thinking", 33, EffectColors.Palette, HasSpeed: true, MaxColors: 9)
        {
            // The palette Control Center saved for it (HKLM\SOFTWARE\OEM\GamingCenter2\RGBKeyboard\MEZone_Lighbar).
            DefaultColors =
            [
                Color.FromArgb(255, 59, 0), Color.FromArgb(255, 100, 0), Color.FromArgb(255, 158, 0), Color.FromArgb(255, 190, 0),
                Color.FromArgb(255, 155, 0), Color.FromArgb(255, 118, 0), Color.FromArgb(255, 77, 0), Color.FromArgb(0, 0, 255),
                Color.FromArgb(136, 0, 255),
            ],
        },
    ];

    public override string ToString() => DisplayName;
}
