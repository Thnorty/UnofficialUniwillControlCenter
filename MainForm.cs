using System.Diagnostics;
using System.Runtime.InteropServices;
using Microsoft.Win32;

namespace MonsterKeyboardRGB;

internal sealed class MainForm : Form
{
    // Control Center's default 7-color palette for animated effects (from its RGBKeyboard.reg defaults).
    static readonly Color[] DefaultPalette =
    [
        Color.FromArgb(255, 0, 0), Color.FromArgb(255, 165, 0), Color.FromArgb(255, 255, 0), Color.FromArgb(0, 255, 0),
        Color.FromArgb(0, 0, 255), Color.FromArgb(0, 255, 255), Color.FromArgb(139, 0, 255),
    ];
    const int MaxPaletteColors = 7; // the controller has seven color slots for effects

    static readonly Color[] Presets =
    [
        Color.FromArgb(255, 0, 0), Color.FromArgb(255, 80, 0), Color.FromArgb(255, 200, 0), Color.FromArgb(0, 255, 0),
        Color.FromArgb(0, 255, 160), Color.FromArgb(0, 200, 255), Color.FromArgb(0, 40, 255), Color.FromArgb(140, 0, 255),
        Color.FromArgb(255, 0, 160), Color.FromArgb(255, 255, 255),
    ];

    // LM_ITE_RGB.Translate_ITE_DirectionValue: LeftRight = 1, RightLeft = 2.
    static readonly (string Name, byte Value)[] WaveDirections = [("Left → right", 1), ("Right → left", 2)];

    // Control Center's "Idle Timer" (KeyboardCloseTimer): switch the backlight off after this long without input.
    static readonly (string Name, int Seconds)[] IdleTimeouts =
    [
        ("Never", 0), ("15 seconds", 15), ("30 seconds", 30), ("1 minute", 60), ("2 minutes", 120), ("5 minutes", 300),
        ("10 minutes", 600), ("15 minutes", 900), ("30 minutes", 1800), ("1 hour", 3600),
    ];

    public const string TrayArgument = "--tray";

    /// <summary>Posted by a second launch so the running instance shows itself.</summary>
    public static readonly int ShowMessage = RegisterWindowMessage("MonsterKeyboardRGB.Show");

    readonly AppSettings settings = AppSettings.Load();
    readonly bool startHidden;
    readonly Color[] zoneColors = new Color[4];
    readonly Dictionary<string, List<Color>> palettes = [];
    readonly Button[] zoneButtons = new Button[4];
    readonly ComboBox effectBox = new() { DropDownStyle = ComboBoxStyle.DropDownList, Width = 220 };
    readonly Label effectHint = new() { AutoSize = true, ForeColor = SystemColors.GrayText, MaximumSize = new Size(420, 0), Margin = new Padding(0, 0, 0, 10) };
    readonly ComboBox directionBox = new() { DropDownStyle = ComboBoxStyle.DropDownList, Width = 220 };
    readonly Label directionLabel = RowLabel("Direction");
    // The controller takes any brightness from 0 to 50; Control Center only uses 0, 8, 22, 36 and 50.
    readonly TrackBar brightness = new()
    {
        Minimum = 0, Maximum = FourZoneKeyboard.MaxLight, Value = FourZoneKeyboard.MaxLight, TickStyle = TickStyle.BottomRight,
        TickFrequency = 5, Width = 220, SmallChange = 1, LargeChange = 5,
    };
    readonly TrackBar speed = new() { Minimum = 0, Maximum = 4, TickStyle = TickStyle.BottomRight, Width = 220, LargeChange = 1 };
    readonly Label brightnessValue = new() { AutoSize = true, Padding = new Padding(0, 8, 0, 0) };
    readonly Label speedValue = new() { AutoSize = true, Padding = new Padding(0, 8, 0, 0) };
    readonly ComboBox idleBox = new() { DropDownStyle = ComboBoxStyle.DropDownList, Width = 220 };
    readonly Label status = new() { AutoSize = true, ForeColor = SystemColors.GrayText };
    readonly Button retry = new() { Text = "Retry", AutoSize = true, Visible = false };
    readonly Panel zonePanel = new() { AutoSize = true };
    readonly FlowLayoutPanel paletteRow = new() { AutoSize = true, WrapContents = false, Margin = new Padding(0, 0, 0, 12) };
    readonly Panel palettePanel = new() { AutoSize = true };
    readonly Button touchpadButton = new() { AutoSize = true, Margin = new Padding(3, 6, 3, 3) };
    readonly Button winKeyButton = new() { AutoSize = true, Margin = new Padding(3, 6, 3, 3), Text = "Needs administrator rights", Enabled = false };
    readonly CheckBox startWithWindows = new() { Text = "Start with Windows (in the tray)", AutoSize = true, Margin = new Padding(0, 0, 0, 8) };
    readonly ToolTip toolTip = new();
    readonly System.Windows.Forms.Timer sendDebounce = new() { Interval = 80 };
    // The touchpad can also be toggled with its Fn shortcut, so keep the button in sync.
    readonly System.Windows.Forms.Timer touchpadRefresh = new() { Interval = 250 };
    readonly System.Windows.Forms.Timer idleCheck = new() { Interval = 100 };
    readonly NotifyIcon tray = new() { Text = "Keyboard RGB" };
    readonly OsdWindow osd = new();
    readonly MusicVisualizer music = new();
    readonly CancellationTokenSource watcherStop = new();
    // Sleep/resume and Control Center can clear the EC's "app present" bit; keep checking it.
    readonly System.Windows.Forms.Timer hotkeyCheck = new() { Interval = 5000 };

