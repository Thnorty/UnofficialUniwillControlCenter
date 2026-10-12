using System.Runtime.InteropServices;

namespace MonsterKeyboardRGB;

/// <summary>
/// Control Center's four-zone Music effect (LM_ITE_RGB.DLL_SetMusicMode, MUSICTYPE.BRIGHTNESS): the keyboard shows
/// the static zone colors and the brightness follows the peak level of the default audio output.
/// </summary>
internal sealed class MusicVisualizer : IDisposable
{
    const int TickMilliseconds = 25;
    // How much of the level is kept each tick, by speed 0-4: faster speeds fall back sooner.
    static readonly float[] Decay = [0.93f, 0.89f, 0.84f, 0.77f, 0.68f];

    readonly Lock gate = new();
    readonly AutoResetEvent wake = new(false);
    readonly Thread thread;
    FourZoneKeyboard? keyboard; // null while paused
    byte peakLight;
    int speed;
    volatile bool stopping;

    public MusicVisualizer()
    {
        thread = new Thread(Run) { IsBackground = true, Name = "Music effect" };
        thread.Start();
    }

    /// <summary>Starts or updates the effect. <paramref name="peakLight"/> is the firmware brightness (0-50) at full volume.</summary>
    public void Play(FourZoneKeyboard keyboard, byte peakLight, int speed)
    {
        lock (gate)
        {
            this.keyboard = keyboard;
            this.peakLight = peakLight;
            this.speed = Math.Clamp(speed, 0, 4);
        }
        wake.Set();
    }

    /// <summary>Stops sending. Once this returns the keyboard won't receive another brightness change from here.</summary>
    public void Pause()
    {
        lock (gate) keyboard = null;
    }

    public bool IsPlaying
    {
        get { lock (gate) return keyboard is not null; }
    }

    void Run()
    {
        AudioMeter? meter = null;
        long meterOpened = 0;
        float ceiling = 0, level = 0;
        int lastSent = -1;
        while (!stopping)
        {
            lock (gate)
            {
                if (keyboard is { } target)
                {
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
                        float shaped = MathF.Pow(Math.Clamp(peak / ceiling, 0, 1), 1.6f);
                        level = Math.Max(shaped, level * Decay[speed]);
                        int light = (int)MathF.Round(peakLight * level);
                        if (light != lastSent)
                        {
                            target.SetRawLight((byte)light);
                            lastSent = light;
                        }
                    }
                    catch (Exception)
                    {
                        // Audio device or keyboard went away; try again on the next tick.
                        meter?.Dispose();
                        meter = null;
                        lastSent = -1;
                    }
                }
                else
                {
                    meter?.Dispose();
                    meter = null;
                    lastSent = -1;
                    level = 0;
                }
            }
            if (IsPlaying) Thread.Sleep(TickMilliseconds);
            else wake.WaitOne();
        }
        meter?.Dispose();
    }

    public void Dispose()
    {
        Pause();
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
