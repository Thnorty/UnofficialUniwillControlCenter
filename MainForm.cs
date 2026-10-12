using System.Diagnostics;
using System.Runtime.InteropServices;
using Microsoft.Win32;

namespace MonsterKeyboardRGB;

internal sealed class MainForm : Form
{
    // Control Center's "Idle Timer" (KeyboardCloseTimer): switch the backlight off after this long without input.
    static readonly (string Name, int Seconds)[] IdleTimeouts =
    [
        ("Never", 0), ("15 seconds", 15), ("30 seconds", 30), ("1 minute", 60), ("2 minutes", 120), ("5 minutes", 300),
        ("10 minutes", 600), ("15 minutes", 900), ("30 minutes", 1800), ("1 hour", 3600),
    ];

    const int FnKeyStep = 10; // keyboard brightness per Fn key press, out of IteLightingDevice.MaxLight

    public const string TrayArgument = "--tray";

    /// <summary>Posted by a second launch so the running instance shows itself.</summary>
    public static readonly int ShowMessage = RegisterWindowMessage("MonsterKeyboardRGB.Show");

    readonly AppSettings settings = AppSettings.Load();
    readonly bool startHidden;
    readonly MusicVisualizer music = new();
    readonly LightingPanel keyboardPanel;
    LightingPanel? lightBarPanel;
    readonly RadioButton keyboardTab = PageButton("Keyboard");
    readonly RadioButton lightBarTab = PageButton("Light bar");
    readonly FlowLayoutPanel pageButtons = new() { AutoSize = true, WrapContents = false, Margin = new Padding(0, 0, 0, 12), Visible = false };
    readonly Panel pageHost = new() { AutoSize = true, AutoSizeMode = AutoSizeMode.GrowAndShrink, Margin = Padding.Empty };
    readonly ComboBox idleBox = new() { DropDownStyle = ComboBoxStyle.DropDownList, Width = 220 };
    readonly Label status = new() { AutoSize = true, ForeColor = SystemColors.GrayText };
    readonly Button retry = new() { Text = "Retry", AutoSize = true, Visible = false };
    readonly Button touchpadButton = new() { AutoSize = true, Margin = new Padding(3, 6, 3, 3) };
    readonly Button winKeyButton = new() { AutoSize = true, Margin = new Padding(3, 6, 3, 3), Text = "Needs administrator rights", Enabled = false };
    readonly CheckBox startWithWindows = new() { Text = "Start with Windows (in the tray)", AutoSize = true, Margin = new Padding(0, 0, 0, 8) };
    // The touchpad can also be toggled with its Fn shortcut, so keep the button in sync.
    readonly System.Windows.Forms.Timer touchpadRefresh = new() { Interval = 250 };
    readonly System.Windows.Forms.Timer idleCheck = new() { Interval = 100 };
    readonly NotifyIcon tray = new() { Text = "Keyboard RGB" };
    readonly OsdWindow osd = new();
    readonly CancellationTokenSource watcherStop = new();
    // Sleep/resume and Control Center can clear the EC's "app present" bit; keep checking it.
    readonly System.Windows.Forms.Timer hotkeyCheck = new() { Interval = 5000 };

    FirmwareHotkeys? hotkeys;
    string fnKeyMode = "";
    bool keyboardChanged;
    bool lightBarChanged;
    bool exiting;
    bool started;
    bool? lastTouchpadState;
    Icon? currentIcon;
    // Idle timer: the lights are faded out until the next input. The Fn keys reach us through the EC rather than
    // as Windows input, so they count as activity separately.
    bool idleOff;
    long lastHotkeyActivity;

