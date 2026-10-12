namespace MonsterKeyboardRGB;

/// <summary>
/// Sends a light bar effect drawn by this app, frame by frame, with any indicator flashes on top. The light bar must
/// be in user mode (<see cref="IteLightingDevice.EnterUserMode"/>).
/// </summary>
internal sealed class LightBarRenderer(IteLightingDevice lightBar, LightBarScene scene, Action? overlaysDone = null) : IEffectTarget
{
    readonly Lock overlayLock = new();
    readonly List<LightBarOverlay> overlays = [];
    Color[]? lastSent;
    bool hadOverlays;

    public EffectInputs Inputs => scene.Inputs;

    /// <summary>Shows a flash on top of the effect until it finishes.</summary>
    public void AddOverlay(LightBarOverlay overlay)
    {
        lock (overlayLock) overlays.Add(overlay);
    }

    public void Tick(EffectFrame frame)
    {
        var leds = scene.Render(frame);
        bool anyOverlay;
        lock (overlayLock)
        {
            overlays.RemoveAll(o => !o.Apply(leds, frame.Time));
            anyOverlay = overlays.Count > 0;
        }
        if (hadOverlays && !anyOverlay) overlaysDone?.Invoke();
        hadOverlays = anyOverlay;
        if (lastSent is not null && leds.AsSpan().SequenceEqual(lastSent)) return;
        lightBar.SetLeds(leds);
        lastSent = leds;
    }
}

/// <summary>A light bar effect drawn by this app: one color per LED, left to right, every tick.</summary>
internal abstract class LightBarScene
{
    protected const int Leds = IteLightingDevice.LightBarLeds;
    protected const int Center = Leds / 2;

    public virtual EffectInputs Inputs => EffectInputs.None;

    /// <summary>Returns a new frame (the renderer keeps it to compare with the next one).</summary>
    public abstract Color[] Render(EffectFrame frame);

    protected static Color Scale(Color c, float amount)
    {
        amount = Math.Clamp(amount, 0, 1);
        return Color.FromArgb((int)(c.R * amount), (int)(c.G * amount), (int)(c.B * amount));
    }

    protected static Color Lerp(Color a, Color b, float t)
    {
        t = Math.Clamp(t, 0, 1);
        return Color.FromArgb((int)(a.R + (b.R - a.R) * t), (int)(a.G + (b.G - a.G) * t), (int)(a.B + (b.B - a.B) * t));
    }

    protected static Color Brighter(Color a, Color b) =>
        Color.FromArgb(Math.Max(a.R, b.R), Math.Max(a.G, b.G), Math.Max(a.B, b.B));

    /// <summary>A color from a palette spread over 0-1, blending between neighbours.</summary>
    protected static Color Gradient(IReadOnlyList<Color> palette, float t)
    {
        if (palette.Count == 1) return palette[0];
        float position = Math.Clamp(t, 0, 1) * (palette.Count - 1);
        int index = Math.Min((int)position, palette.Count - 2);
        return Lerp(palette[index], palette[index + 1], position - index);
    }

    /// <summary>How fast things fade per second, by speed 0-4.</summary>
    protected static float Rate(int speed, float slowest, float fastest) =>
        slowest + (fastest - slowest) * Math.Clamp(speed, 0, 4) / 4f;
}

/// <summary>Fixed colors, such as the keyboard's zones as four sections.</summary>
internal sealed class StaticScene(Color[] leds) : LightBarScene
{
    public override Color[] Render(EffectFrame frame) => (Color[])leds.Clone();
}

/// <summary>A dark bar, under an indicator flash shown while a firmware effect is paused.</summary>
internal sealed class DarkScene : LightBarScene
{
    public override Color[] Render(EffectFrame frame) => new Color[Leds];
}

/// <summary>
/// Music (LM_ITE_RGB.AudioData_LightbarEventHandler): a level meter that grows from the middle outwards, each LED
/// colored by its distance from the middle.
/// </summary>
internal sealed class MusicMeterScene(IReadOnlyList<Color> colors, int speed) : LightBarScene
{
    static readonly float[] Decay = [0.93f, 0.89f, 0.84f, 0.77f, 0.68f]; // kept per tick, by speed
    float level;

