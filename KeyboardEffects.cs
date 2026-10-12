namespace MonsterKeyboardRGB;

/// <summary>
/// The keyboard's Music (LM_ITE_RGB.DLL_SetMusicMode, MUSICTYPE.BRIGHTNESS): the static zone colors, with the
/// brightness following the sound.
/// </summary>
internal sealed class KeyboardMusic(IteLightingDevice keyboard, byte peakLight, int speed) : IEffectTarget
{
    // How much of the level is kept each tick, by speed 0-4: faster speeds fall back sooner.
    static readonly float[] Decay = [0.93f, 0.89f, 0.84f, 0.77f, 0.68f];

    float level;
    int lastSent = -1;

    public EffectInputs Inputs => EffectInputs.Loudness;

    public void Tick(EffectFrame frame)
    {
        level = Math.Max(frame.Loudness, level * Decay[Math.Clamp(speed, 0, 4)]);
        int light = (int)MathF.Round(peakLight * level);
        if (light == lastSent) return;
        keyboard.SetRawLight((byte)light);
        lastSent = light;
    }
}

/// <summary>
/// A spectrum analyzer on the four zones: the lowest notes on the left, the highest on the right, each zone glowing in
/// its color as loud as its band.
/// </summary>
internal sealed class KeyboardSpectrum(IteLightingDevice keyboard, IReadOnlyList<Color> zones, int speed) : IEffectTarget
{
    const double MinInterval = 0.05; // the zones take a full color update; 20 a second is plenty
    readonly float[] levels = new float[4];
    readonly Color[] shown = new Color[4];
    double lastSend = double.NegativeInfinity;

    public EffectInputs Inputs => EffectInputs.Spectrum;

    public void Tick(EffectFrame frame)
    {
        var bands = frame.Spectrum.Bands(4, 50, 12000);
        float fall = (2.5f + 6.5f * Math.Clamp(speed, 0, 4) / 4f) * frame.Delta;
        for (int i = 0; i < 4; i++) levels[i] = Math.Max(bands[i], levels[i] - fall);
        if (frame.Time - lastSend < MinInterval) return;
        var colors = new Color[4];
        for (int i = 0; i < 4; i++)
        {
            float amount = levels[i] * levels[i];
            colors[i] = Color.FromArgb((int)(zones[i].R * amount), (int)(zones[i].G * amount), (int)(zones[i].B * amount));
        }
        if (colors.AsSpan().SequenceEqual(shown)) return;
        keyboard.SetZoneColors(colors);
        colors.CopyTo(shown, 0);
        lastSend = frame.Time;
    }
}

/// <summary>
/// The whole keyboard colored by the CPU or GPU temperature: blue when cool, through green and yellow, to red when
/// hot.
/// </summary>
internal sealed class KeyboardTemperature(IteLightingDevice keyboard, bool gpu) : IEffectTarget
{
    static readonly Color[] Scale5 =
    [
        Color.FromArgb(0, 80, 255), Color.FromArgb(0, 255, 120), Color.FromArgb(255, 220, 0), Color.FromArgb(255, 90, 0),
        Color.FromArgb(255, 0, 0),
    ];
    static readonly Color Unknown = Color.FromArgb(40, 40, 40); // dim white: no reading (e.g. the GPU is off)

    Color shown = Color.Empty;

    public EffectInputs Inputs => gpu ? EffectInputs.GpuTemperature : EffectInputs.CpuTemperature;

    public void Tick(EffectFrame frame)
    {
        float? celsius = gpu ? frame.Status.GpuTemperature : frame.Status.CpuTemperature;
        var color = celsius is float c ? Gradient((c - 35) / (gpu ? 55 : 60)) : Unknown;
        // Only resend for a visible change; readings arrive once a second.
        if (Math.Abs(color.R - shown.R) + Math.Abs(color.G - shown.G) + Math.Abs(color.B - shown.B) < 12 && !shown.IsEmpty) return;
        keyboard.SetZoneColors([color, color, color, color]);
        shown = color;
    }

    static Color Gradient(float t)
    {
        float position = Math.Clamp(t, 0, 1) * (Scale5.Length - 1);
        int i = Math.Min((int)position, Scale5.Length - 2);
        float f = position - i;
        var (a, b) = (Scale5[i], Scale5[i + 1]);
        return Color.FromArgb((int)(a.R + (b.R - a.R) * f), (int)(a.G + (b.G - a.G) * f), (int)(a.B + (b.B - a.B) * f));
    }
}