    public MainForm(bool startHidden)
    {
        this.startHidden = startHidden;
        Text = "Keyboard RGB";
        FormBorderStyle = FormBorderStyle.FixedSingle;
        MaximizeBox = false;
        AutoSize = true;
        AutoSizeMode = AutoSizeMode.GrowAndShrink;
        Padding = new Padding(16);
        Font = new Font("Segoe UI", 9.75f);

        keyboardPanel = new LightingPanel(LightingEffect.Keyboard, settings.Keyboard!, powerLabel: null, music);
        keyboardPanel.Changed += () => keyboardChanged = true;
        keyboardPanel.Failed += message => DeviceFailed("keyboard", message);
        keyboardPanel.ColorsChanged += UpdateIcon;

        var layout = new TableLayoutPanel { ColumnCount = 1, AutoSize = true, Dock = DockStyle.Fill };
        pageButtons.Controls.Add(keyboardTab);
        pageButtons.Controls.Add(lightBarTab);
        layout.Controls.Add(pageButtons);
        pageHost.Controls.Add(keyboardPanel);
        layout.Controls.Add(pageHost);

        var shared = new TableLayoutPanel { ColumnCount = 2, AutoSize = true, Margin = new Padding(0, 4, 0, 12) };
        shared.Controls.Add(UiText.RowLabel("Turn off when idle"), 0, 0);
        shared.Controls.Add(idleBox, 1, 0);
        shared.Controls.Add(UiText.RowLabel("Touchpad"), 0, 1);
        shared.Controls.Add(touchpadButton, 1, 1);
        shared.Controls.Add(UiText.RowLabel("Windows key"), 0, 2);
        shared.Controls.Add(winKeyButton, 1, 2);
        layout.Controls.Add(shared);

        layout.Controls.Add(startWithWindows);
        var footer = new FlowLayoutPanel { AutoSize = true, WrapContents = false };
        footer.Controls.Add(status);
        footer.Controls.Add(retry);
        layout.Controls.Add(footer);
        Controls.Add(layout);

        var menu = new ContextMenuStrip();
        menu.Opening += (_, _) => BuildTrayMenu(menu);
        BuildTrayMenu(menu);
        tray.ContextMenuStrip = menu;
        tray.DoubleClick += (_, _) => ShowFromTray();
        tray.Visible = true;

        idleBox.Items.AddRange([.. IdleTimeouts.Select(t => t.Name)]);
        idleBox.SelectedIndex = Math.Max(0, Array.FindIndex(IdleTimeouts, t => t.Seconds == settings.IdleTimeoutSeconds));
        startWithWindows.Checked = Startup.IsEnabled();
        Startup.RemoveLegacyRunEntry();
        keyboardTab.Checked = true;

        keyboardTab.CheckedChanged += (_, _) => ShowPage();
        retry.Click += (_, _) => OpenDevices();
        touchpadButton.Click += (_, _) => ToggleTouchpad();
        winKeyButton.Click += (_, _) => RunEc(h => h.ToggleWinKeyLock()); // the EC answers with a 64/65 event
        touchpadRefresh.Tick += (_, _) => RefreshTouchpad();
        idleCheck.Tick += (_, _) => CheckIdle();
        startWithWindows.CheckedChanged += (_, _) => SetStartWithWindows(startWithWindows.Checked);

        RefreshTouchpad();
        UpdateIcon();
    }

    static RadioButton PageButton(string text) => new()
    {
        Text = text, Appearance = Appearance.Button, AutoSize = true, MinimumSize = new Size(110, 32),
        TextAlign = ContentAlignment.MiddleCenter, Margin = new Padding(0, 0, 6, 0),
    };

    void ShowPage()
    {
        var page = keyboardTab.Checked || lightBarPanel is null ? keyboardPanel : lightBarPanel;
        if (pageHost.Controls.Contains(page)) return;
        SuspendLayout();
        pageHost.Controls.Clear();
        pageHost.Controls.Add(page);
        ResumeLayout();
    }

    // Start in the tray without flashing the window when launched with --tray.
    protected override void SetVisibleCore(bool value)
    {
        if (startHidden && !IsHandleCreated)
        {
            CreateHandle();
            value = false;
        }
        base.SetVisibleCore(value);
    }

    protected override void OnHandleCreated(EventArgs e)
    {
        base.OnHandleCreated(e);
        if (started) return; // the handle can be recreated; only start once
        started = true;
        OpenDevices(); // also takes over the Fn hotkeys
        touchpadRefresh.Start();
        idleCheck.Start();
        hotkeyCheck.Tick += (_, _) => CheckHotkeys();
        hotkeyCheck.Start();
        SystemEvents.PowerModeChanged += OnPowerModeChanged;
        _ = WatchBrightnessAsync(watcherStop.Token);
    }

