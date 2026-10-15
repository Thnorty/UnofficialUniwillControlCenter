//! The Linux side of the readings, from /proc and /sys: CPU time and memory from /proc, the ACPI thermal zones (or
//! coretemp), the battery and the power adapter from the kernel's power_supply class, and the NVIDIA GPU's
//! temperature through the driver's NVML.

use std::path::Path;

use super::BatteryDetails;

/// What the readers keep between readings.
#[derive(Default)]
pub struct State;

/// The idle (idle + iowait) and total time of the `cpu` line of /proc/stat, in clock ticks.
pub fn parse_cpu_times(proc_stat: &str) -> Option<(u64, u64)> {
    let line = proc_stat.lines().find(|l| l.split_whitespace().next() == Some("cpu"))?;
    let times: Vec<u64> = line.split_whitespace().skip(1).map(|t| t.parse().ok()).collect::<Option<_>>()?;
    if times.len() < 5 {
        return None;
    }
    Some((times[3] + times[4], times.iter().sum()))
}

/// The CPU's idle time and its total time so far, in any unit.
pub fn cpu_times() -> Option<(u64, u64)> {
    parse_cpu_times(&std::fs::read_to_string("/proc/stat").ok()?)
}

/// Memory in use (MemTotal − MemAvailable) and in total, in bytes, from /proc/meminfo.
pub fn mem_used_total(meminfo: &str) -> Option<(u64, u64)> {
    let field = |name: &str| -> Option<u64> {
        let line = meminfo.lines().find(|l| l.split(':').next() == Some(name))?;
        line.split(':').nth(1)?.split_whitespace().next()?.parse::<u64>().ok().map(|kb| kb * 1024)
    };
    let (total, available) = (field("MemTotal")?, field("MemAvailable")?);
    Some((total.saturating_sub(available), total))
}

/// Physical memory in use and in total, in bytes.
pub fn memory() -> Option<(u64, u64)> {
    mem_used_total(&std::fs::read_to_string("/proc/meminfo").ok()?)
}

