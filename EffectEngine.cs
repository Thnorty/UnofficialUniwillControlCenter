using System.Collections.Concurrent;

namespace MonsterKeyboardRGB;

/// <summary>What a software effect reads each frame; the engine only gathers what some effect needs.</summary>
[Flags]
internal enum EffectInputs
{
    None = 0,
    Loudness = 1,
    Spectrum = 2,
    Keys = 4,
    Battery = 8,
    CpuLoad = 16,
    CpuTemperature = 32,
    GpuTemperature = 64,
    Status = Battery | CpuLoad | CpuTemperature | GpuTemperature,
}

/// <summary>A software effect running on a device, ticked by the <see cref="EffectEngine"/> about 40 times a second.</summary>
internal interface IEffectTarget
{
    EffectInputs Inputs { get; }

    void Tick(EffectFrame frame);
}

/// <summary>The inputs of one engine tick.</summary>
internal sealed class EffectFrame
{
    /// <summary>Seconds since the engine started.</summary>
    public double Time { get; set; }

    /// <summary>Seconds since the previous tick.</summary>
    public float Delta { get; set; }

    /// <summary>How loud the output is, 0-1, scaled to the loudest recent sound.</summary>
    public float Loudness { get; set; }

    public AudioSpectrum Spectrum { get; } = new();

    /// <summary>Keys pressed since the previous tick, as positions across the keyboard from 0 (left) to 1 (right).</summary>
    public List<float> Keys { get; } = [];

    public SystemStatus Status { get; } = new();
}

/// <summary>
/// Runs the software effects (Music, Spectrum, Star Spark, typing, status meters, …) on one background thread, and
/// gathers the audio, key and system inputs they need. Each owner (a lighting panel) has at most one target.
/// </summary>
internal sealed class EffectEngine : IDisposable
{
    const int TickMilliseconds = 25;
    const int SpectrumSize = 2048;

    readonly Lock gate = new();
    readonly AutoResetEvent wake = new(false);
    readonly Thread thread;
    readonly Dictionary<object, IEffectTarget> targets = [];
    readonly ConcurrentQueue<float> keyQueue = new();
    readonly EffectFrame frame = new();
    readonly SampleHistory samples = new(SpectrumSize);
    volatile bool stopping;
    volatile EffectInputs inputsInUse;

    public EffectEngine()
    {
        thread = new Thread(Run) { IsBackground = true, Name = "Lighting effects" };
        thread.Start();
    }

    /// <summary>Starts <paramref name="target"/>, replacing what <paramref name="owner"/> ran before.</summary>
    public void Play(object owner, IEffectTarget target)
    {
        lock (gate)
        {
            targets[owner] = target;
            inputsInUse = CombinedInputs();
        }
        wake.Set();
    }

    /// <summary>Stops <paramref name="owner"/>'s target. Once this returns, it won't be ticked again.</summary>
    public void Pause(object owner)
    {
        lock (gate)
        {
            targets.Remove(owner);
            inputsInUse = CombinedInputs();
        }
    }

    public bool IsPlaying(object owner)
    {
        lock (gate) return targets.ContainsKey(owner);
    }

    public bool IsPlaying(object owner, IEffectTarget target)
    {
        lock (gate) return targets.TryGetValue(owner, out var current) && current == target;
    }

    /// <summary>The target <paramref name="owner"/> is running, if any.</summary>
    public IEffectTarget? Current(object owner)
    {
        lock (gate) return targets.GetValueOrDefault(owner);
    }

    /// <summary>Whether a running effect reacts to typing, so the keyboard hook is worth having.</summary>
    public bool NeedsKeys => inputsInUse.HasFlag(EffectInputs.Keys);

    /// <summary>Reports a key press at a position across the keyboard (0 = left, 1 = right).</summary>
    public void KeyPressed(float position)
    {
        if (NeedsKeys) keyQueue.Enqueue(position);
    }

    EffectInputs CombinedInputs() => targets.Values.Aggregate(EffectInputs.None, (all, t) => all | t.Inputs);

    void Run()
    {
        var inputs = new AudioInputs();
        long start = Environment.TickCount64, previous = start, lastStatus = 0;
        try
        {
            while (!stopping)
            {
                var needed = inputsInUse;
                long now = Environment.TickCount64;
                frame.Time = (now - start) / 1000.0;
                frame.Delta = Math.Min(0.25f, (now - previous) / 1000f);
                previous = now;

                // Gather inputs outside the lock: audio and WMI calls can take a while.
                inputs.Update(needed, frame, samples);
                frame.Keys.Clear();
                while (keyQueue.TryDequeue(out float key)) frame.Keys.Add(key);
                if ((needed & EffectInputs.Status) != 0 && now - lastStatus >= 1000)
                {
                    frame.Status.Update(needed);
                    lastStatus = now;
                }

                lock (gate)
                {
                    foreach (var (owner, target) in targets.ToArray())
                    {
                        try { target.Tick(frame); }
                        catch (Exception) { targets.Remove(owner); } // device gone; its panel reports it on the next send
                    }
                    inputsInUse = CombinedInputs();
                }

                if (inputsInUse != EffectInputs.None || IsAnyPlaying()) Thread.Sleep(TickMilliseconds);
                else
                {
                    inputs.Close();
                    wake.WaitOne();
                    previous = Environment.TickCount64;
                }
            }
        }
        finally
        {
            inputs.Close();
            frame.Status.Dispose();
        }
    }

    bool IsAnyPlaying()
    {
        lock (gate) return targets.Count > 0;
    }