    void OpenDevices()
    {
        retry.Visible = false;
        try
        {
            if (keyboardPanel.Device is null)
            {
                var keyboard = IteLightingDevice.Open(LightingDeviceKind.Keyboard);
                if (keyboard is null)
                {
                    SetStatus("No ITE four-zone keyboard controller found.", error: true);
                    retry.Visible = true;
                    return;
                }
                // Keep a nonzero brightness the keyboard reports, since the Fn keys may have changed it while this
                // app wasn't running; a zero is more likely the lights having been reset.
                if (keyboard.ReadLight() is var light and > 0) keyboardPanel.ShowLight(light);
                keyboardPanel.Attach(keyboard); // restores the saved lighting
                if (keyboardPanel.Device is null) return; // the send failed and was reported
            }
            if (lightBarPanel is null) OpenLightBar();
            else if (lightBarPanel.Device is null && IteLightingDevice.Open(LightingDeviceKind.LightBar) is { } lightBar)
                lightBarPanel.Attach(lightBar);
            SetConnectedStatus();
            StartHotkeys();
        }
        catch (Exception e)
        {
            SetStatus($"Can't talk to the lighting controller: {e.Message}", error: true);
            retry.Visible = true;
        }
    }

    void OpenLightBar()
    {
        var lightBar = IteLightingDevice.Open(LightingDeviceKind.LightBar);
        if (lightBar is null) return; // this laptop has none
        if (settings.LightBar is null)
        {
            // First run: start from what the light bar is showing now.
            var (on, effectId) = lightBar.ReadState();
            settings.LightBar = new LightingSettings
            {
                On = on,
                Effect = LightingEffect.LightBar.FirstOrDefault(e => e.Id == effectId)?.Key ?? "static",
            };
        }
        lightBarPanel = new LightingPanel(LightingEffect.LightBar, settings.LightBar, powerLabel: "Light bar on", music);
        lightBarPanel.Changed += () => lightBarChanged = true;
        lightBarPanel.Failed += message => DeviceFailed("light bar", message);
        lightBarPanel.AllowLink(keyboardPanel, "Same as keyboard");
        lightBarPanel.Attach(lightBar);
        pageButtons.Visible = true;
    }

    /// <summary>Quick settings in the tray icon's menu, built fresh each time it opens.</summary>
    void BuildTrayMenu(ContextMenuStrip menu)
    {
        menu.Items.Clear();
        var keyboard = new ToolStripMenuItem("Keyboard");
        AddEffectItems(keyboard, keyboardPanel, enabled: true);
        keyboard.DropDownItems.Add(new ToolStripSeparator());
        var brightness = new ToolStripMenuItem("Brightness");
        for (int percent = 0; percent <= 100; percent += 20)
        {
            byte light = (byte)(percent * IteLightingDevice.MaxLight / 100);
            var item = new ToolStripMenuItem(percent == 0 ? "Off" : $"{percent}%") { Checked = keyboardPanel.Light == light };
            item.Click += (_, _) => keyboardPanel.SetLight(light);
            brightness.DropDownItems.Add(item);
        }
        keyboard.DropDownItems.Add(brightness);
        menu.Items.Add(keyboard);

        if (lightBarPanel is { } bar)
        {
            var lightBar = new ToolStripMenuItem("Light bar");
            var on = new ToolStripMenuItem("On") { Checked = bar.IsOn };
            on.Click += (_, _) => bar.SetPower(!bar.IsOn);
            var linked = new ToolStripMenuItem("Same as keyboard") { Checked = bar.Linked, Enabled = bar.IsOn };
            linked.Click += (_, _) => bar.Linked = !bar.Linked;
            lightBar.DropDownItems.Add(on);
            lightBar.DropDownItems.Add(linked);
            lightBar.DropDownItems.Add(new ToolStripSeparator());
            AddEffectItems(lightBar, bar, enabled: bar.IsOn && !bar.Linked);
            menu.Items.Add(lightBar);
        }

        menu.Items.Add(new ToolStripSeparator());
        menu.Items.Add("Open", null, (_, _) => ShowFromTray());
        menu.Items.Add("Exit", null, (_, _) => ExitApp());
    }

    static void AddEffectItems(ToolStripMenuItem parent, LightingPanel panel, bool enabled)
    {
        foreach (var effect in panel.Effects)
        {
            var item = new ToolStripMenuItem(effect.DisplayName) { Checked = effect == panel.SelectedEffect, Enabled = enabled };
            item.Click += (_, _) => panel.SelectEffect(effect);
            parent.DropDownItems.Add(item);
        }
    }

    void DeviceFailed(string device, string message)
    {
        SetStatus($"Sending to the {device} failed: {message}", error: true);
        retry.Visible = true;
    }

