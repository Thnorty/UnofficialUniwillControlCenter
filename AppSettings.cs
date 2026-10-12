using System.Text.Json;
using System.Text.Json.Serialization;

namespace MonsterKeyboardRGB;

/// <summary>Last-used settings, kept in %APPDATA% because the controllers can't report colors back.</summary>
internal sealed class AppSettings
{
    static readonly string FilePath = Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData), "MonsterKeyboardRGB", "settings.json");

    // Effect list of the first versions, which saved the selected index.
    static readonly string[] LegacyEffects = ["static", "breathing", "wave", "wave", "rainbow", "flash", "mix"];

    public LightingSettings? Keyboard { get; set; }
    /// <summary>Null until the app has seen the light bar, so the first run can pick up its current state.</summary>
    public LightingSettings? LightBar { get; set; }
    public int IdleTimeoutSeconds { get; set; }
    /// <summary>The light bar flashes when these change.</summary>
    public bool FlashCapsLock { get; set; } = true;
    public bool FlashNumLock { get; set; } = true;
    public bool FlashMicMute { get; set; } = true;

    // Earlier versions kept the keyboard's settings at the top level. Only read, to migrate them.
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)] public int[]? ZoneColors { get; set; }
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)] public string? Effect { get; set; }
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)] public int? EffectIndex { get; set; }
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)] public Dictionary<string, int[]>? Palettes { get; set; }
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)] public int? WaveDirection { get; set; }
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)] public int? Light { get; set; }
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)] public int? Brightness { get; set; }
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)] public int? Speed { get; set; }

    public static AppSettings Load()
    {
        AppSettings settings;
        try
        {
            settings = JsonSerializer.Deserialize<AppSettings>(File.ReadAllText(FilePath)) ?? new AppSettings();
        }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException or JsonException)
        {
            settings = new AppSettings();
        }
        settings.Keyboard ??= settings.MigrateKeyboard();
        settings.ZoneColors = null;
        settings.Effect = null;
        settings.EffectIndex = null;
        settings.Palettes = null;
        settings.WaveDirection = null;
        settings.Light = null;
        settings.Brightness = null;
        settings.Speed = null;
        return settings;
    }

    LightingSettings MigrateKeyboard()
    {
        var keyboard = new LightingSettings();
        if (ZoneColors is { Length: 4 }) keyboard.ZoneColors = ZoneColors;
        if (Effect is not null) keyboard.Effect = Effect;
        if (EffectIndex is int index && index >= 0 && index < LegacyEffects.Length)
        {
            keyboard.Effect = LegacyEffects[index];
            if (index == 3) WaveDirection = 2; // "Wave (right → left)"
        }
        if (Palettes is not null) keyboard.Palettes = Palettes;
        if (WaveDirection is int direction) keyboard.WaveDirection = direction;
        if (Light is int light) keyboard.Light = light;
        else if (Brightness is int level) keyboard.Light = IteLightingDevice.StandardLights[Math.Clamp(level, 0, 4)];
        if (Speed is int speed) keyboard.Speed = speed;
        return keyboard;
    }

    public void Save()
    {
        try
        {
            Directory.CreateDirectory(Path.GetDirectoryName(FilePath)!);
            File.WriteAllText(FilePath, JsonSerializer.Serialize(this));
        }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException) { }
    }
}

/// <summary>The lighting of one device: the keyboard or the light bar.</summary>
internal sealed class LightingSettings
{
    public bool On { get; set; } = true;
    public string Effect { get; set; } = "static";
    /// <summary>Static and Music colors of the keyboard's four zones (ARGB).</summary>
    public int[] ZoneColors { get; set; } = [Color.Red.ToArgb(), Color.Red.ToArgb(), Color.Red.ToArgb(), Color.Red.ToArgb()];
    /// <summary>Colors of each palette effect, by effect key.</summary>
    public Dictionary<string, int[]> Palettes { get; set; } = [];
    public int WaveDirection { get; set; } = 1;
    /// <summary>Firmware brightness, 0-50.</summary>
    public int Light { get; set; } = IteLightingDevice.MaxLight;
    public int Speed { get; set; } = 2;
    /// <summary>The light bar shows the keyboard's effect, colors and speed.</summary>
    public bool Linked { get; set; }
}
