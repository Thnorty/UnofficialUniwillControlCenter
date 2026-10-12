using System.Runtime.InteropServices;

namespace MonsterKeyboardRGB;

/// <summary>
/// The Windows Core Audio interfaces this app uses: the output peak meter (Music), loopback capture of what's playing
/// (Spectrum) and the microphone's mute state (indicators). Only the leading methods of each interface are declared;
/// the vtable order must match the SDK headers.
/// </summary>
internal static class CoreAudio
{
    public const int RenderFlow = 0;   // eRender
    public const int ConsoleRole = 0;  // eConsole
    public const int MultimediaRole = 1;
    const int ClsctxAll = 0x17;

    /// <summary>Activates an interface on the default device, or returns null if there is no such device.</summary>
    public static T? ActivateDefault<T>(int flow, int role) where T : class
    {
        var enumerator = (IMMDeviceEnumerator)new MMDeviceEnumerator();
        try
        {
            if (enumerator.GetDefaultAudioEndpoint(flow, role, out var device) != 0 || device is null) return null;
            try
            {
                var iid = typeof(T).GUID;
                return device.Activate(ref iid, ClsctxAll, IntPtr.Zero, out object instance) == 0 ? (T)instance : null;
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

    public static void Release(object? com)
    {
        if (com is not null) Marshal.ReleaseComObject(com);
    }

    [ComImport, Guid("BCDE0395-E52F-467C-8E3D-C4579291692E")]
    class MMDeviceEnumerator { }

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
    public interface IAudioMeterInformation
    {
        [PreserveSig] int GetPeakValue(out float peak);
    }

    [ComImport, Guid("1CB9AD4C-DBFA-4C32-B178-C2F568A703B2"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    public interface IAudioClient
    {
        [PreserveSig] int Initialize(int shareMode, uint streamFlags, long bufferDuration, long periodicity, IntPtr format, IntPtr sessionGuid);
        [PreserveSig] int GetBufferSize(out uint frames);
        [PreserveSig] int GetStreamLatency(out long latency);
        [PreserveSig] int GetCurrentPadding(out uint frames);
        [PreserveSig] int IsFormatSupported(int shareMode, IntPtr format, out IntPtr closestMatch);
        [PreserveSig] int GetMixFormat(out IntPtr format);
        [PreserveSig] int GetDevicePeriod(out long defaultPeriod, out long minimumPeriod);
        [PreserveSig] int Start();
        [PreserveSig] int Stop();
        [PreserveSig] int Reset();
        [PreserveSig] int SetEventHandle(IntPtr handle);
        [PreserveSig] int GetService(ref Guid iid, [MarshalAs(UnmanagedType.IUnknown)] out object service);
    }

    [ComImport, Guid("C8ADBD64-E71E-48A0-A4DE-185C395CD317"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    public interface IAudioCaptureClient
    {
        [PreserveSig] int GetBuffer(out IntPtr data, out uint frames, out uint flags, out ulong devicePosition, out ulong qpcPosition);
        [PreserveSig] int ReleaseBuffer(uint frames);
        [PreserveSig] int GetNextPacketSize(out uint frames);
    }
}

/// <summary>The peak meter of the default playback device.</summary>
internal sealed class AudioMeter : IDisposable
{
    CoreAudio.IAudioMeterInformation? meter;

    AudioMeter(CoreAudio.IAudioMeterInformation meter) => this.meter = meter;

    public static AudioMeter? OpenDefaultOutput() =>
        CoreAudio.ActivateDefault<CoreAudio.IAudioMeterInformation>(CoreAudio.RenderFlow, CoreAudio.MultimediaRole) is { } meter
            ? new AudioMeter(meter)
            : null;

    public float ReadPeak()
    {
        if (meter is null) return 0;
        Marshal.ThrowExceptionForHR(meter.GetPeakValue(out float peak));
        return peak;
    }

    public void Dispose()
    {
        CoreAudio.Release(meter);
        meter = null;
    }
}

/// <summary>
/// Records what the default playback device is playing (WASAPI loopback), mixed down to mono, for the spectrum.
/// Read it regularly with <see cref="ReadInto"/>; nothing is buffered when nothing plays.
/// </summary>
internal sealed class LoopbackCapture : IDisposable
{
    const int SharedMode = 0;
    const uint LoopbackFlag = 0x00020000;       // AUDCLNT_STREAMFLAGS_LOOPBACK
    const uint SilentFlag = 0x2;                // AUDCLNT_BUFFERFLAGS_SILENT
    const long BufferDuration = 10_000_000;     // 1 s, in 100 ns units
    const ushort FormatPcm = 1, FormatFloat = 3, FormatExtensible = 0xFFFE;
    static readonly Guid FloatSubFormat = new("00000003-0000-0010-8000-00aa00389b71");

    CoreAudio.IAudioClient? client;
    CoreAudio.IAudioCaptureClient? capture;
    readonly int channels;
    readonly int bitsPerSample;
    readonly bool isFloat;

    public int SampleRate { get; }

    LoopbackCapture(CoreAudio.IAudioClient client, CoreAudio.IAudioCaptureClient capture, int sampleRate, int channels, int bitsPerSample, bool isFloat)
    {
        this.client = client;
        this.capture = capture;
        SampleRate = sampleRate;
        this.channels = channels;
        this.bitsPerSample = bitsPerSample;
        this.isFloat = isFloat;
    }

    public static LoopbackCapture? OpenDefaultOutput()
    {
        var client = CoreAudio.ActivateDefault<CoreAudio.IAudioClient>(CoreAudio.RenderFlow, CoreAudio.MultimediaRole);
        if (client is null) return null;
        IntPtr format = IntPtr.Zero;
        try
        {
            Marshal.ThrowExceptionForHR(client.GetMixFormat(out format));
            // WAVEFORMATEX: tag, channels, rate, bytes/s, block align, bits; WAVEFORMATEXTENSIBLE adds the subformat at 24.
            ushort tag = (ushort)Marshal.ReadInt16(format, 0);
            int channels = Marshal.ReadInt16(format, 2);
            int rate = Marshal.ReadInt32(format, 4);
            int bits = Marshal.ReadInt16(format, 14);
            bool isFloat = tag == FormatFloat
                || (tag == FormatExtensible && Marshal.PtrToStructure<Guid>(format + 24) == FloatSubFormat);
            if (!(isFloat && bits == 32) && !(!isFloat && (tag == FormatPcm || tag == FormatExtensible) && bits is 16 or 32))
                return Fail(client);
            Marshal.ThrowExceptionForHR(client.Initialize(SharedMode, LoopbackFlag, BufferDuration, 0, format, IntPtr.Zero));
            var iid = typeof(CoreAudio.IAudioCaptureClient).GUID;
            Marshal.ThrowExceptionForHR(client.GetService(ref iid, out object service));
            Marshal.ThrowExceptionForHR(client.Start());
            return new LoopbackCapture(client, (CoreAudio.IAudioCaptureClient)service, rate, channels, bits, isFloat);
        }
        catch (Exception)
        {
            return Fail(client);
        }
        finally
        {
            if (format != IntPtr.Zero) Marshal.FreeCoTaskMem(format);
        }

        static LoopbackCapture? Fail(CoreAudio.IAudioClient client)
        {
            CoreAudio.Release(client);
            return null;
        }
    }

    /// <summary>Appends everything captured since the last call to <paramref name="samples"/> as mono floats.</summary>
    public void ReadInto(SampleHistory samples)
    {
        if (capture is null) return;
        while (true)
        {
            Marshal.ThrowExceptionForHR(capture.GetNextPacketSize(out uint packet));
            if (packet == 0) return;
            Marshal.ThrowExceptionForHR(capture.GetBuffer(out IntPtr data, out uint frames, out uint flags, out _, out _));
            try
            {
                int bytesPerSample = bitsPerSample / 8;
                for (int f = 0; f < frames; f++)
                {
                    float sum = 0;
                    if ((flags & SilentFlag) == 0)
                    {
                        for (int c = 0; c < channels; c++)
                        {
                            int offset = (f * channels + c) * bytesPerSample;
                            sum += isFloat ? BitConverter.Int32BitsToSingle(Marshal.ReadInt32(data, offset))
                                : bitsPerSample == 16 ? Marshal.ReadInt16(data, offset) / 32768f
                                : Marshal.ReadInt32(data, offset) / 2147483648f;
                        }
                    }
                    samples.Add(sum / channels);
                }
            }
            finally
            {
                capture.ReleaseBuffer(frames);
            }
        }
    }

    public void Dispose()
    {
        try { client?.Stop(); } catch (Exception) { }
        CoreAudio.Release(capture);
        CoreAudio.Release(client);
        capture = null;
        client = null;
    }
}

/// <summary>The most recent audio samples, oldest first.</summary>
internal sealed class SampleHistory(int length)
{
    readonly float[] ring = new float[length];
    int next;

    public int Length => ring.Length;

    public void Add(float sample)
    {
        ring[next] = sample;
        next = (next + 1) % ring.Length;
    }

    public void Clear() => Array.Clear(ring);

    public void CopyTo(float[] destination)
    {
        for (int i = 0; i < ring.Length; i++) destination[i] = ring[(next + i) % ring.Length];
    }
}