    FourZoneKeyboard? keyboard;
    FirmwareHotkeys? hotkeys;
    string fnKeyMode = "";
    bool changedThisSession;
    bool suppressSend;
    bool exiting;
    bool started;
    bool? lastTouchpadState;
    Icon? currentIcon;
    // Idle timer: the lights are faded out until the next input. The Fn keys reach us through the EC rather than
    // as Windows input, so they count as activity separately.
    bool idleOff;
    long lastHotkeyActivity;
    // Brightness changes fade over a few frames using the brightness-only command.
    const int FadeMilliseconds = 180;
    int fadeGeneration;
    bool fading;
    byte shownLight; // the brightness the keyboard is showing, including partway through a fade

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

        var layout = new TableLayoutPanel { ColumnCount = 1, AutoSize = true, Dock = DockStyle.Fill };

        var effectTable = new TableLayoutPanel { ColumnCount = 2, AutoSize = true, Margin = new Padding(0, 0, 0, 4) };
        effectTable.Controls.Add(RowLabel("Effect"), 0, 0);
        effectTable.Controls.Add(effectBox, 1, 0);
        effectTable.Controls.Add(directionLabel, 0, 1);
        effectTable.Controls.Add(directionBox, 1, 1);
        layout.Controls.Add(effectTable);
        layout.Controls.Add(effectHint);

        // Zone colors: Static and Music.
        var zoneLayout = new FlowLayoutPanel { FlowDirection = FlowDirection.TopDown, AutoSize = true, WrapContents = false, Margin = Padding.Empty };
        zoneLayout.Controls.Add(Heading("Zones (left → right)"));
        var zones = new FlowLayoutPanel { AutoSize = true, WrapContents = false, Margin = new Padding(0, 0, 0, 8) };
        for (int i = 0; i < 4; i++)
        {
            int zone = i;
            var cell = new FlowLayoutPanel { FlowDirection = FlowDirection.TopDown, AutoSize = true, Margin = new Padding(0, 0, 8, 0) };
            var button = Swatch(new Size(72, 48));
            button.Click += (_, _) => PickZoneColor(zone);
            zoneButtons[i] = button;
            cell.Controls.Add(button);
            cell.Controls.Add(new Label { Text = $"Zone {i + 1}", AutoSize = true, Anchor = AnchorStyles.None });
            zones.Controls.Add(cell);
        }
        var allButton = new Button { Text = "Set all…", Size = new Size(84, 48), Margin = new Padding(4, 0, 0, 0) };
        allButton.Click += (_, _) => PickAllColor();
        zones.Controls.Add(allButton);
        zoneLayout.Controls.Add(zones);
        zoneLayout.Controls.Add(Heading("Quick colors (all zones)"));
        var presets = new FlowLayoutPanel { AutoSize = true, WrapContents = false, Margin = new Padding(0, 0, 0, 12) };
        foreach (var color in Presets)
        {
            var swatch = Swatch(new Size(30, 30));
            swatch.BackColor = color;
            swatch.Margin = new Padding(0, 0, 6, 0);
            swatch.Click += (_, _) => SetAllZones(color);
            presets.Controls.Add(swatch);
        }
        zoneLayout.Controls.Add(presets);
        zonePanel.Controls.Add(zoneLayout);
        layout.Controls.Add(zonePanel);