    /// <summary>
    /// Takes the Fn hotkeys over from the firmware so the keyboard-light keys apply instantly and the Win-lock key
    /// reports its state. Without admin rights the firmware keeps handling them (slower, and no Win-lock OSD).
    /// </summary>
    void StartHotkeys()
    {
        if (keyboardPanel.Device is null || hotkeys is not null) return;
        if (ControlCenterIsRunning())
        {
            SetFnKeyMode("Fn keys: handled by Control Center");
            return;
        }
        hotkeys = FirmwareHotkeys.TryStart(out string? failure);
        if (hotkeys is null)
        {
            SetFnKeyMode($"Fn keys: handled by firmware ({failure})");
            return;
        }
        hotkeys.HotkeyPressed += code => BeginInvoke(() => OnHotkey(code));
        SetFnKeyMode("Fn keys: instant");
        RefreshWinKey();
    }

    /// <summary>EC calls go through WMI and take a few milliseconds; keep them off the UI thread.</summary>
    void RunEc(Action<FirmwareHotkeys> action)
    {
        var current = hotkeys;
        if (current is null) return;
        Task.Run(() =>
        {
            try { action(current); }
            catch (Exception) { } // transient WMI failure; the periodic check retries what matters
        });
    }

    void RefreshWinKey()
    {
        var current = hotkeys;
        if (current is null)
        {
            winKeyButton.Enabled = false;
            winKeyButton.Text = "Needs administrator rights";
            return;
        }
        Task.Run(() =>
        {
            try
            {
                bool locked = current.IsWinKeyLocked();
                BeginInvoke(() => ShowWinKeyState(locked));
            }
            catch (Exception) { }
        });
    }

    void ShowWinKeyState(bool locked)
    {
        winKeyButton.Enabled = hotkeys is not null;
        winKeyButton.Text = locked ? "Locked — click to unlock" : "Unlocked — click to lock";
    }

    void CheckHotkeys()
    {
        if (hotkeys is null)
        {
            StartHotkeys(); // e.g. Control Center was stopped
            return;
        }
        if (ControlCenterIsRunning())
        {
            // Its service owns the bit now; step back without clearing it.
            hotkeys.Abandon();
            hotkeys.Dispose();
            hotkeys = null;
            SetFnKeyMode("Fn keys: handled by Control Center");
            RefreshWinKey();
            return;
        }
        RunEc(h => h.EnsureApExist());
        RefreshWinKey();
    }

    void OnPowerModeChanged(object? sender, PowerModeChangedEventArgs e)
    {
        if (e.Mode == PowerModes.Resume) BeginInvoke(CheckHotkeys);
    }

    void OnHotkey(int code)
    {
        switch (code)
        {
            case FirmwareHotkeys.KeyboardLightDown:
            case FirmwareHotkeys.KeyboardLightUp:
            case FirmwareHotkeys.KeyboardLightCycle:
                lastHotkeyActivity = Environment.TickCount64;
                if (idleOff) WakeFromIdle();
                // The keys move the brightness by a fifth of the range (20%), from wherever the slider is.
                byte current = keyboardPanel.Light;
                byte light = code switch
                {
                    FirmwareHotkeys.KeyboardLightDown => (byte)Math.Max(0, current - FnKeyStep),
                    FirmwareHotkeys.KeyboardLightUp => (byte)Math.Min(IteLightingDevice.MaxLight, current + FnKeyStep),
                    _ => current >= IteLightingDevice.MaxLight ? (byte)0 : (byte)Math.Min(IteLightingDevice.MaxLight, current + FnKeyStep), // past the top wraps to off
                };
                keyboardPanel.SetLight(light);
                osd.ShowBrightness(light / (float)IteLightingDevice.MaxLight);
                break;
            case FirmwareHotkeys.LightBarToggle:
                lastHotkeyActivity = Environment.TickCount64;
                if (lightBarPanel is null) break;
                lightBarPanel.TogglePower();
                osd.ShowLightBar(lightBarPanel.IsOn);
                break;
            case FirmwareHotkeys.WinKeyLocked:
            case FirmwareHotkeys.WinKeyUnlocked:
                bool locked = code == FirmwareHotkeys.WinKeyLocked;
                ShowWinKeyState(locked);
                if (ActiveForm != this) osd.ShowWinKey(locked); // no popup when the click came from this window
                break;
        }
    }

