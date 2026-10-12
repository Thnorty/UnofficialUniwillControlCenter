using System.Runtime.InteropServices;

namespace MonsterKeyboardRGB;

/// <summary>
/// Caps Lock and Num Lock shown as flashes on the light bar: light opening from the middle when one turns on, closing
/// to the middle when it turns off. Polled, since nothing tells a background app about them.
/// </summary>
internal sealed class LockIndicators
{
    const int VK_CAPITAL = 0x14, VK_NUMLOCK = 0x90;
    static readonly Color CapsColor = Color.FromArgb(255, 255, 255);
    static readonly Color NumColor = Color.FromArgb(0, 140, 255);

    bool? caps, num;

    public bool CapsLock { get; set; }
    public bool NumLock { get; set; }

    /// <summary>The flashes for whatever changed since the last call.</summary>
    public List<LightBarOverlay> Poll()
    {
        var flashes = new List<LightBarOverlay>();
        // The states are tracked even when not shown, so turning an indicator on doesn't flash at once.
        Check(ref caps, IsToggled(VK_CAPITAL), CapsLock, on => new LightBarOverlay(CapsColor, Style(on)), flashes);
        Check(ref num, IsToggled(VK_NUMLOCK), NumLock, on => new LightBarOverlay(NumColor, Style(on)), flashes);
        return flashes;
    }

    static LightBarOverlay.Style Style(bool on) => on ? LightBarOverlay.Style.Open : LightBarOverlay.Style.Close;

    static void Check(ref bool? previous, bool? now, bool shown, Func<bool, LightBarOverlay> flash, List<LightBarOverlay> flashes)
    {
        if (shown && previous is bool before && now is bool after && before != after) flashes.Add(flash(after));
        previous = now;
    }

    static bool IsToggled(int key) => (GetKeyState(key) & 1) != 0;

    [DllImport("user32.dll")]
    static extern short GetKeyState(int key);
}
