namespace MonsterKeyboardRGB;

/// <summary>
/// The lighting controls of one device (the keyboard or the light bar): power, effect, colors, brightness and
/// speed, and sending them to the controller.
/// </summary>
internal sealed class LightingPanel : UserControl
{
    static readonly Color[] Presets =
    [
        Color.FromArgb(255, 0, 0), Color.FromArgb(255, 80, 0), Color.FromArgb(255, 200, 0), Color.FromArgb(0, 255, 0),
        Color.FromArgb(0, 255, 160), Color.FromArgb(0, 200, 255), Color.FromArgb(0, 40, 255), Color.FromArgb(140, 0, 255),
        Color.FromArgb(255, 0, 160), Color.FromArgb(255, 255, 255),
    ];

    // LM_ITE_RGB.Translate_ITE_DirectionValue: LeftRight = 1, RightLeft = 2.
    static readonly (string Name, byte Value)[] WaveDirections = [("Left → right", 1), ("Right → left", 2)];

    // Brightness changes fade over a few frames using the brightness-only command.
    const int FadeMilliseconds = 180;

    readonly MusicVisualizer? music;
    readonly Color[] zoneColors = new Color[4];
    readonly Dictionary<string, List<Color>> palettes = [];
    readonly Button[] zoneButtons = new Button[4];
    readonly CheckBox powerSwitch = new() { AutoSize = true, Margin = new Padding(0, 0, 0, 10) };
    readonly ComboBox effectBox = new() { DropDownStyle = ComboBoxStyle.DropDownList, Width = 220 };
    readonly Label effectHint = new() { AutoSize = true, ForeColor = SystemColors.GrayText, MaximumSize = new Size(420, 0), Margin = new Padding(0, 0, 0, 10) };
    readonly ComboBox directionBox = new() { DropDownStyle = ComboBoxStyle.DropDownList, Width = 220 };
    readonly Label directionLabel = UiText.RowLabel("Direction");
    // The controllers take any brightness from 0 to 50; Control Center only uses 0, 8, 22, 36 and 50.
    readonly TrackBar brightness = new()
    {
        Minimum = 0, Maximum = IteLightingDevice.MaxLight, Value = IteLightingDevice.MaxLight, TickStyle = TickStyle.BottomRight,
        TickFrequency = 5, Width = 220, SmallChange = 1, LargeChange = 5,
    };
    readonly TrackBar speed = new() { Minimum = 0, Maximum = 4, TickStyle = TickStyle.BottomRight, Width = 220, LargeChange = 1 };
    readonly Label brightnessValue = new() { AutoSize = true, Padding = new Padding(0, 8, 0, 0) };
    readonly Label speedValue = new() { AutoSize = true, Padding = new Padding(0, 8, 0, 0) };
    readonly Panel zonePanel = new() { AutoSize = true };
    readonly FlowLayoutPanel paletteRow = new() { AutoSize = true, WrapContents = false, Margin = new Padding(0, 0, 0, 12) };
    readonly Label paletteHeading = UiText.Heading("");
    readonly Panel palettePanel = new() { AutoSize = true };
    readonly TableLayoutPanel settingsLayout = new() { ColumnCount = 1, AutoSize = true, Margin = Padding.Empty };
    readonly ToolTip toolTip = new();
    readonly System.Windows.Forms.Timer sendDebounce = new() { Interval = 80 };

    IteLightingDevice? device;
    bool suppressSend;
    int fadeGeneration;
    byte shownLight; // the brightness the device is showing, including partway through a fade

