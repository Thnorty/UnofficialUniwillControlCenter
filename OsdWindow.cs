using System.Drawing.Drawing2D;
using System.Runtime.InteropServices;
using Microsoft.Win32;

namespace MonsterKeyboardRGB;

/// <summary>
/// A small flyout near the bottom of the screen, like Windows' volume/brightness OSD.
/// It never takes focus and clicks pass through it.
/// </summary>
internal sealed class OsdWindow : Form
{
    const string KeyboardGlyph = ""; // KeyboardClassic
    const string TouchpadGlyph = ""; // Touchpad
    const string LockGlyph = "";     // Lock
    const string UnlockGlyph = "";   // Unlock
    const string LightGlyph = "";  // Lightbulb

    static readonly string IconFont = FontFamily.Families.Any(f => f.Name == "Segoe Fluent Icons") ? "Segoe Fluent Icons" : "Segoe MDL2 Assets";

    readonly System.Windows.Forms.Timer hideTimer = new() { Interval = 1400 };
    readonly System.Windows.Forms.Timer fadeTimer = new() { Interval = 15 };
    string glyph = KeyboardGlyph;
    string caption = "";
    float level = -1; // brightness bar fill, 0-1, or -1 for none
    bool dark;

    public OsdWindow()
    {
        FormBorderStyle = FormBorderStyle.None;
        ShowInTaskbar = false;
        StartPosition = FormStartPosition.Manual;
        TopMost = true;
        DoubleBuffered = true;
        hideTimer.Tick += (_, _) => { hideTimer.Stop(); fadeTimer.Start(); };
        fadeTimer.Tick += (_, _) =>
        {
            Opacity = Math.Max(0, Opacity - 0.08);
            if (Opacity <= 0) { fadeTimer.Stop(); Hide(); }
        };
    }

    protected override bool ShowWithoutActivation => true;

    protected override CreateParams CreateParams
    {
        get
        {
            const int WS_EX_TOPMOST = 0x8, WS_EX_TRANSPARENT = 0x20, WS_EX_TOOLWINDOW = 0x80, WS_EX_LAYERED = 0x80000, WS_EX_NOACTIVATE = 0x08000000;
            var cp = base.CreateParams;
            cp.ExStyle |= WS_EX_TOPMOST | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_LAYERED | WS_EX_NOACTIVATE;
            return cp;
        }
    }

    protected override void OnHandleCreated(EventArgs e)
    {
        base.OnHandleCreated(e);
        int round = 2; // DWMWCP_ROUND: Windows 11 rounded corners
        DwmSetWindowAttribute(Handle, 33 /* DWMWA_WINDOW_CORNER_PREFERENCE */, ref round, sizeof(int));
    }

    /// <summary>Shows the keyboard brightness as a fraction of the maximum, 0-1.</summary>
    public void ShowBrightness(float fraction) =>
        Flash(KeyboardGlyph, fraction <= 0 ? "Keyboard light off" : $"Keyboard light  {fraction:P0}", Math.Clamp(fraction, 0, 1));

    public void ShowLightBar(bool on) => Flash(LightGlyph, on ? "Light bar on" : "Light bar off", -1);

    public void ShowTouchpad(bool enabled) => Flash(TouchpadGlyph, enabled ? "Touchpad on" : "Touchpad off", -1);

    public void ShowWinKey(bool locked) => Flash(locked ? LockGlyph : UnlockGlyph, locked ? "Windows key locked" : "Windows key unlocked", -1);

    void Flash(string glyph, string caption, float level)
    {
        this.glyph = glyph;
        this.caption = caption;
        this.level = level;
        dark = IsDarkMode();

        float scale = DeviceDpi / 96f;
        Size = new Size((int)(264 * scale), (int)(56 * scale));
        var area = Screen.FromPoint(Cursor.Position).WorkingArea;
        Location = new Point(area.Left + (area.Width - Width) / 2, area.Bottom - Height - (int)(48 * scale));
        BackColor = dark ? Color.FromArgb(44, 44, 44) : Color.FromArgb(249, 249, 249);

        fadeTimer.Stop();
        Opacity = 0.96;
        if (!Visible) Show();
        Invalidate();
        hideTimer.Stop();
        hideTimer.Start();
    }

    protected override void OnPaint(PaintEventArgs e)
    {
        var g = e.Graphics;
        g.SmoothingMode = SmoothingMode.AntiAlias;
        g.TextRenderingHint = System.Drawing.Text.TextRenderingHint.ClearTypeGridFit;
        float s = DeviceDpi / 96f;
        var text = dark ? Color.White : Color.FromArgb(27, 27, 27);
        var accent = dark ? Color.FromArgb(76, 194, 255) : Color.FromArgb(0, 95, 184);
        var track = dark ? Color.FromArgb(90, 90, 90) : Color.FromArgb(200, 200, 200);

        using (var iconFont = new Font(IconFont, 20 * s, GraphicsUnit.Pixel))
        {
            var iconBox = new RectangleF(14 * s, 0, 32 * s, Height);
            TextRenderer.DrawText(g, glyph, iconFont, Rectangle.Round(iconBox), text,
                TextFormatFlags.HorizontalCenter | TextFormatFlags.VerticalCenter | TextFormatFlags.NoPadding);
        }

        using var captionFont = new Font("Segoe UI", 13 * s, GraphicsUnit.Pixel);
        if (level < 0)
        {
            TextRenderer.DrawText(g, caption, captionFont, new Rectangle((int)(56 * s), 0, Width - (int)(64 * s), Height), text,
                TextFormatFlags.VerticalCenter | TextFormatFlags.Left);
            return;
        }

        TextRenderer.DrawText(g, caption, captionFont, new Rectangle((int)(56 * s), (int)(9 * s), Width - (int)(64 * s), (int)(18 * s)), text,
            TextFormatFlags.Left | TextFormatFlags.Top);
        float x = 56 * s, y = 34 * s, h = 4 * s;
        float w = Width - x - 18 * s;
        using (var trackBrush = new SolidBrush(track))
        using (var trackPath = RoundedRect(new RectangleF(x, y, w, h), h / 2))
            g.FillPath(trackBrush, trackPath);
        if (level > 0)
        {
            using var fill = new SolidBrush(accent);
            using var fillPath = RoundedRect(new RectangleF(x, y, Math.Max(h, w * level), h), h / 2);
            g.FillPath(fill, fillPath);
        }
    }

    static GraphicsPath RoundedRect(RectangleF r, float radius)
    {
        var path = new GraphicsPath();
        float d = radius * 2;
        path.AddArc(r.Left, r.Top, d, d, 180, 90);
        path.AddArc(r.Right - d, r.Top, d, d, 270, 90);
        path.AddArc(r.Right - d, r.Bottom - d, d, d, 0, 90);
        path.AddArc(r.Left, r.Bottom - d, d, d, 90, 90);
        path.CloseFigure();
        return path;
    }

    static bool IsDarkMode()
    {
        using var key = Registry.CurrentUser.OpenSubKey(@"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");
        return key?.GetValue("SystemUsesLightTheme") is int light && light == 0;
    }

    [DllImport("dwmapi.dll")]
    static extern int DwmSetWindowAttribute(IntPtr hwnd, int attribute, ref int value, int size);
}
