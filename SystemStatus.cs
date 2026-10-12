using System.Management;
using System.Runtime.InteropServices;

namespace MonsterKeyboardRGB;

/// <summary>
/// Battery, CPU load and temperatures for the status effects, read about once a second on the effects thread.
/// Each reading is null when this machine can't provide it right now.
/// </summary>
internal sealed class SystemStatus : IDisposable
{
    long lastIdle, lastKernel, lastUser;
    ManagementObjectSearcher? thermalZones;
    bool nvmlStarted, nvmlMissing;

    /// <summary>Battery charge, 0-100.</summary>
    public int? BatteryPercent { get; private set; }

    public bool Charging { get; private set; }

    /// <summary>CPU use since the previous reading, 0-1.</summary>
    public float? CpuLoad { get; private set; }

    /// <summary>
    /// The hottest ACPI thermal zone in °C. Control Center reads the same Windows counter ("Thermal Zone Information").
    /// </summary>
    public float? CpuTemperature { get; private set; }

    /// <summary>The NVIDIA GPU's temperature in °C; null while the GPU is switched off or absent.</summary>
    public float? GpuTemperature { get; private set; }

    public void Update(EffectInputs needed)
    {
        if (needed.HasFlag(EffectInputs.Battery)) ReadBattery();
        if (needed.HasFlag(EffectInputs.CpuLoad)) ReadCpuLoad();
        if (needed.HasFlag(EffectInputs.CpuTemperature)) ReadCpuTemperature();
        if (needed.HasFlag(EffectInputs.GpuTemperature)) ReadGpuTemperature();
    }

    void ReadBattery()
    {
        var power = SystemInformation.PowerStatus;
        bool hasBattery = !power.BatteryChargeStatus.HasFlag(BatteryChargeStatus.NoSystemBattery)
            && !power.BatteryChargeStatus.HasFlag(BatteryChargeStatus.Unknown);
        BatteryPercent = hasBattery ? (int)Math.Round(Math.Clamp(power.BatteryLifePercent, 0, 1) * 100) : null;
        Charging = hasBattery && power.PowerLineStatus == PowerLineStatus.Online && power.BatteryLifePercent < 1;
    }

    void ReadCpuLoad()
    {
        if (!GetSystemTimes(out long idle, out long kernel, out long user)) return;
        long idleDelta = idle - lastIdle, total = kernel - lastKernel + (user - lastUser); // kernel time includes idle
        if (lastKernel != 0 && total > 0) CpuLoad = Math.Clamp(1f - idleDelta / (float)total, 0, 1);
        (lastIdle, lastKernel, lastUser) = (idle, kernel, user);
    }

    void ReadCpuTemperature()
    {
        try
        {
            thermalZones ??= new ManagementObjectSearcher(@"root\CIMV2",
                "SELECT HighPrecisionTemperature FROM Win32_PerfFormattedData_Counters_ThermalZoneInformation");
            float? hottest = null;
            using var results = thermalZones.Get();
            foreach (var zone in results)
            {
                using (zone)
                {
                    // Tenths of a kelvin.
                    float celsius = Convert.ToSingle(zone["HighPrecisionTemperature"]) / 10f - 273.15f;
                    if (celsius > 0 && celsius < 150 && (hottest is null || celsius > hottest)) hottest = celsius;
                }
            }
            CpuTemperature = hottest;
        }
        catch (Exception)
        {
            CpuTemperature = null;
        }
    }

    void ReadGpuTemperature()
    {
        GpuTemperature = null;
        if (nvmlMissing) return;
        try
        {
            if (!nvmlStarted)
            {
                if (nvmlInit_v2() != 0) return; // no NVIDIA GPU in use right now; try again next time
                nvmlStarted = true;
            }
            if (nvmlDeviceGetHandleByIndex_v2(0, out IntPtr device) == 0
                && nvmlDeviceGetTemperature(device, 0 /* NVML_TEMPERATURE_GPU */, out uint celsius) == 0)
            {
                GpuTemperature = celsius;
            }
        }
        catch (DllNotFoundException)
        {
            nvmlMissing = true; // no NVIDIA driver
        }
        catch (EntryPointNotFoundException)
        {
            nvmlMissing = true;
        }
    }

    public void Dispose()
    {
        thermalZones?.Dispose();
        if (nvmlStarted) nvmlShutdown();
    }

    [DllImport("kernel32.dll")]
    static extern bool GetSystemTimes(out long idle, out long kernel, out long user);

    [DllImport("nvml.dll")] static extern int nvmlInit_v2();
    [DllImport("nvml.dll")] static extern int nvmlShutdown();
    [DllImport("nvml.dll")] static extern int nvmlDeviceGetHandleByIndex_v2(uint index, out IntPtr device);
    [DllImport("nvml.dll")] static extern int nvmlDeviceGetTemperature(IntPtr device, int sensor, out uint temperature);
}