    /// <param name="powerLabel">Text of an on/off switch, or null for none.</param>
    public LightingPanel(IReadOnlyList<LightingEffect> effects, LightingSettings settings, string? powerLabel, MusicVisualizer? music = null)
    {
        this.music = music;
        AutoSize = true;
        AutoSizeMode = AutoSizeMode.GrowAndShrink;

        var layout = new TableLayoutPanel { ColumnCount = 1, AutoSize = true, Margin = Padding.Empty };
        if (powerLabel is not null)
        {
            powerSwitch.Text = powerLabel;
            layout.Controls.Add(powerSwitch);
        }

        var effectTable = new TableLayoutPanel { ColumnCount = 2, AutoSize = true, Margin = new Padding(0, 0, 0, 4) };
        effectTable.Controls.Add(UiText.RowLabel("Effect"), 0, 0);
        effectTable.Controls.Add(effectBox, 1, 0);
        effectTable.Controls.Add(directionLabel, 0, 1);
        effectTable.Controls.Add(directionBox, 1, 1);
        settingsLayout.Controls.Add(effectTable);
        settingsLayout.Controls.Add(effectHint);

        // Zone colors: the keyboard's Static and Music.
        var zoneLayout = new FlowLayoutPanel { FlowDirection = FlowDirection.TopDown, AutoSize = true, WrapContents = false, Margin = Padding.Empty };
        zoneLayout.Controls.Add(UiText.Heading("Zones (left → right)"));
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
        zoneLayout.Controls.Add(UiText.Heading("Quick colors (all zones)"));
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
        settingsLayout.Controls.Add(zonePanel);

        // Color palette: effects that cycle through colors.
        var paletteLayout = new FlowLayoutPanel { FlowDirection = FlowDirection.TopDown, AutoSize = true, WrapContents = false, Margin = Padding.Empty };
        paletteLayout.Controls.Add(paletteHeading);
        paletteLayout.Controls.Add(paletteRow);
        palettePanel.Controls.Add(paletteLayout);
        settingsLayout.Controls.Add(palettePanel);

        var sliders = new TableLayoutPanel { ColumnCount = 3, AutoSize = true, Margin = Padding.Empty };
        sliders.Controls.Add(UiText.RowLabel("Brightness"), 0, 0);
        sliders.Controls.Add(brightness, 1, 0);
        sliders.Controls.Add(brightnessValue, 2, 0);
        sliders.Controls.Add(UiText.RowLabel("Speed"), 0, 1);
        sliders.Controls.Add(speed, 1, 1);
        sliders.Controls.Add(speedValue, 2, 1);
        settingsLayout.Controls.Add(sliders);
        layout.Controls.Add(settingsLayout);
        Controls.Add(layout);

        powerSwitch.Checked = powerLabel is null || settings.On;
        for (int i = 0; i < 4; i++)
            zoneColors[i] = settings.ZoneColors.Length == 4 ? Color.FromArgb(settings.ZoneColors[i]) : Color.Red;
        foreach (var effect in effects.Where(e => e.Colors == EffectColors.Palette))
        {
            palettes[effect.Key] = settings.Palettes.TryGetValue(effect.Key, out var saved) && saved.Length > 0 && saved.Length <= effect.MaxColors
                ? [.. saved.Select(Color.FromArgb)]
                : [.. effect.DefaultColors];
        }
        effectBox.Items.AddRange([.. effects]);
        effectBox.SelectedItem = effects.FirstOrDefault(e => e.Key == settings.Effect) ?? effects[0];
        directionBox.Items.AddRange([.. WaveDirections.Select(d => d.Name)]);
        directionBox.SelectedIndex = Math.Max(0, Array.FindIndex(WaveDirections, d => d.Value == settings.WaveDirection));
        brightness.Value = Math.Clamp(settings.Light, 0, IteLightingDevice.MaxLight);
        speed.Value = Math.Clamp(settings.Speed, 0, 4);

        powerSwitch.CheckedChanged += (_, _) => { settingsLayout.Enabled = powerSwitch.Checked; QueueSend(); };
        effectBox.SelectedIndexChanged += (_, _) => { UpdateEffectControls(); QueueSend(); };
        directionBox.SelectedIndexChanged += (_, _) => QueueSend();
        brightness.ValueChanged += (_, _) => { UpdateLabels(); if (!suppressSend) FadeTo(Light); };
        speed.ValueChanged += (_, _) => { UpdateLabels(); QueueSend(); };
        sendDebounce.Tick += (_, _) => { sendDebounce.Stop(); Apply(saveToFirmware: false); };

        settingsLayout.Enabled = powerSwitch.Checked;
        RefreshZoneButtons();
        UpdateLabels();
        UpdateEffectControls();
    }

    /// <summary>Something was sent to the device.</summary>
    public event Action? Changed;

    /// <summary>Sending failed; the device has been closed.</summary>
    public event Action<string>? Failed;

