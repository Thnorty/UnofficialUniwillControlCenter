using System.Runtime.InteropServices;

namespace MonsterKeyboardRGB;

/// <summary>
/// Reports where on the keyboard each key press is, from 0 (left edge) to 1 (right edge, the numpad), for the
/// light bar's Typing effect. Only key positions are looked at, nothing is recorded, and the hook is only installed
/// while an effect needs it. It must be created on a thread with a message loop (the UI thread).
/// </summary>
internal sealed class KeyboardHook : IDisposable
{
    const int WH_KEYBOARD_LL = 13;
    const int WM_KEYDOWN = 0x100, WM_KEYUP = 0x101, WM_SYSKEYDOWN = 0x104, WM_SYSKEYUP = 0x105;
    const uint LLKHF_EXTENDED = 0x01;
    const int Extended = 0x100;

    // The Tulpar's layout in key widths: the main block is 15 wide (arrows and navigation keys at its right edge),
    // the numpad four more.
    const float Width = 19;
    static readonly Dictionary<int, float> Positions = BuildPositions();

    readonly Action<float> pressed;
    readonly HookProc callback; // kept alive while the hook is installed
    readonly HashSet<int> down = [];
    IntPtr hook;

    KeyboardHook(Action<float> pressed)
    {
        this.pressed = pressed;
        callback = OnKey;
        hook = SetWindowsHookEx(WH_KEYBOARD_LL, callback, GetModuleHandle(null), 0);
    }

    /// <summary>Installs the hook, or returns null if Windows refused it.</summary>
    public static KeyboardHook? Install(Action<float> pressed)
    {
        var hook = new KeyboardHook(pressed);
        if (hook.hook != IntPtr.Zero) return hook;
        return null;
    }

    IntPtr OnKey(int code, IntPtr message, IntPtr data)
    {
        if (code >= 0)
        {
            try
            {
                var key = Marshal.PtrToStructure<KbdLlHookStruct>(data);
                int id = (int)(key.ScanCode & 0xFF) | ((key.Flags & LLKHF_EXTENDED) != 0 ? Extended : 0);
                switch ((int)message)
                {
                    case WM_KEYDOWN or WM_SYSKEYDOWN:
                        if (down.Add(id)) pressed(Positions.TryGetValue(id, out float x) ? x / Width : 0.5f); // not auto-repeat
                        break;
                    case WM_KEYUP or WM_SYSKEYUP:
                        down.Remove(id);
                        break;
                }
            }
            catch (Exception)
            {
                // never let an effect get in the way of typing
            }
        }
        return CallNextHookEx(hook, code, message, data);
    }

    static Dictionary<int, float> BuildPositions()
    {
        var map = new Dictionary<int, float>();
        void Row(int firstScanCode, int count, float firstX, float step = 1)
        {
            for (int i = 0; i < count; i++) map[firstScanCode + i] = firstX + i * step;
        }

        // Function row: Esc, F1-F10, F11, F12.
        map[0x01] = 0.5f;
        Row(0x3B, 10, 1.6f, 1.05f);
        map[0x57] = 12.1f;
        map[0x58] = 13.15f;
        map[0x37 | Extended] = 14.2f; // Print Screen
        // Number row: `, 1-0, -, =, Backspace.
        map[0x29] = 0.5f;
        Row(0x02, 12, 1.5f);
        map[0x0E] = 14;
        // Q row: Tab, Q-], \.
        map[0x0F] = 0.75f;
        Row(0x10, 12, 2);
        map[0x2B] = 14.25f;
        // A row: Caps Lock, A-', Enter.
        map[0x3A] = 0.9f;
        Row(0x1E, 11, 2.25f);
        map[0x1C] = 14;
        // Z row: Shift, Z-/, Shift.
        map[0x2A] = 1.1f;
        Row(0x2C, 10, 2.75f);
        map[0x36] = 13.25f;
        // Bottom row: Ctrl, Fn (not seen by Windows), Win, Alt, Space, Alt, Menu, Ctrl.
        map[0x1D] = 0.6f;
        map[0x5B | Extended] = 2.4f;
        map[0x38] = 3.5f;
        map[0x39] = 7.25f;
        map[0x38 | Extended] = 10.5f;
        map[0x5D | Extended] = 11.25f;
        map[0x1D | Extended] = 11.75f;
        // Arrows and the navigation keys, at the right edge of the main block.
        map[0x4B | Extended] = 12.75f;
        map[0x48 | Extended] = 13.75f;
        map[0x50 | Extended] = 13.75f;
        map[0x4D | Extended] = 14.75f;
        foreach (int nav in new[] { 0x52, 0x53, 0x47, 0x4F, 0x49, 0x51 }) map[nav | Extended] = 14.5f;
        // Numpad.
        map[0x45] = 15.5f; // Num Lock
        map[0x35 | Extended] = 16.5f;
        map[0x37] = 17.5f;
        map[0x4A] = 18.5f;
        Row(0x47, 3, 15.5f); // 7 8 9
        map[0x4E] = 18.5f;
        Row(0x4B, 3, 15.5f); // 4 5 6
        Row(0x4F, 3, 15.5f); // 1 2 3
        map[0x1C | Extended] = 18.5f;
        map[0x52] = 16;
        map[0x53] = 17.5f;
        return map;
    }

    public void Dispose()
    {
        if (hook == IntPtr.Zero) return;
        UnhookWindowsHookEx(hook);
        hook = IntPtr.Zero;
    }

    delegate IntPtr HookProc(int code, IntPtr message, IntPtr data);

    [StructLayout(LayoutKind.Sequential)]
    struct KbdLlHookStruct
    {
        public uint VkCode;
        public uint ScanCode;
        public uint Flags;
        public uint Time;
        public IntPtr ExtraInfo;
    }

    [DllImport("user32.dll", SetLastError = true)]
    static extern IntPtr SetWindowsHookEx(int hookType, HookProc callback, IntPtr module, uint threadId);

    [DllImport("user32.dll")]
    static extern bool UnhookWindowsHookEx(IntPtr hook);

    [DllImport("user32.dll")]
    static extern IntPtr CallNextHookEx(IntPtr hook, int code, IntPtr message, IntPtr data);

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode)]
    static extern IntPtr GetModuleHandle(string? name);
}
