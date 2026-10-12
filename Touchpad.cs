using System.Runtime.InteropServices;
using Microsoft.Win32;

namespace MonsterKeyboardRGB;

/// <summary>
/// Turns the Precision Touchpad on and off the way Control Center does (GCService5.DeviceSwitchItem.TriggerTouchpad):
/// by pressing Ctrl+Win+F24, which Windows handles as its built-in touchpad toggle.
/// </summary>
internal static class Touchpad
{
    const string StatusKey = @"Software\Microsoft\Windows\CurrentVersion\PrecisionTouchPad\Status";

    /// <summary>Windows' current touchpad state, or null if it doesn't report one.</summary>
    public static bool? IsEnabled()
    {
        using var key = Registry.CurrentUser.OpenSubKey(StatusKey);
        return key?.GetValue("Enabled") is int value ? value != 0 : null;
    }

    public static void Toggle()
    {
        const ushort VK_CONTROL = 0x11, VK_LWIN = 0x5B, VK_F24 = 0x87;
        Input[] inputs =
        [
            Key(VK_CONTROL, up: false), Key(VK_LWIN, up: false), Key(VK_F24, up: false),
            Key(VK_F24, up: true), Key(VK_LWIN, up: true), Key(VK_CONTROL, up: true),
        ];
        if (SendInput((uint)inputs.Length, inputs, Marshal.SizeOf<Input>()) != inputs.Length)
            throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error(), "Couldn't send the touchpad shortcut");
    }

    static Input Key(ushort vk, bool up) => new()
    {
        Type = 1, // INPUT_KEYBOARD
        Keyboard = new KeyboardInput { VirtualKey = vk, Flags = up ? 0x0002u : 0u }, // KEYEVENTF_KEYUP
    };

    [StructLayout(LayoutKind.Sequential)]
    struct KeyboardInput
    {
        public ushort VirtualKey;
        public ushort ScanCode;
        public uint Flags;
        public uint Time;
        public IntPtr ExtraInfo;
    }

    // INPUT is a tagged union; the mouse member is the largest, so pad the keyboard member up to its size.
    [StructLayout(LayoutKind.Explicit)]
    struct Input
    {
        [FieldOffset(0)] public uint Type;
        [FieldOffset(8)] public KeyboardInput Keyboard;
        [FieldOffset(8)] MouseInputPadding padding;
    }

    [StructLayout(LayoutKind.Sequential)]
    struct MouseInputPadding
    {
        public int Dx, Dy;
        public uint MouseData, Flags, Time;
        public IntPtr ExtraInfo;
    }

    [DllImport("user32.dll", SetLastError = true)]
    static extern uint SendInput(uint count, Input[] inputs, int size);
}