        // Color palette: Breathing, Wave, Flash and Mix cycle through up to seven colors.
        var paletteLayout = new FlowLayoutPanel { FlowDirection = FlowDirection.TopDown, AutoSize = true, WrapContents = false, Margin = Padding.Empty };
        paletteLayout.Controls.Add(Heading("Colors (click to change, right-click to remove)"));
        paletteLayout.Controls.Add(paletteRow);
        palettePanel.Controls.Add(paletteLayout);
        layout.Controls.Add(palettePanel);

        var controls = new TableLayoutPanel { ColumnCount = 3, AutoSize = true, Margin = new Padding(0, 0, 0, 12) };
        controls.Controls.Add(RowLabel("Brightness"), 0, 0);
        controls.Controls.Add(brightness, 1, 0);
        controls.Controls.Add(brightnessValue, 2, 0);
        controls.Controls.Add(RowLabel("Speed"), 0, 1);
        controls.Controls.Add(speed, 1, 1);
        controls.Controls.Add(speedValue, 2, 1);
        controls.Controls.Add(RowLabel("Turn off when idle"), 0, 2);
        controls.Controls.Add(idleBox, 1, 2);
        controls.Controls.Add(RowLabel("Touchpad"), 0, 3);
        controls.Controls.Add(touchpadButton, 1, 3);
        controls.Controls.Add(RowLabel("Windows key"), 0, 4);
        controls.Controls.Add(winKeyButton, 1, 4);
        layout.Controls.Add(controls);

        layout.Controls.Add(startWithWindows);
        var footer = new FlowLayoutPanel { AutoSize = true, WrapContents = false };
        footer.Controls.Add(status);
        footer.Controls.Add(retry);
        layout.Controls.Add(footer);
        Controls.Add(layout);

        var menu = new ContextMenuStrip();
        menu.Items.Add("Open", null, (_, _) => ShowFromTray());
        menu.Items.Add("Exit", null, (_, _) => ExitApp());
        tray.ContextMenuStrip = menu;
        tray.DoubleClick += (_, _) => ShowFromTray();
        tray.Visible = true;

        for (int i = 0; i < 4; i++) zoneColors[i] = Color.FromArgb(settings.ZoneColors[i]);
        foreach (var effect in KeyboardEffect.All.Where(e => e.Colors == EffectColors.Palette))
        {
            palettes[effect.Key] = settings.Palettes.TryGetValue(effect.Key, out var saved) && saved.Length is > 0 and <= MaxPaletteColors
                ? [.. saved.Select(Color.FromArgb)]
                : [.. DefaultPalette];
        }
        effectBox.Items.AddRange(KeyboardEffect.All);
        effectBox.SelectedItem = KeyboardEffect.Find(settings.Effect);
        directionBox.Items.AddRange([.. WaveDirections.Select(d => d.Name)]);
        directionBox.SelectedIndex = Math.Max(0, Array.FindIndex(WaveDirections, d => d.Value == settings.WaveDirection));
        brightness.Value = Math.Clamp(settings.Light, 0, FourZoneKeyboard.MaxLight);
        speed.Value = Math.Clamp(settings.Speed, 0, 4);
        idleBox.Items.AddRange([.. IdleTimeouts.Select(t => t.Name)]);
        idleBox.SelectedIndex = Math.Max(0, Array.FindIndex(IdleTimeouts, t => t.Seconds == settings.IdleTimeoutSeconds));
        startWithWindows.Checked = Startup.IsEnabled();
        Startup.RemoveLegacyRunEntry();