    /// <summary>
    /// About a second after the Fn keys go back to the firmware, the EC applies its own internal keyboard level,
    /// which isn't the level shown and isn't in any EC register we can see (it switched the lights off in testing;
    /// Control Center has the same behaviour). Wait for that and put the user's brightness back.
    /// </summary>
    void RestoreBrightnessAfterHandOver()
    {
        var keyboard = keyboardPanel.Device;
        if (keyboard is null) return;
        byte light = keyboardPanel.Light;
        var deadline = Environment.TickCount64 + 3000;
        try
        {
            while (Environment.TickCount64 < deadline)
            {
                if (keyboard.ReadLight() != light && !keyboard.TrySetLight(light))
                {
                    keyboardPanel.Apply(saveToFirmware: false);
                    if (keyboardPanel.Device is null) return;
                }
                Thread.Sleep(50);
            }
        }
        catch (Exception) { } // exiting anyway
    }

    void SetFnKeyMode(string text)
    {
        fnKeyMode = text;
        if (keyboardPanel.Device is not null) SetConnectedStatus();
    }

    void SetConnectedStatus()
    {
        var (major, minor) = keyboardPanel.Device!.FirmwareVersion;
        string devices = $"keyboard firmware {major:X2}.{minor:X2}";
        if (lightBarPanel?.Device is { } lightBar)
            devices += $", light bar firmware {lightBar.FirmwareVersion.Major:X2}.{lightBar.FirmwareVersion.Minor:X2}";
        SetStatus($"Connected ({devices}). {fnKeyMode}");
    }

    protected override void WndProc(ref Message m)
    {
        if (m.Msg == ShowMessage) ShowFromTray();
        base.WndProc(ref m);
    }

    void ShowFromTray()
    {
        Show();
        WindowState = FormWindowState.Normal;
        Activate();
    }

    void ExitApp()
    {
        exiting = true;
        Close();
    }

    /// <summary>
    /// Without Control Center the laptop's firmware handles the keyboard-light Fn keys itself and tells nobody,
    /// so poll the controller and react when its brightness changes.
    /// </summary>
    async Task WatchBrightnessAsync(CancellationToken stop)
    {
        while (!stop.IsCancellationRequested)
        {
            try
            {
                await Task.Delay(100, stop);
            }
            catch (OperationCanceledException)
            {
                return;
            }
            var keyboard = keyboardPanel.Device;
            // Fades, Music and the idle timer change the brightness on purpose.
            if (keyboard is null || BrightnessIsBusy()) continue;
            byte? changed = await Task.Run(() =>
            {
                try { return keyboard.PollExternalLightChange(); }
                catch (Exception) { return null; } // device gone or handle closed; the UI path reports send errors
            });
            if (changed is byte light && !BrightnessIsBusy()) OnKeyboardBrightnessChanged(light);
        }
    }

    bool BrightnessIsBusy() => keyboardPanel.Fading || idleOff || music.IsPlaying(keyboardPanel);

    void OnKeyboardBrightnessChanged(byte light)
    {
        lastHotkeyActivity = Environment.TickCount64;
        keyboardPanel.ShowLight(light);
        keyboardChanged = true;
        if (!ControlCenterIsRunning()) osd.ShowBrightness(light / (float)IteLightingDevice.MaxLight);
    }

    // Control Center shows its own OSD for the same keys.
    static bool ControlCenterIsRunning()
    {
        var processes = Process.GetProcessesByName("GCUService");
        foreach (var p in processes) p.Dispose();
        return processes.Length > 0;
    }

    void CheckIdle()
    {
        if (keyboardPanel.Device is null) return;
        long idle = Math.Min(MillisecondsSinceInput(), Environment.TickCount64 - lastHotkeyActivity);
        if (idleOff)
        {
            if (idle < 1000) WakeFromIdle();
            return;
        }
        int timeout = IdleTimeouts[Math.Max(0, idleBox.SelectedIndex)].Seconds;
        if (timeout > 0 && idle >= timeout * 1000L)
        {
            idleOff = true;
            _ = keyboardPanel.FadeOutAsync();
            if (lightBarPanel is not null) _ = lightBarPanel.FadeOutAsync();
        }
    }

    void WakeFromIdle()
    {
        idleOff = false;
        keyboardPanel.Wake();
        lightBarPanel?.Wake();
    }

    static long MillisecondsSinceInput()
    {
        var info = new LastInputInfo { Size = (uint)Marshal.SizeOf<LastInputInfo>() };
        if (!GetLastInputInfo(ref info)) return 0;
        return (uint)Environment.TickCount - info.Time; // both wrap at 2^32 ms
    }