fn read(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

fn number(path: &Path) -> Option<i64> {
    read(path)?.parse().ok()
}

/// The entries of a sysfs class folder.
fn entries(folder: &Path) -> Vec<std::path::PathBuf> {
    let mut entries: Vec<_> = std::fs::read_dir(folder).map(|d| d.flatten().map(|e| e.path()).collect()).unwrap_or_default();
    entries.sort();
    entries
}

/// The hottest ACPI thermal zone in °C, the reading Control Center uses; coretemp's package temperature when there
/// are no ACPI zones. `root` is /sys.
pub fn hottest_acpitz(root: &Path) -> Option<f32> {
    let zones = entries(&root.join("class/thermal"));
    let acpitz = zones
        .iter()
        .filter(|zone| read(&zone.join("type")).as_deref() == Some("acpitz"))
        .filter_map(|zone| number(&zone.join("temp")))
        .max();
    if let Some(millidegrees) = acpitz {
        return Some(millidegrees as f32 / 1000.0);
    }
    entries(&root.join("class/hwmon"))
        .iter()
        .find(|hwmon| read(&hwmon.join("name")).as_deref() == Some("coretemp"))
        .and_then(|hwmon| number(&hwmon.join("temp1_input")))
        .map(|millidegrees| millidegrees as f32 / 1000.0)
}

/// The hottest ACPI thermal zone in °C.
pub fn cpu_temperature(_state: &mut State) -> Option<f32> {
    hottest_acpitz(Path::new("/sys"))
}

/// The power supplies of a type ("Battery", "Mains").
fn supplies(root: &Path, kind: &str) -> Vec<std::path::PathBuf> {
    entries(&root.join("class/power_supply")).into_iter().filter(|s| read(&s.join("type")).as_deref() == Some(kind)).collect()
}

/// The battery's charge (None without one), whether it's charging, and whether the power adapter is in. `root` is
/// /sys.
pub fn battery(root: &Path) -> (Option<u8>, bool, bool) {
    let battery = supplies(root, "Battery").into_iter().next();
    let percent = battery.as_ref().and_then(|b| number(&b.join("capacity"))).map(|p| p.clamp(0, 100) as u8);
    let charging = battery.as_ref().is_some_and(|b| read(&b.join("status")).as_deref() == Some("Charging"));
    let on_ac = supplies(root, "Mains").iter().any(|m| number(&m.join("online")) == Some(1));
    (percent, charging, on_ac)
}

/// The battery's capacities in mWh, its rate in mW (negative while discharging), voltage in mV and cycles. Drivers
/// report energy (µWh) or charge (µAh, turned into energy at the design's minimum voltage), and power (µW) or current
/// (µA × the voltage now).
pub fn battery_details(root: &Path) -> BatteryDetails {
    let Some(battery) = supplies(root, "Battery").into_iter().next() else { return BatteryDetails::default() };
    let value = |name: &str| number(&battery.join(name)).filter(|&v| v >= 0);
    let minimum_voltage = value("voltage_min_design");
    let energy = |name: &str| -> Option<u32> {
        match value(&format!("energy_{name}")) {
            Some(uwh) => Some((uwh / 1000) as u32),
            None => Some((value(&format!("charge_{name}"))? as i128 * minimum_voltage? as i128 / 1_000_000_000) as u32),
        }
    };
    let voltage_now = value("voltage_now");
    let rate = value("power_now")
        .map(|uw| uw / 1000)
        .or_else(|| Some((value("current_now")? as i128 * voltage_now? as i128 / 1_000_000_000) as i64))
        .map(|mw| if read(&battery.join("status")).as_deref() == Some("Discharging") { -mw } else { mw } as i32);
    BatteryDetails {
        designed: energy("full_design").filter(|&c| c > 0),
        full: energy("full").filter(|&c| c > 0),
        remaining: energy("now"),
        rate,
        voltage: voltage_now.map(|uv| (uv / 1000) as u32).filter(|&v| v > 0),
        cycles: value("cycle_count").map(|c| c as u32).filter(|&c| c > 0),
    }
}

/// The battery's charge (None without one), whether it's charging, and whether the power adapter is in.
pub fn power_now() -> (Option<u8>, bool, bool) {
    battery(Path::new("/sys"))
}

/// Whether the laptop runs on its power adapter.
pub fn on_ac() -> bool {
    power_now().2
}

/// Reads the battery's details.
#[derive(Default)]
pub struct BatteryReader;

impl BatteryReader {
    pub fn read(&mut self) -> BatteryDetails {
        battery_details(Path::new("/sys"))
    }
}

pub use crate::hardware::nvml::Nvml;

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    /// A fake /sys tree in a temporary folder.
    struct Tree(PathBuf);

    impl Tree {
        fn new(name: &str) -> Tree {
            let root = std::env::temp_dir().join(format!("uucc-status-test-{}-{name}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).unwrap();
            Tree(root)
        }

        fn file(&self, path: &str, contents: &str) -> &Tree {
            let path = self.0.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, format!("{contents}\n")).unwrap();
            self
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn cpu_line_parses() {
        let stat = "cpu  100 0 50 800 50 0 0 0 0 0\ncpu0 50 0 25 400 25 0 0 0 0 0\nintr 1 2 3\n";
        assert_eq!(parse_cpu_times(stat), Some((850, 1000)));
        assert_eq!(parse_cpu_times("intr 1 2 3"), None);
    }

    #[test]
    fn memory_uses_available() {
        let meminfo = "MemTotal:       16000000 kB\nMemFree:         1000000 kB\nMemAvailable:    6000000 kB\nBuffers: 1 kB\n";
        assert_eq!(mem_used_total(meminfo), Some((10_000_000 * 1024, 16_000_000 * 1024)));
        assert_eq!(mem_used_total("MemTotal: 5 kB\n"), None);
    }

    #[test]
    fn hottest_acpitz_wins() {
        let tree = Tree::new("acpitz");
        tree.file("class/thermal/thermal_zone0/type", "acpitz").file("class/thermal/thermal_zone0/temp", "52000");
        tree.file("class/thermal/thermal_zone1/type", "x86_pkg_temp").file("class/thermal/thermal_zone1/temp", "90000");
        tree.file("class/thermal/thermal_zone2/type", "acpitz").file("class/thermal/thermal_zone2/temp", "61000");
        assert_eq!(hottest_acpitz(&tree.0), Some(61.0));
    }

    #[test]
    fn coretemp_when_no_acpitz() {
        let tree = Tree::new("coretemp");
        tree.file("class/thermal/thermal_zone0/type", "x86_pkg_temp").file("class/thermal/thermal_zone0/temp", "90000");
        tree.file("class/hwmon/hwmon0/name", "nvme").file("class/hwmon/hwmon0/temp1_input", "40000");
        tree.file("class/hwmon/hwmon3/name", "coretemp").file("class/hwmon/hwmon3/temp1_input", "57000");
        assert_eq!(hottest_acpitz(&tree.0), Some(57.0));
        assert_eq!(hottest_acpitz(&Tree::new("no-sensors").0), None);
    }

    /// This laptop's battery, which reports charge in µAh.
    fn laptop_battery(tree: &Tree, status: &str) {
        let at = |name: &str| format!("class/power_supply/BAT0/{name}");
        tree.file(&at("type"), "Battery").file(&at("status"), status).file(&at("capacity"), "80");
        tree.file(&at("charge_full_design"), "4100000").file(&at("charge_full"), "2500000").file(&at("charge_now"), "2000000");
        tree.file(&at("voltage_min_design"), "11400000").file(&at("voltage_now"), "12000000");
        tree.file(&at("current_now"), "1250000").file(&at("cycle_count"), "0");
        tree.file("class/power_supply/AC/type", "Mains").file("class/power_supply/AC/online", if status == "Discharging" { "0" } else { "1" });
    }

    #[test]
    fn battery_from_charge_and_voltage() {
        let tree = Tree::new("charge");
        laptop_battery(&tree, "Charging");
        let details = battery_details(&tree.0);
        assert_eq!((details.designed, details.full, details.remaining), (Some(46740), Some(28500), Some(22800)));
        assert_eq!(details.rate, Some(15000)); // 1.25 A × 12 V
        assert_eq!(details.voltage, Some(12000));
        assert_eq!(details.cycles, None); // 0 means the driver doesn't count
        assert_eq!(battery(&tree.0), (Some(80), true, true));
    }

    #[test]
    fn discharge_rate_is_negative() {
        let tree = Tree::new("discharge");
        laptop_battery(&tree, "Discharging");
        assert_eq!(battery_details(&tree.0).rate, Some(-15000));
        assert_eq!(battery(&tree.0), (Some(80), false, false));
        // A driver that reports power and energy instead.
        let at = |name: &str| format!("class/power_supply/BAT1/{name}");
        let other = Tree::new("energy");
        other.file(&at("type"), "Battery").file(&at("status"), "Discharging").file(&at("power_now"), "9500000");
        other.file(&at("energy_full_design"), "50000000").file(&at("energy_full"), "45000000").file(&at("energy_now"), "30000000");
        other.file(&at("cycle_count"), "12");
        let details = battery_details(&other.0);
        assert_eq!((details.designed, details.full, details.remaining), (Some(50000), Some(45000), Some(30000)));
        assert_eq!((details.rate, details.cycles), (Some(-9500), Some(12)));
    }
}