    /// <summary>The colors shown changed (for the window icon).</summary>
    public event Action? ColorsChanged;

    public IteLightingDevice? Device => device;

    public LightingEffect SelectedEffect => (LightingEffect)effectBox.SelectedItem!;

    public bool IsOn => powerSwitch.Checked;

    /// <summary>The brightness setting, 0-<see cref="IteLightingDevice.MaxLight"/>.</summary>
    public byte Light => (byte)brightness.Value;

    /// <summary>True while a fade is changing the brightness, so a changed reading isn't someone else's doing.</summary>
    public bool Fading { get; private set; }

    /// <summary>Faded out by the idle timer until the next input.</summary>
    public bool IdleOff { get; private set; }

    /// <summary>A pending change that hasn't been sent yet.</summary>
    public bool HasPendingChange => sendDebounce.Enabled;

    byte SelectedDirection => WaveDirections[Math.Max(0, directionBox.SelectedIndex)].Value;

    /// <summary>The colors the selected effect shows, for the window icon.</summary>
    public IReadOnlyList<Color> IconColors => SelectedEffect switch
    {
        { Colors: EffectColors.Palette } effect => palettes[effect.Key],
        { Id: LightingEffect.RainbowId } => [Color.Red, Color.Lime, Color.Blue, Color.Magenta],
        _ => zoneColors,
    };

    IReadOnlyList<Color> EffectColorsFor(LightingEffect effect) => effect.Colors switch
    {
        EffectColors.Zones => zoneColors,
        EffectColors.Palette => palettes[effect.Key],
        _ => [],
    };

    public void Attach(IteLightingDevice device)
    {
        this.device = device;
        Apply(saveToFirmware: false);
    }

    /// <summary>Shows a brightness the device already has (changed by the firmware), without sending it.</summary>
    public void ShowLight(byte light)
    {
        WithoutSending(() => brightness.Value = Math.Min(light, IteLightingDevice.MaxLight));
        shownLight = light;
    }

    /// <summary>Sets the brightness and fades the device to it.</summary>
    public void SetLight(byte light)
    {
        if (light == Light) return;
        WithoutSending(() => brightness.Value = Math.Min(light, IteLightingDevice.MaxLight));
        FadeTo(light);
    }

    /// <summary>Turns the device on or off with its power switch.</summary>
    public void TogglePower() => powerSwitch.Checked = !powerSwitch.Checked;

    public void Store(LightingSettings settings)
    {
        settings.On = powerSwitch.Checked;
        settings.Effect = SelectedEffect.Key;
        settings.ZoneColors = [.. zoneColors.Select(c => c.ToArgb())];
        settings.Palettes = palettes.ToDictionary(p => p.Key, p => p.Value.Select(c => c.ToArgb()).ToArray());
        settings.WaveDirection = SelectedDirection;
        settings.Light = brightness.Value;
        settings.Speed = speed.Value;
    }

    /// <summary>HIDKeyboard.CloseAnimation: fade the lights out until <see cref="Wake"/>.</summary>
    public async Task FadeOutAsync()
    {
        if (IdleOff || !IsOn) return;
        IdleOff = true;
        music?.Pause();
        await FadeToAsync(0, 600);
    }

    public void Wake()
    {
        if (!IdleOff) return;
        IdleOff = false;
        if (SelectedEffect.IsMusic) Apply(saveToFirmware: false);
        else _ = FadeToAsync(Light, 300);
    }

    void FadeTo(byte target) => _ = FadeToAsync(target, FadeMilliseconds);