    void ToggleTouchpad()
    {
        try
        {
            Touchpad.Toggle();
        }
        catch (Exception e)
        {
            SetStatus(e.Message, error: true);
        }
    }

    void RefreshTouchpad()
    {
        bool? state = Touchpad.IsEnabled();
        if (state is bool enabled && lastTouchpadState is bool previous && enabled != previous
            && ActiveForm != this && !ControlCenterIsRunning())
        {
            osd.ShowTouchpad(enabled);
        }
        lastTouchpadState = state;
        touchpadButton.Text = state switch
        {
            true => "On — click to turn off",
            false => "Off — click to turn on",
            null => "Toggle on/off",
        };
    }

    /// <summary>The window and tray icon show the keyboard's colors.</summary>
    void UpdateIcon()
    {
        var colors = keyboardPanel.IconColors;
        using var bitmap = new Bitmap(32, 32);
        using (var g = Graphics.FromImage(bitmap))
        {
            g.Clear(Color.Transparent);
            using var frame = new SolidBrush(Color.FromArgb(40, 40, 40));
            g.FillRectangle(frame, 0, 6, 32, 20);
            for (int i = 0; i < 4; i++)
            {
                using var zone = new SolidBrush(colors[i % colors.Count]);
                g.FillRectangle(zone, 2 + i * 7, 8, 6, 16);
            }
        }
        var icon = Icon.FromHandle(bitmap.GetHicon());
        Icon = icon;
        tray.Icon = icon;
        if (currentIcon is not null) DestroyIcon(currentIcon.Handle);
        currentIcon = icon;
    }

    void SetStatus(string text, bool error = false)
    {
        status.Text = text;
        status.ForeColor = error ? Color.Firebrick : SystemColors.GrayText;
    }

    void SetStartWithWindows(bool enabled)
    {
        try
        {
            if (enabled) Startup.Enable();
            else Startup.Disable();
        }
        catch (Exception e) when (e is InvalidOperationException or IOException or UnauthorizedAccessException or System.ComponentModel.Win32Exception)
        {
            SetStatus($"Couldn't change startup setting: {e.Message}", error: true);
        }
    }

    protected override void OnFormClosing(FormClosingEventArgs e)
    {
        // The window's close button only hides it; the tray icon keeps the OSD running.
        if (e.CloseReason == CloseReason.UserClosing && !exiting)
        {
            e.Cancel = true;
            Hide();
            return;
        }

        watcherStop.Cancel();
        hotkeyCheck.Stop();
        idleCheck.Stop();
        if (idleOff) WakeFromIdle();
        music.Dispose();
        SystemEvents.PowerModeChanged -= OnPowerModeChanged;
        // Live changes skip the firmware's non-volatile save to avoid a flash write per slider step;
        // commit the final state once so it survives without this app running. Music can't run without
        // this app, so its static zone colors are what stays.
        if (keyboardPanel.HasPendingChange || keyboardChanged) keyboardPanel.Apply(saveToFirmware: true);
        if (lightBarPanel is not null && (lightBarPanel.HasPendingChange || lightBarChanged)) lightBarPanel.Apply(saveToFirmware: true);
        if (hotkeys is not null)
        {
            Hide();
            tray.Visible = false;
            hotkeys.Dispose(); // hands the Fn keys back to the firmware
            hotkeys = null;
            RestoreBrightnessAfterHandOver();
        }
        keyboardPanel.Store(settings.Keyboard!);
        if (lightBarPanel is not null) lightBarPanel.Store(settings.LightBar!);
        settings.IdleTimeoutSeconds = IdleTimeouts[Math.Max(0, idleBox.SelectedIndex)].Seconds;
        settings.Save();
        tray.Visible = false;
        keyboardPanel.Device?.Dispose();
        lightBarPanel?.Device?.Dispose();
        base.OnFormClosing(e);
    }

    [StructLayout(LayoutKind.Sequential)]
    struct LastInputInfo
    {
        public uint Size;
        public uint Time;
    }

    [DllImport("user32.dll")]
    static extern bool GetLastInputInfo(ref LastInputInfo info);

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    static extern int RegisterWindowMessage(string message);

    [DllImport("user32.dll")]
    static extern bool DestroyIcon(IntPtr handle);
}
