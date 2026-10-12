using System.Text.Json;

namespace MonsterKeyboardRGB;

/// <summary>Last-used lighting, kept in %APPDATA% because the controller can't report colors back.</summary>
internal sealed class AppSettings
{
    static readonly string FilePath = Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData), "MonsterKeyboardRGB", "settings.json");

    // Effect list of earlier versions, which saved the selected index.
    static readonly string[] LegacyEffects = ["static", "breathing", "wave", "wave", "rainbow", "flash", "mix"];

    public int[] ZoneColors { get; set; } = [Color.Red.ToArgb(), Color.Red.ToArgb(), Color.Red.ToArgb(), Color.Red.ToArgb()];
    public string Effect { get; set; } = "static";
    /// <summary>Only read from old settings files.</summary>
    public int EffectIndex { get; set; } = -1;
    /// <summary>Colors of each palette effect (Breathing, Wave, Flash, Mix), by effect key.</summary>
    public Dictionary<string, int[]> Palettes { get; set; } = [];
    public int WaveDirection { get; set; } = 1;
    public int Brightness { get; set; } = 4;
    public int Speed { get; set; } = 2;
    public bool NightLight { get; set; }
    public int NightColor { get; set; } = NightGlow.Colors.Length - 1;
    public int IdleTimeoutSeconds { get; set; }

    public static AppSettings Load()
    {
        try
        {
            var settings = JsonSerializer.Deserialize<AppSettings>(File.ReadAllText(FilePath));
            if (settings is { ZoneColors.Length: 4 })
            {
                if (settings.EffectIndex >= 0 && settings.EffectIndex < LegacyEffects.Length)
                {
                    settings.Effect = LegacyEffects[settings.EffectIndex];
                    if (settings.EffectIndex == 3) settings.WaveDirection = 2; // "Wave (right → left)"
                }
                settings.EffectIndex = -1;
                return settings;
            }
        }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException or JsonException) { }
        return new AppSettings();
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