        effectBox.SelectedIndexChanged += (_, _) => { UpdateEffectControls(); QueueSend(); };
        directionBox.SelectedIndexChanged += (_, _) => QueueSend();
        brightness.ValueChanged += (_, _) => { UpdateLabels(); if (!suppressSend) FadeTo(Light); };
        speed.ValueChanged += (_, _) => { UpdateLabels(); QueueSend(); };
        sendDebounce.Tick += (_, _) => { sendDebounce.Stop(); Send(saveToFirmware: false); };
        retry.Click += (_, _) => OpenKeyboard();
        touchpadButton.Click += (_, _) => ToggleTouchpad();
        winKeyButton.Click += (_, _) => RunEc(h => h.ToggleWinKeyLock()); // the EC answers with a 64/65 event
        touchpadRefresh.Tick += (_, _) => RefreshTouchpad();
        idleCheck.Tick += (_, _) => CheckIdle();
        startWithWindows.CheckedChanged += (_, _) => SetStartWithWindows(startWithWindows.Checked);

        RefreshTouchpad();
        RefreshZoneButtons();
        UpdateLabels();
        UpdateEffectControls();
    }

    static Label Heading(string text) => new()
    {
        Text = text,
        AutoSize = true,
        Font = new Font("Segoe UI Semibold", 9.75f),
        Margin = new Padding(0, 0, 0, 6),
    };

    static Label RowLabel(string text) => new()
    {
        Text = text, AutoSize = true, Anchor = AnchorStyles.Left, Margin = new Padding(0, 6, 12, 6), MinimumSize = new Size(120, 0),
    };

    static Button Swatch(Size size)
    {
        var button = new Button { Size = size, FlatStyle = FlatStyle.Flat, Cursor = Cursors.Hand };
        button.FlatAppearance.BorderColor = SystemColors.ControlDark;
        return button;
    }

    KeyboardEffect SelectedEffect => (KeyboardEffect)effectBox.SelectedItem!;

    byte SelectedDirection => WaveDirections[Math.Max(0, directionBox.SelectedIndex)].Value;

    byte Light => (byte)brightness.Value;

    /// <summary>The colors the selected effect shows.</summary>
    IReadOnlyList<Color> EffectColorsFor(KeyboardEffect effect) => effect.Colors switch
    {
        EffectColors.Zones => zoneColors,
        EffectColors.Palette => palettes[effect.Key],
        _ => [],
    };

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
        OpenKeyboard(); // also takes over the Fn hotkeys
        touchpadRefresh.Start();
        idleCheck.Start();
        hotkeyCheck.Tick += (_, _) => CheckHotkeys();
        hotkeyCheck.Start();
        SystemEvents.PowerModeChanged += OnPowerModeChanged;
        _ = WatchBrightnessAsync(watcherStop.Token);
    }

    /// <summary>
    /// Takes the Fn hotkeys over from the firmware so the keyboard-light keys apply instantly and the Win-lock key
    /// reports its state. Without admin rights the firmware keeps handling them (slower, and no Win-lock OSD).
    /// </summary>
    void StartHotkeys()
    {
        if (keyboard is null || hotkeys is not null) return;
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
                // The keys step through Control Center's levels, from wherever the slider is.
                var levels = FourZoneKeyboard.StandardLights;
                byte light = code switch
                {
                    FirmwareHotkeys.KeyboardLightDown => levels.LastOrDefault(l => l < Light),
                    FirmwareHotkeys.KeyboardLightUp => levels.FirstOrDefault(l => l > Light, FourZoneKeyboard.MaxLight),
                    _ => levels.FirstOrDefault(l => l > Light), // past the top wraps to off
                };
                if (light != Light)
                {
                    WithoutSending(() => brightness.Value = light);
                    FadeTo(light);
                }
                osd.ShowBrightness(light / (float)FourZoneKeyboard.MaxLight);
                break;
            case FirmwareHotkeys.WinKeyLocked:
            case FirmwareHotkeys.WinKeyUnlocked:
                bool locked = code == FirmwareHotkeys.WinKeyLocked;
                ShowWinKeyState(locked);
                if (ActiveForm != this) osd.ShowWinKey(locked); // no popup when the click came from this window
                break;
        }
    }

    void FadeTo(byte target) => _ = FadeToAsync(target, FadeMilliseconds);

    /// <summary>
    /// Moves the brightness to <paramref name="target"/> smoothly with the brightness-only command. A newer fade or
    /// a full effect send takes over from one in progress.
    /// </summary>
    async Task FadeToAsync(byte target, int milliseconds)
    {
        if (keyboard is null) return;
        int generation = ++fadeGeneration;
        if (SelectedEffect.IsMusic && !idleOff)
        {
            // The music loop owns the brightness; the slider sets its peak.
            if (music.IsPlaying) music.Play(keyboard, target, speed.Value);
            else Send(saveToFirmware: false);
            return;
        }
        byte from = shownLight;
        fading = true;
        try
        {
            long start = Environment.TickCount64;
            while (from != target)
            {
                float t = Math.Min(1f, (Environment.TickCount64 - start) / (float)milliseconds);
                if (t >= 1) break;
                float eased = t * (2 - t); // ease out: quick to respond, gentle to settle
                shownLight = (byte)MathF.Round(from + (target - from) * eased);
                keyboard.SetRawLight(shownLight);
                await Task.Delay(15);
                if (generation != fadeGeneration || keyboard is null) return; // superseded
            }
            if (keyboard.TrySetLight(target)) shownLight = target;
            else Send(saveToFirmware: false, lightOverride: target); // switched off by something else, or no 09h
            changedThisSession = true;
        }
        catch (Exception e)
        {
            SetStatus($"Send failed: {e.Message}", error: true);
        }
        finally
        {
            if (generation == fadeGeneration) fading = false;
        }
    }

    /// <summary>
    /// About a second after the Fn keys go back to the firmware, the EC applies its own internal keyboard level,
    /// which isn't the level shown and isn't in any EC register we can see (it switched the lights off in testing;
    /// Control Center has the same behaviour). Wait for that and put the user's brightness back.
    /// </summary>
    void RestoreBrightnessAfterHandOver()
    {
        if (keyboard is null) return;
        byte light = Light;
        var deadline = Environment.TickCount64 + 3000;
        try
        {
            while (Environment.TickCount64 < deadline)
            {
                if (keyboard.ReadLight() != light)
                {
                    if (!keyboard.TrySetLight(light)) Send(saveToFirmware: false);
                    if (keyboard is null) return;
                }
                Thread.Sleep(50);
            }
        }
        catch (Exception) { } // exiting anyway
    }

    void SetFnKeyMode(string text)
    {
        fnKeyMode = text;
        if (keyboard is not null) SetConnectedStatus();
    }

    void SetConnectedStatus()
    {
        var (major, minor) = keyboard!.FirmwareVersion;
        SetStatus($"Connected (firmware {major:X2}.{minor:X2}). {fnKeyMode}");
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

    void OpenKeyboard()
    {
        retry.Visible = false;
        try
        {
            keyboard = FourZoneKeyboard.Open();
            if (keyboard is null)
            {
                SetStatus("No ITE four-zone keyboard controller found.", error: true);
                retry.Visible = true;
                return;
            }
            // Restore the saved lighting. Keep a nonzero brightness the keyboard reports, since the Fn keys may
            // have changed it while this app wasn't running; a zero is more likely the lights having been reset.
            if (keyboard.ReadLight() is var light and > 0) WithoutSending(() => brightness.Value = light);
            Send(saveToFirmware: false);
            if (keyboard is null) return; // Send reported the failure
            SetConnectedStatus();
            StartHotkeys();
        }
        catch (Exception e)
        {
            keyboard?.Dispose();
            keyboard = null;
            SetStatus($"Can't talk to the keyboard: {e.Message}", error: true);
            retry.Visible = true;
        }
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
            var current = keyboard;
            // Fades, Music and the idle timer change the brightness on purpose.
            if (current is null || fading || idleOff || music.IsPlaying) continue;
            byte? changed = await Task.Run(() =>
            {
                try { return current.PollExternalLightChange(); }
                catch (Exception) { return null; } // device gone or handle closed; the UI path reports send errors
            });
            if (changed is byte light && !fading && !idleOff && !music.IsPlaying) OnKeyboardBrightnessChanged(light);
        }
    }

    void OnKeyboardBrightnessChanged(byte light)
    {
        lastHotkeyActivity = Environment.TickCount64;
        WithoutSending(() => brightness.Value = light);
        shownLight = light;
        changedThisSession = true;
        if (!ControlCenterIsRunning()) osd.ShowBrightness(light / (float)FourZoneKeyboard.MaxLight);
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
        if (keyboard is null) return;
        long idle = Math.Min(MillisecondsSinceInput(), Environment.TickCount64 - lastHotkeyActivity);
        if (idleOff)
        {
            if (idle < 1000) WakeFromIdle();
            return;
        }
        int timeout = IdleTimeouts[Math.Max(0, idleBox.SelectedIndex)].Seconds;
        if (timeout > 0 && brightness.Value > 0 && idle >= timeout * 1000L) _ = FadeOutAsync();
    }

    /// <summary>HIDKeyboard.CloseAnimation: fade the backlight out until the next input.</summary>
    async Task FadeOutAsync()
    {
        idleOff = true;
        music.Pause();
        await FadeToAsync(0, 600);
    }

    void WakeFromIdle()
    {
        idleOff = false;
        if (SelectedEffect.IsMusic) Send(saveToFirmware: false);
        else _ = FadeToAsync(Light, 300);
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

    Color? PickColor(Color initial)
    {
        using var dialog = new ColorDialog { Color = initial, FullOpen = true, AnyColor = true };
        return dialog.ShowDialog(this) == DialogResult.OK ? dialog.Color : null;
    }

    void PickZoneColor(int zone)
    {
        if (PickColor(zoneColors[zone]) is not Color color) return;
        zoneColors[zone] = color;
        ZoneColorsChanged();
    }

    void PickAllColor()
    {
        if (PickColor(zoneColors[0]) is Color color) SetAllZones(color);
    }

    void SetAllZones(Color color)
    {
        Array.Fill(zoneColors, color);
        ZoneColorsChanged();
    }

    void ZoneColorsChanged()
    {
        RefreshZoneButtons();
        QueueSend();
    }

    void RefreshZoneButtons()
    {
        for (int i = 0; i < 4; i++)
        {
            zoneButtons[i].BackColor = zoneColors[i];
            zoneButtons[i].FlatAppearance.MouseOverBackColor = ControlPaint.Light(zoneColors[i], 0.3f);
        }
        UpdateIcon();
    }

    /// <summary>Rebuilds the swatches of the selected effect's palette.</summary>
    void RefreshPalette()
    {
        var effect = SelectedEffect;
        if (effect.Colors != EffectColors.Palette) return;
        var palette = palettes[effect.Key];
        paletteRow.SuspendLayout();
        foreach (Control old in paletteRow.Controls.Cast<Control>().ToArray()) old.Dispose();
        for (int i = 0; i < palette.Count; i++)
        {
            int index = i;
            var swatch = Swatch(new Size(40, 40));
            swatch.BackColor = palette[i];
            swatch.FlatAppearance.MouseOverBackColor = ControlPaint.Light(palette[i], 0.3f);
            swatch.Margin = new Padding(0, 0, 6, 0);
            swatch.Click += (_, _) => EditPaletteColor(index);
            swatch.MouseUp += (_, e) => { if (e.Button == MouseButtons.Right) RemovePaletteColor(index); };
            paletteRow.Controls.Add(swatch);
        }
        if (palette.Count < MaxPaletteColors)
        {
            var add = new Button { Text = "+", Size = new Size(40, 40), Margin = new Padding(0, 0, 6, 0) };
            toolTip.SetToolTip(add, "Add a color");
            add.Click += (_, _) => AddPaletteColor();
            paletteRow.Controls.Add(add);
        }
        var reset = new Button { Text = "Default", AutoSize = true, MinimumSize = new Size(0, 40), Margin = new Padding(6, 0, 0, 0) };
        reset.Click += (_, _) => { palettes[effect.Key] = [.. DefaultPalette]; PaletteChanged(); };
        paletteRow.Controls.Add(reset);
        paletteRow.ResumeLayout();
    }

    void EditPaletteColor(int index)
    {
        var palette = palettes[SelectedEffect.Key];
        if (PickColor(palette[index]) is not Color color) return;
        palette[index] = color;
        PaletteChanged();
    }

    void AddPaletteColor()
    {
        var palette = palettes[SelectedEffect.Key];
        if (palette.Count >= MaxPaletteColors || PickColor(palette[^1]) is not Color color) return;
        palette.Add(color);
        PaletteChanged();
    }

    void RemovePaletteColor(int index)
    {
        var palette = palettes[SelectedEffect.Key];
        if (palette.Count <= 1) return; // an effect needs at least one color
        palette.RemoveAt(index);
        PaletteChanged();
    }

    void PaletteChanged()
    {
        BeginInvoke(RefreshPalette); // the swatch that was clicked is still handling its event; don't dispose it yet
        UpdateIcon();
        QueueSend();
    }

    /// <summary>The window and tray icon show the colors of the selected effect.</summary>
    void UpdateIcon()
    {
        var effect = effectBox.SelectedItem as KeyboardEffect;
        IReadOnlyList<Color> colors = effect switch
        {
            { Colors: EffectColors.Palette } => palettes[effect.Key],
            { Id: KeyboardEffect.RainbowId } => [Color.Red, Color.Lime, Color.Blue, Color.Magenta],
            _ => zoneColors,
        };
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

    void UpdateEffectControls()
    {
        var effect = SelectedEffect;
        SuspendLayout();
        zonePanel.Visible = effect.Colors == EffectColors.Zones;
        palettePanel.Visible = effect.Colors == EffectColors.Palette;
        directionLabel.Visible = directionBox.Visible = effect.HasDirection;
        effectHint.Text = effect.Hint ?? "";
        effectHint.Visible = effect.Hint is not null;
        speed.Enabled = effect.HasSpeed;
        RefreshPalette();
        UpdateIcon();
        ResumeLayout();
    }

    void UpdateLabels()
    {
        brightnessValue.Text = brightness.Value == 0 ? "Off" : $"{brightness.Value * 100 / FourZoneKeyboard.MaxLight}%";
        speedValue.Text = $"{speed.Value} / 4";
    }

    void WithoutSending(Action action)
    {
        suppressSend = true;
        try { action(); } finally { suppressSend = false; }
    }

    void QueueSend()
    {
        if (suppressSend) return;
        sendDebounce.Stop();
        sendDebounce.Start();
    }

    void Send(bool saveToFirmware, byte? lightOverride = null)
    {
        if (keyboard is null) return;
        var effect = SelectedEffect;
        byte light = lightOverride ?? Light;
        fadeGeneration++; // a fade in progress would undo this
        fading = false;
        music.Pause(); // the music loop must not change the brightness while the effect is being set
        try
        {
            keyboard.SetEffect(effect, EffectColorsFor(effect), SelectedDirection, light, speed.Value, saveToFirmware);
            shownLight = light;
            changedThisSession = true;
            if (effect.IsMusic && !idleOff && !saveToFirmware) music.Play(keyboard, light, speed.Value);
        }
        catch (Exception e)
        {
            SetStatus($"Send failed: {e.Message}", error: true);
            keyboard.Dispose();
            keyboard = null;
            retry.Visible = true;
        }
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
        idleOff = false;
        music.Dispose();
        SystemEvents.PowerModeChanged -= OnPowerModeChanged;
        // Live changes skip the firmware's non-volatile save to avoid a flash write per slider step;
        // commit the final state once so it survives without this app running. Music can't run without
        // this app, so its static zone colors are what stays.
        if (sendDebounce.Enabled || changedThisSession) Send(saveToFirmware: true);
        sendDebounce.Stop();
        if (hotkeys is not null)
        {
            Hide();
            tray.Visible = false;
            hotkeys.Dispose(); // hands the Fn keys back to the firmware
            hotkeys = null;
            RestoreBrightnessAfterHandOver();
        }
        settings.ZoneColors = [.. zoneColors.Select(c => c.ToArgb())];
        settings.Effect = SelectedEffect.Key;
        settings.Palettes = palettes.ToDictionary(p => p.Key, p => p.Value.Select(c => c.ToArgb()).ToArray());
        settings.WaveDirection = SelectedDirection;
        settings.Light = brightness.Value;
        settings.Speed = speed.Value;
        settings.IdleTimeoutSeconds = IdleTimeouts[Math.Max(0, idleBox.SelectedIndex)].Seconds;
        settings.Save();
        tray.Visible = false;
        keyboard?.Dispose();
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
