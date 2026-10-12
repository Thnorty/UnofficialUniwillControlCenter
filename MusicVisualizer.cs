using System.Runtime.InteropServices;

namespace MonsterKeyboardRGB;

/// <summary>
/// Control Center's Music effect (LM_ITE_RGB.DLL_SetMusicMode): follows the peak level of the default audio output.
/// Each device shows the level its own way (see <see cref="IMusicTarget"/>); the keyboard and the light bar can
/// play at the same time.
/// </summary>
internal sealed class MusicVisualizer : IDisposable
{
    const int TickMilliseconds = 25;

    readonly Lock gate = new();
    readonly AutoResetEvent wake = new(false);
    readonly Thread thread;
    readonly Dictionary<object, IMusicTarget> targets = [];
    volatile bool stopping;

    public MusicVisualizer()
    {
        thread = new Thread(Run) { IsBackground = true, Name = "Music effect" };
        thread.Start();
    }

    /// <summary>Starts showing the music on <paramref name="target"/>, replacing what <paramref name="owner"/> played before.</summary>
    public void Play(object owner, IMusicTarget target)
    {
        lock (gate) targets[owner] = target;
        wake.Set();
    }

    /// <summary>Stops <paramref name="owner"/>'s target. Once this returns, it won't be sent anything more from here.</summary>
    public void Pause(object owner)
    {
        lock (gate) targets.Remove(owner);
    }

    public bool IsPlaying(object owner)
    {
        lock (gate) return targets.ContainsKey(owner);
    }

    bool AnyPlaying
    {
        get { lock (gate) return targets.Count > 0; }
    }

    void Run()
    {
        AudioMeter? meter = null;
        long meterOpened = 0;
        float ceiling = 0;
        while (!stopping)
        {
            lock (gate)
            {
                if (targets.Count > 0)
                {
                    float level = 0;
                    try
                    {
                        // Reopen now and then so a change of default output device is picked up.
                        if (meter is null || Environment.TickCount64 - meterOpened > 3000)
                        {
                            meter?.Dispose();
                            meter = AudioMeter.OpenDefaultOutput();
                            meterOpened = Environment.TickCount64;
                        }
                        float peak = meter?.ReadPeak() ?? 0;
                        // Automatic gain like the service's EqualMachine: scale to the loudest recent peak.
                        ceiling = Math.Max(Math.Max(peak, ceiling * 0.997f), 0.02f);
                        level = MathF.Pow(Math.Clamp(peak / ceiling, 0, 1), 1.6f);
                    }
                    catch (Exception)
                    {
                        meter?.Dispose(); // the audio device went away; try again on the next tick
                        meter = null;
                    }
                    foreach (var (owner, target) in targets.ToArray())
                    {
                        try { target.Show(level); }
                        catch (Exception) { targets.Remove(owner); } // device gone; its panel reports it on the next send
                    }
                }
                else
                {
                    meter?.Dispose();
                    meter = null;
                }
            }
            if (AnyPlaying) Thread.Sleep(TickMilliseconds);
            else wake.WaitOne();
        }
        meter?.Dispose();
    }

    public void Dispose()
    {
        lock (gate) targets.Clear();
        stopping = true;
        wake.Set();
        thread.Join(1000);
        wake.Dispose();
    }

    /// <summary>The peak meter of the default playback device (Core Audio IAudioMeterInformation).</summary>
    sealed class AudioMeter : IDisposable
    {
        const int RenderFlow = 0;      // eRender
        const int MultimediaRole = 1;  // eMultimedia
        const int ClsctxAll = 0x17;
        static readonly Guid MeterInterface = typeof(IAudioMeterInformation).GUID;

        IAudioMeterInformation? meter;

        AudioMeter(IAudioMeterInformation meter) => this.meter = meter;

        public static AudioMeter? OpenDefaultOutput()
        {
            var enumerator = (IMMDeviceEnumerator)new MMDeviceEnumerator();
            try
            {
                if (enumerator.GetDefaultAudioEndpoint(RenderFlow, MultimediaRole, out var device) != 0 || device is null) return null;
                try
                {
                    var iid = MeterInterface;
                    if (device.Activate(ref iid, ClsctxAll, IntPtr.Zero, out object instance) != 0) return null;
                    return new AudioMeter((IAudioMeterInformation)instance);
                }
                finally
                {
                    Marshal.ReleaseComObject(device);
                }
            }
            finally
            {
                Marshal.ReleaseComObject(enumerator);
            }
        }