    public override EffectInputs Inputs => EffectInputs.Loudness;

    public override Color[] Render(EffectFrame frame)
    {
        level = Math.Max(frame.Loudness, level * Decay[Math.Clamp(speed, 0, 4)]);
        var leds = new Color[Leds];
        int lit = (int)MathF.Round(level * (Center + 1)); // LEDs lit on each side, counting the middle one
        for (int distance = 0; distance < lit; distance++)
        {
            // The service gives the middle two steps the first color, then one color per step outwards.
            var color = colors[Math.Clamp(distance - 1, 0, colors.Count - 1)];
            leds[Center - distance] = color;
            leds[Center + distance] = color;
        }
        return leds;
    }
}

/// <summary>
/// A spectrum analyzer, mirrored: the lowest notes in the middle, the highest at the ends. Each LED glows as loud
/// as its band, colored from the palette by pitch.
/// </summary>
internal sealed class SpectrumScene(IReadOnlyList<Color> colors, int speed) : LightBarScene
{
    readonly float[] levels = new float[Center + 1];

    public override EffectInputs Inputs => EffectInputs.Spectrum;

    public override Color[] Render(EffectFrame frame)
    {
        var bands = frame.Spectrum.Bands(levels.Length, 50, 14000);
        float fall = Rate(speed, 2.5f, 9f) * frame.Delta; // how much a band drops per tick
        var leds = new Color[Leds];
        for (int d = 0; d < levels.Length; d++)
        {
            levels[d] = Math.Max(bands[d], levels[d] - fall);
            var color = Scale(Gradient(colors, d / (float)(levels.Length - 1)), levels[d] * levels[d]);
            leds[Center - d] = color;
            leds[Center + d] = color;
        }
        return leds;
    }
}

/// <summary>
/// Star Spark (LM_ITE_RGB.AP_Effect_Task, effect 24): every LED twinkles at a random brightness of its color.
/// </summary>
internal sealed class StarSparkScene(IReadOnlyList<Color> colors, int speed) : LightBarScene
{
    readonly Random random = new();
    readonly float[] brightness = new float[Leds];
    readonly float[] target = new float[Leds];
    double nextChange;

    public override Color[] Render(EffectFrame frame)
    {
        if (frame.Time >= nextChange)
        {
            // The service picks new brightnesses every 150 ms; the speed slider sets how often here.
            for (int i = 0; i < Leds; i++) target[i] = random.NextSingle();
            nextChange = frame.Time + 0.6 / Rate(speed, 1, 5);
        }
        float step = Rate(speed, 2, 10) * frame.Delta;
        var leds = new Color[Leds];
        for (int i = 0; i < Leds; i++)
        {
            brightness[i] += Math.Clamp(target[i] - brightness[i], -step, step);
            leds[i] = Scale(colors[i % colors.Count], brightness[i] * brightness[i]);
        }
        return leds;
    }
}

/// <summary>
/// Star Hitting (LM_ITE_RGB.AP_Effect_Task, effect 25): a ball of light per color bouncing along the bar, turning
/// back at the ends and off each other.
/// </summary>
internal sealed class StarHittingScene : LightBarScene
{
    readonly IReadOnlyList<Color> colors;
    readonly float velocity;
    readonly float[] position;
    readonly float[] direction;

    public StarHittingScene(IReadOnlyList<Color> colors, int speed)
    {
        this.colors = colors;
        velocity = Rate(speed, 4, 18); // LEDs per second
        position = new float[colors.Count];
        direction = new float[colors.Count];
        for (int i = 0; i < colors.Count; i++)
        {
            position[i] = (i + 0.5f) * (Leds - 1) / colors.Count;
            direction[i] = i % 2 == 0 ? 1 : -1; // like the service: alternate directions
        }
    }

