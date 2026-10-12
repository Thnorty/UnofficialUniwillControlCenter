using System.Runtime.InteropServices;

namespace MonsterKeyboardRGB;

static class Program
{
    [STAThread]
    static void Main(string[] args)
    {
        // Two instances would send interleaved commands to the same controller; bring the running one forward instead.
        using var mutex = new Mutex(true, @"Local\MonsterKeyboardRGB", out bool firstInstance);
        if (!firstInstance)
        {
            const nint HWND_BROADCAST = 0xFFFF;
            PostMessage(HWND_BROADCAST, MainForm.ShowMessage, 0, 0);
            return;
        }

        // Never leave the firmware's hotkeys pointed at a process that is gone.
        AppDomain.CurrentDomain.UnhandledException += (_, _) => FirmwareHotkeys.Current?.Release();
        AppDomain.CurrentDomain.ProcessExit += (_, _) => FirmwareHotkeys.Current?.Release();
        Application.ThreadException += (_, e) =>
        {
            FirmwareHotkeys.Current?.Release();
            MessageBox.Show(e.Exception.ToString(), "Keyboard RGB crashed", MessageBoxButtons.OK, MessageBoxIcon.Error);
            Application.Exit();
        };

        ApplicationConfiguration.Initialize();
        Application.Run(new MainForm(startHidden: args.Contains(MainForm.TrayArgument)));
    }

    [DllImport("user32.dll")]
    static extern bool PostMessage(nint hwnd, int message, nint wParam, nint lParam);
}