        public float ReadPeak()
        {
            if (meter is null) return 0;
            Marshal.ThrowExceptionForHR(meter.GetPeakValue(out float peak));
            return peak;
        }

        public void Dispose()
        {
            if (meter is not null) Marshal.ReleaseComObject(meter);
            meter = null;
        }

        [ComImport, Guid("BCDE0395-E52F-467C-8E3D-C4579291692E")]
        class MMDeviceEnumerator { }

        // Only the leading methods of each interface are declared; the vtable order must match the SDK headers.
        [ComImport, Guid("A95664D2-9614-4F35-A746-DE8DB63617E6"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
        interface IMMDeviceEnumerator
        {
            [PreserveSig] int EnumAudioEndpoints(int dataFlow, int stateMask, out IntPtr devices);
            [PreserveSig] int GetDefaultAudioEndpoint(int dataFlow, int role, out IMMDevice? endpoint);
        }

        [ComImport, Guid("D666063F-1587-4E43-81F1-B948E807363F"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
        interface IMMDevice
        {
            [PreserveSig] int Activate(ref Guid iid, int clsCtx, IntPtr activationParams, [MarshalAs(UnmanagedType.IUnknown)] out object instance);
        }

        [ComImport, Guid("C02216F6-8C67-4B5B-9D00-D008E73E0064"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
        interface IAudioMeterInformation
        {
            [PreserveSig] int GetPeakValue(out float peak);
        }
    }
}

/// <summary>Shows the music level on a device. Called on the music thread every tick.</summary>
internal interface IMusicTarget
{
    /// <param name="loudness">How loud it is, 0-1, scaled to the loudest recent sound.</param>
    void Show(float loudness);
}

/// <summary>Remembers how loud it was a moment ago, falling back at a rate set by the speed slider.</summary>
internal abstract class DecayingMusicTarget(int speed) : IMusicTarget
{
    // How much of the level is kept each tick, by speed 0-4: faster speeds fall back sooner.
    static readonly float[] Decay = [0.93f, 0.89f, 0.84f, 0.77f, 0.68f];

    readonly float decay = Decay[Math.Clamp(speed, 0, 4)];
    float level;

    public void Show(float loudness)
    {
        level = Math.Max(loudness, level * decay);
        ShowLevel(level);
    }

    protected abstract void ShowLevel(float level);
}

/// <summary>
/// The keyboard's Music (MUSICTYPE.BRIGHTNESS): the static zone colors, with the brightness following the sound.
/// </summary>
internal sealed class KeyboardMusic(IteLightingDevice keyboard, byte peakLight, int speed) : DecayingMusicTarget(speed)
{
    int lastSent = -1;

    protected override void ShowLevel(float level)
    {
        int light = (int)MathF.Round(peakLight * level);
        if (light == lastSent) return;
        keyboard.SetRawLight((byte)light);
        lastSent = light;
    }
}

/// <summary>
/// The light bar's Music (LM_ITE_RGB.AudioData_LightbarEventHandler): a level meter that grows from the middle of the
/// bar outwards, each LED colored by its distance from the middle.
/// </summary>
internal sealed class LightBarMusic(IteLightingDevice lightBar, IReadOnlyList<Color> colors, int speed) : DecayingMusicTarget(speed)
{
    const int Center = IteLightingDevice.LightBarLeds / 2;
    readonly Color[] leds = new Color[IteLightingDevice.LightBarLeds];
    int lastLit = -1;

    protected override void ShowLevel(float level)
    {
        int lit = (int)MathF.Round(level * (Center + 1)); // LEDs lit on each side, counting the middle one
        if (lit == lastLit) return;
        Array.Clear(leds);
        for (int distance = 0; distance < lit; distance++)
        {
            // The service gives the middle two steps the first color, then one color per step outwards.
            var color = colors[Math.Clamp(distance - 1, 0, colors.Count - 1)];
            leds[Center - distance] = color;
            leds[Center + distance] = color;
        }
        lightBar.SetLeds(leds);
        lastLit = lit;
    }
}