    /// <summary>
    /// Moves the brightness to <paramref name="target"/> smoothly with the brightness-only command. A newer fade or
    /// a full send takes over from one in progress.
    /// </summary>
    async Task FadeToAsync(byte target, int milliseconds)
    {
        if (device is null || !IsOn) return;
        int generation = ++fadeGeneration;
        if (SelectedEffect.IsMusic && music is not null && !IdleOff)
        {
            // The music loop owns the brightness; the slider sets its peak.
            if (music.IsPlaying) music.Play(device, target, speed.Value);
            else Apply(saveToFirmware: false);
            return;
        }
        byte from = shownLight;
        Fading = true;
        try
        {
            long start = Environment.TickCount64;
            while (from != target)
            {
                float t = Math.Min(1f, (Environment.TickCount64 - start) / (float)milliseconds);
                if (t >= 1) break;
                float eased = t * (2 - t); // ease out: quick to respond, gentle to settle
                shownLight = (byte)MathF.Round(from + (target - from) * eased);
                device.SetRawLight(shownLight);
                await Task.Delay(15);
                if (generation != fadeGeneration || device is null) return; // superseded
            }
            if (device.TrySetLight(target)) shownLight = target;
            else Apply(saveToFirmware: false, lightOverride: target); // switched off by something else, or no 09h
            Changed?.Invoke();
        }
        catch (Exception e)
        {
            Fail(e);
        }
        finally
        {
            if (generation == fadeGeneration) Fading = false;
        }
    }

    /// <summary>Sends the whole lighting state to the device.</summary>
    public void Apply(bool saveToFirmware, byte? lightOverride = null)
    {
        if (device is null) return;
        sendDebounce.Stop();
        var effect = SelectedEffect;
        byte light = lightOverride ?? Light;
        fadeGeneration++; // a fade in progress would undo this
        Fading = false;
        music?.Pause(); // the music loop must not change the brightness while the effect is being set
        try
        {
            if (!IsOn)
            {
                device.SwitchOff();
                shownLight = 0;
            }
            else
            {
                device.SetEffect(effect, EffectColorsFor(effect), SelectedDirection, light, speed.Value, saveToFirmware);
                shownLight = light;
                if (effect.IsMusic && music is not null && !IdleOff && !saveToFirmware) music.Play(device, light, speed.Value);
            }
            Changed?.Invoke();
        }
        catch (Exception e)
        {
            Fail(e);
        }
    }

    void Fail(Exception e)
    {
        device?.Dispose();
        device = null;
        Failed?.Invoke(e.Message);
    }

    static Button Swatch(Size size)
    {
        var button = new Button { Size = size, FlatStyle = FlatStyle.Flat, Cursor = Cursors.Hand };
        button.FlatAppearance.BorderColor = SystemColors.ControlDark;
        return button;
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
        ColorsChanged?.Invoke();
    }

    /// <summary>Rebuilds the swatches of the selected effect's palette.</summary>
    void RefreshPalette()
    {
        var effect = SelectedEffect;
        if (effect.Colors != EffectColors.Palette) return;
        var palette = palettes[effect.Key];
        paletteHeading.Text = effect.MaxColors == 1 ? "Color (click to change)" : "Colors (click to change, right-click to remove)";
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
        if (palette.Count < effect.MaxColors)
        {
            var add = new Button { Text = "+", Size = new Size(40, 40), Margin = new Padding(0, 0, 6, 0) };
            toolTip.SetToolTip(add, "Add a color");
            add.Click += (_, _) => AddPaletteColor();
            paletteRow.Controls.Add(add);
        }
        if (effect.MaxColors > 1)
        {
            var reset = new Button { Text = "Default", AutoSize = true, MinimumSize = new Size(0, 40), Margin = new Padding(6, 0, 0, 0) };
            reset.Click += (_, _) => { palettes[effect.Key] = [.. effect.DefaultColors]; PaletteChanged(); };
            paletteRow.Controls.Add(reset);
        }
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
        if (palette.Count >= SelectedEffect.MaxColors || PickColor(palette[^1]) is not Color color) return;
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
        ColorsChanged?.Invoke();
        QueueSend();
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
        ColorsChanged?.Invoke();
        ResumeLayout();
    }

    void UpdateLabels()
    {
        brightnessValue.Text = brightness.Value == 0 ? "Off" : $"{brightness.Value * 100 / IteLightingDevice.MaxLight}%";
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
}

/// <summary>Label styles shared by the windows.</summary>
internal static class UiText
{
    public static Label Heading(string text) => new()
    {
        Text = text,
        AutoSize = true,
        Font = new Font("Segoe UI Semibold", 9.75f),
        Margin = new Padding(0, 0, 0, 6),
    };

    public static Label RowLabel(string text) => new()
    {
        Text = text, AutoSize = true, Anchor = AnchorStyles.Left, Margin = new Padding(0, 6, 12, 6), MinimumSize = new Size(120, 0),
    };
}