    public override Color[] Render(EffectFrame frame)
    {
        for (int i = 0; i < position.Length; i++)
        {
            position[i] += direction[i] * velocity * frame.Delta;
            if (position[i] <= 0) { position[i] = 0; direction[i] = 1; }
            if (position[i] >= Leds - 1) { position[i] = Leds - 1; direction[i] = -1; }
        }
        // Balls heading into each other bounce apart.
        for (int i = 0; i < position.Length; i++)
        {
            for (int j = i + 1; j < position.Length; j++)
            {
                if (MathF.Abs(position[i] - position[j]) < 1 && direction[i] != direction[j]
                    && (position[i] - position[j]) * (direction[i] - direction[j]) < 0)
                {
                    (direction[i], direction[j]) = (direction[j], direction[i]);
                }
            }
        }
        var leds = new Color[Leds];
        for (int i = 0; i < position.Length; i++)
        {
            for (int led = 0; led < Leds; led++)
            {
                float glow = 1 - MathF.Abs(led - position[i]) / 1.5f; // a soft ball about three LEDs wide
                if (glow > 0) leds[led] = Brighter(leds[led], Scale(colors[i], glow));
            }
        }
        return leds;
    }
}

/// <summary>
/// Typing ripples: each key press sends a pulse of light outwards from the matching spot on the bar. The colors take
/// turns, one per key press.
/// </summary>
internal sealed class TypingScene(IReadOnlyList<Color> colors, int speed) : LightBarScene
{
    readonly List<(float Origin, double Start, Color Color)> ripples = [];
    int nextColor;

    public override EffectInputs Inputs => EffectInputs.Keys;

    public override Color[] Render(EffectFrame frame)
    {
        foreach (float key in frame.Keys)
        {
            ripples.Add((key * (Leds - 1), frame.Time, colors[nextColor % colors.Count]));
            nextColor++;
        }
        float spread = Rate(speed, 10, 30);   // LEDs per second
        float lifetime = Rate(speed, 1.2f, 0.4f);
        ripples.RemoveAll(r => frame.Time - r.Start > lifetime);
        var leds = new Color[Leds];
        foreach (var (origin, start, color) in ripples)
        {
            float age = (float)(frame.Time - start);
            float radius = age * spread, fade = 1 - age / lifetime;
            for (int led = 0; led < Leds; led++)
            {
                float edge = 1 - MathF.Abs(MathF.Abs(led - origin) - radius) / 1.5f; // a ring about three LEDs wide
                if (edge > 0) leds[led] = Brighter(leds[led], Scale(color, edge * fade));
            }
        }
        return leds;
    }
}

/// <summary>
/// The battery level as a bar from the left, green when full through yellow to red when low. While charging, a
/// pulse runs along it.
/// </summary>
internal sealed class BatteryScene : LightBarScene
{
    static readonly Color Low = Color.FromArgb(255, 0, 0), Middle = Color.FromArgb(255, 160, 0), Full = Color.FromArgb(0, 255, 0);

    public override EffectInputs Inputs => EffectInputs.Battery;

    public override Color[] Render(EffectFrame frame)
    {
        var leds = new Color[Leds];
        if (frame.Status.BatteryPercent is not int percent)
        {
            leds[Center] = Scale(Color.White, 0.2f); // no battery reading
            return leds;
        }
        float charge = percent / 100f;
        var color = charge < 0.5f ? Lerp(Low, Middle, (charge - 0.1f) / 0.4f) : Lerp(Middle, Full, (charge - 0.5f) / 0.4f);
        float filled = Math.Max(1, charge * Leds);
        float pulse = frame.Status.Charging ? (float)(frame.Time * 8 % (filled + 4)) - 2 : float.NaN;
        for (int led = 0; led < Leds; led++)
        {
            float inside = Math.Clamp(filled - led, 0, 1); // the last LED can be partly lit
            leds[led] = inside > 0 ? Scale(color, 0.35f + 0.65f * inside) : Color.Empty;
            if (!float.IsNaN(pulse) && inside > 0)
            {
                float glow = 1 - MathF.Abs(led - pulse) / 2f;
                if (glow > 0) leds[led] = Lerp(leds[led], Color.White, glow * 0.7f);
            }
        }
        return leds;
    }
}