    public void Dispose()
    {
        lock (gate)
        {
            targets.Clear();
            inputsInUse = EffectInputs.None;
        }
        stopping = true;
        wake.Set();
        thread.Join(1000);
        wake.Dispose();
    }

    /// <summary>The audio devices behind <see cref="EffectFrame.Loudness"/> and <see cref="EffectFrame.Spectrum"/>.</summary>
    sealed class AudioInputs
    {
        AudioMeter? meter;
        LoopbackCapture? capture;
        long meterOpened, captureOpened;
        float ceiling;
        readonly float[] window = new float[SpectrumSize];

        public void Update(EffectInputs needed, EffectFrame frame, SampleHistory samples)
        {
            if (needed.HasFlag(EffectInputs.Loudness)) frame.Loudness = ReadLoudness();
            else CloseMeter();
            if (needed.HasFlag(EffectInputs.Spectrum)) ReadSpectrum(frame.Spectrum, samples);
            else CloseCapture();
        }

        float ReadLoudness()
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
                return MathF.Pow(Math.Clamp(peak / ceiling, 0, 1), 1.6f);
            }
            catch (Exception)
            {
                CloseMeter(); // the audio device went away; try again on the next tick
                return 0;
            }
        }

        void ReadSpectrum(AudioSpectrum spectrum, SampleHistory samples)
        {
            try
            {
                if (capture is null || Environment.TickCount64 - captureOpened > 5000)
                {
                    capture?.Dispose();
                    capture = LoopbackCapture.OpenDefaultOutput();
                    captureOpened = Environment.TickCount64;
                    if (capture is null) return;
                }
                capture.ReadInto(samples);
                samples.CopyTo(window);
                spectrum.Analyze(window, capture.SampleRate);
            }
            catch (Exception)
            {
                CloseCapture();
                samples.Clear();
            }
        }

        void CloseMeter()
        {
            meter?.Dispose();
            meter = null;
        }

        void CloseCapture()
        {
            capture?.Dispose();
            capture = null;
        }

        public void Close()
        {
            CloseMeter();
            CloseCapture();
        }
    }
}

/// <summary>The frequency content of the last ~40 ms of audio, with automatic gain.</summary>
internal sealed class AudioSpectrum
{
    const float RangeDb = 45; // a band shows from this far below the loudest recent band up to it

    float[] power = [];
    float binHz;
    float peakDb = -60;

    public void Analyze(float[] samples, int sampleRate)
    {
        int n = samples.Length;
        var re = new float[n];
        var im = new float[n];
        for (int i = 0; i < n; i++) re[i] = samples[i] * (0.5f - 0.5f * MathF.Cos(2 * MathF.PI * i / (n - 1))); // Hann window
        Fft(re, im);
        power = new float[n / 2];
        float loudest = -120;
        for (int i = 1; i < n / 2; i++)
        {
            float amplitude = MathF.Sqrt(re[i] * re[i] + im[i] * im[i]) / (n / 4f);
            power[i] = amplitude * amplitude;
            loudest = Math.Max(loudest, 10 * MathF.Log10(power[i] + 1e-12f));
        }
        binHz = sampleRate / (float)n;
        // Follow the loudest band at once and let the reference fall back slowly (about 6 dB a second).
        peakDb = Math.Max(Math.Max(loudest, peakDb - 0.15f), -60);
    }

    /// <summary>
    /// <paramref name="count"/> bands spaced evenly in pitch between <paramref name="lowHz"/> and
    /// <paramref name="highHz"/>, each 0-1.
    /// </summary>
    public float[] Bands(int count, float lowHz = 40, float highHz = 16000)
    {
        var bands = new float[count];
        if (power.Length == 0) return bands;
        for (int b = 0; b < count; b++)
        {
            float from = lowHz * MathF.Pow(highHz / lowHz, b / (float)count);
            float to = lowHz * MathF.Pow(highHz / lowHz, (b + 1) / (float)count);
            int first = Math.Clamp((int)(from / binHz), 1, power.Length - 1);
            int last = Math.Clamp((int)(to / binHz), first, power.Length - 1);
            float sum = 0;
            for (int i = first; i <= last; i++) sum += power[i];
            float db = 10 * MathF.Log10(sum + 1e-12f);
            bands[b] = Math.Clamp((db - (peakDb - RangeDb)) / RangeDb, 0, 1);
        }
        return bands;
    }

    /// <summary>In-place radix-2 FFT; the length must be a power of two.</summary>
    static void Fft(float[] re, float[] im)
    {
        int n = re.Length;
        for (int i = 1, j = 0; i < n; i++)
        {
            int bit = n >> 1;
            for (; (j & bit) != 0; bit >>= 1) j ^= bit;
            j ^= bit;
            if (i < j)
            {
                (re[i], re[j]) = (re[j], re[i]);
                (im[i], im[j]) = (im[j], im[i]);
            }
        }
        for (int size = 2; size <= n; size <<= 1)
        {
            float angle = -2 * MathF.PI / size;
            float stepRe = MathF.Cos(angle), stepIm = MathF.Sin(angle);
            for (int start = 0; start < n; start += size)
            {
                float wRe = 1, wIm = 0;
                for (int k = 0; k < size / 2; k++)
                {
                    int a = start + k, b = a + size / 2;
                    float tRe = re[b] * wRe - im[b] * wIm;
                    float tIm = re[b] * wIm + im[b] * wRe;
                    re[b] = re[a] - tRe;
                    im[b] = im[a] - tIm;
                    re[a] += tRe;
                    im[a] += tIm;
                    (wRe, wIm) = (wRe * stepRe - wIm * stepIm, wRe * stepIm + wIm * stepRe);
                }
            }
        }
    }
}