/// <summary>
/// A gauge growing from the middle: CPU load, or CPU or GPU temperature. It turns from blue through green and yellow
/// to red as the value rises, and moves smoothly between the once-a-second readings.
/// </summary>
internal sealed class GaugeScene(EffectInputs input) : LightBarScene
{
    static readonly Color[] Scale5 =
    [
        Color.FromArgb(0, 80, 255), Color.FromArgb(0, 255, 120), Color.FromArgb(255, 220, 0), Color.FromArgb(255, 90, 0),
        Color.FromArgb(255, 0, 0),
    ];

    float shown = -1;

    public override EffectInputs Inputs => input;

    /// <summary>The reading as 0-1, or null if there isn't one (e.g. the GPU is switched off).</summary>
    float? Value(SystemStatus status) => input switch
    {
        EffectInputs.CpuLoad => status.CpuLoad,
        EffectInputs.CpuTemperature => status.CpuTemperature is float c ? (c - 35) / 60 : null,  // 35-95 °C
        EffectInputs.GpuTemperature => status.GpuTemperature is float g ? (g - 35) / 55 : null,  // 35-90 °C
        _ => null,
    };

    public override Color[] Render(EffectFrame frame)
    {
        var leds = new Color[Leds];
        if (Value(frame.Status) is not float value)
        {
            leds[Center] = Scale(Color.White, 0.15f); // no reading right now
            shown = -1;
            return leds;
        }
        value = Math.Clamp(value, 0, 1);
        shown = shown < 0 ? value : shown + Math.Clamp(value - shown, -frame.Delta, frame.Delta);
        var color = Gradient(Scale5, shown);
        float reach = shown * Center + 1; // LEDs on each side, counting the middle one
        for (int d = 0; d <= Center; d++)
        {
            float lit = Math.Clamp(reach - d, 0, 1);
            if (lit <= 0) break;
            leds[Center - d] = leds[Center + d] = Scale(color, lit);
        }
        return leds;
    }
}

/// <summary>A short animation drawn over the light bar for an indicator, such as Caps Lock turning on.</summary>
internal sealed class LightBarOverlay
{
    public enum Style
    {
        /// <summary>Light fills from the middle outwards, then fades: something turned on.</summary>
        Open,
        /// <summary>The bar lights up, then shrinks to the middle: something turned off.</summary>
        Close,
        /// <summary>The whole bar blinks: an alert.</summary>
        Blink,
    }

    const int Leds = IteLightingDevice.LightBarLeds;
    const int Center = Leds / 2;

    readonly Color color;
    readonly Style style;
    readonly double duration;
    double start = double.NaN;

    public LightBarOverlay(Color color, Style style, double seconds = 0.9)
    {
        this.color = color;
        this.style = style;
        duration = seconds;
    }

    /// <summary>Draws over <paramref name="leds"/>; returns false once finished.</summary>
    public bool Apply(Color[] leds, double time)
    {
        if (double.IsNaN(start)) start = time;
        float t = (float)((time - start) / duration);
        if (t >= 1) return false;
        switch (style)
        {
            case Style.Open:
            {
                float reach = Math.Min(1, t * 3) * (Center + 1), fade = t < 0.5f ? 1 : 1 - (t - 0.5f) * 2;
                for (int d = 0; d < reach && d <= Center; d++) leds[Center - d] = leds[Center + d] = Scale(color, fade);
                break;
            }
            case Style.Close:
            {
                float reach = (1 - t) * (Center + 1);
                for (int i = 0; i < Leds; i++) leds[i] = Math.Abs(i - Center) < reach ? Scale(color, 1 - t * 0.5f) : Color.Empty;
                break;
            }
            case Style.Blink:
            {
                bool on = (int)(t * duration * 4) % 2 == 0; // on and off every quarter second: two blinks a second
                for (int i = 0; i < Leds; i++) leds[i] = on ? color : Color.Empty;
                break;
            }
        }
        return true;
    }

    static Color Scale(Color c, float amount)
    {
        amount = Math.Clamp(amount, 0, 1);
        return Color.FromArgb((int)(c.R * amount), (int)(c.G * amount), (int)(c.B * amount));
    }
}
