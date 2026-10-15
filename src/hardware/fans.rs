//! The fans: their speeds, and the EC's RAM fan tables ("RamFan 1.5") that Control Center's FanTable_Manager1p5
//! writes for a custom fan curve.
//!
//! A table has 16 steps. The fan runs at a step's duty from the temperature that steps up to it until the
//! temperature falls below the step's down temperature. Unused steps at the end have an up temperature of 255.

use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use super::ec::{Ec, Registers};
use super::hotkeys;
use super::performance::Mode;

pub const STEPS: usize = 16;

const CPU_UP: u16 = 0xF00; // the temperature that steps up to step i + 1
const CPU_DOWN: u16 = 0xF10; // the temperature that steps down from step i (at i + 1)
const CPU_DUTY: u16 = 0xF20; // duty × 2
const GPU_UP: u16 = 0xF30;
const GPU_DOWN: u16 = 0xF40;
const GPU_DUTY: u16 = 0xF50;
const TABLE_STATUS1: u16 = 0xF5D;
const TABLE_STATUS2: u16 = 0xF5E;
const TABLE_REQUEST: u16 = 0xF5F; // which mode's built-in table to copy into RAM
const RAM_FAN_CONTROL: u16 = 0x7C6; // bit 2: the fans follow the RAM tables (MyFanTableCtrl.SetFanControlByRamFan1p5)
const RAM_FAN_BIT: u8 = 0x04;
const OEM_BYTE5: u16 = 0x7C5; // bit 7: CPU and GPU fans each follow their own table
const SAFETY: u16 = 0x7A5; // bit 3: the EC's fan safety protection is on
const RAM_FAN_SUPPORT: u16 = 0x78E; // bit 6

const CPU_FAN_DUTY: u16 = 0x75B; // duty × 2
const GPU_FAN_DUTY: u16 = 0x75C;
const CPU_FAN_RPM: (u16, u16) = (0x464, 0x465); // high, low
const GPU_FAN_RPM: (u16, u16) = (0x46C, 0x46B);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Step {
    pub up: u8,
    pub down: u8,
    pub duty: u8, // percent
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Table {
    pub cpu: Vec<Step>,
    pub gpu: Vec<Step>,
}

impl Table {
    pub fn is_valid(&self) -> bool {
        let valid = |steps: &[Step]| {
            steps.len() == STEPS
                && steps.iter().any(|s| s.duty > 0)
                && steps.windows(2).all(|w| w[0].duty <= w[1].duty && w[0].up <= w[1].up)
                && steps.iter().all(|s| s.duty <= 100)
        };
        valid(&self.cpu) && valid(&self.gpu)
    }
}

/// The steps a curve editor shows: those the temperature can reach.
pub fn used_steps(steps: &[Step]) -> usize {
    steps.iter().take_while(|s| s.up < 255).count().max(1)
}

/// The lowest duty a custom curve may set for a step: at most 30 points below the default, and never below it from
/// 75 °C up, so the fans still keep the laptop out of thermal trouble.
pub fn minimum_duty(default: &Step) -> u8 {
    if default.up >= 75 { default.duty } else { default.duty.saturating_sub(30) }
}

/// Makes `duty` safe for the step at `index` of a curve and keeps the curve rising: the steps before it come down
/// to it, the steps after it come up to it.
pub fn set_duty(steps: &mut [Step], defaults: &[Step], index: usize, duty: u8) {
    if index >= steps.len() || defaults.len() != steps.len() {
        return;
    }
    steps[index].duty = duty.clamp(minimum_duty(&defaults[index]), 100);
    let value = steps[index].duty;
    for i in (0..index).rev() {
        if steps[i].duty > value {
            steps[i].duty = value.max(minimum_duty(&defaults[i]));
        }
    }
    for i in index + 1..steps.len() {
        steps[i].duty = steps[i].duty.max(steps[i - 1].duty);
    }
}

/// Raises any step that's below its minimum and makes the curve rising again.
pub fn make_safe(steps: &mut Vec<Step>, defaults: &[Step]) {
    if defaults.len() != steps.len() {
        *steps = defaults.to_vec();
        return;
    }
    for i in 0..steps.len() {
        steps[i].up = defaults[i].up;
        steps[i].down = defaults[i].down;
        steps[i].duty = steps[i].duty.clamp(minimum_duty(&defaults[i]), 100);
        if i > 0 {
            steps[i].duty = steps[i].duty.max(steps[i - 1].duty);
        }
    }
}

pub fn supported(ec: &Registers) -> Result<bool> {
    ec.bit(RAM_FAN_SUPPORT, 6)
}

fn read_steps(ec: &Registers, up: u16, down: u16, duty: u16) -> Result<Vec<Step>> {
    // FanTable_Manager1p5.GetEcFanTable
    let mut steps = Vec::with_capacity(STEPS);
    for i in 0..STEPS as u16 {
        steps.push(Step {
            up: if i == 0 { 0 } else { ec.read(up + i - 1)? },
            down: ec.read(down + if i < 15 { i + 1 } else { i })?,
            duty: ec.read(duty + i)? / 2,
        });
    }
    Ok(steps)
}

/// The registers and values that hold `steps` in the EC's RAM table (FanTable_Manager1p5.SetEcFanTable).
fn step_writes(steps: &[Step], up: u16, down: u16, duty: u16) -> Vec<(u16, u8)> {
    let mut writes = Vec::with_capacity(3 * STEPS);
    for i in 0..STEPS {
        let a = i as u16;
        writes.push((up + a, if i < 15 { steps[i + 1].up } else { 255 }));
        if i < 15 {
            writes.push((down + a + 1, steps[i].down));
        }
        writes.push((duty + a, steps[i].duty.min(100) * 2));
    }
    writes
}

fn table_writes(table: &Table) -> Vec<(u16, u8)> {
    let mut writes = step_writes(&table.cpu, CPU_UP, CPU_DOWN, CPU_DUTY);
    writes.extend(step_writes(&table.gpu, GPU_UP, GPU_DOWN, GPU_DUTY));
    writes
}

/// The writes that turn the RAM tables from `written` (None: unknown) into `table`. On Windows each EC call takes
/// over 100 ms, so a whole table is many seconds; a dragged point is one register.
fn changed_writes(written: Option<&Table>, table: &Table) -> Vec<(u16, u8)> {
    let next = table_writes(table);
    match written.map(table_writes) {
        Some(before) if before.len() == next.len() => next.into_iter().zip(before).filter(|(n, b)| n != b).map(|(n, _)| n).collect(),
        _ => next,
    }
}

/// What this app last wrote to the RAM tables, while nothing else can have changed them since.
static WRITTEN: Mutex<Option<Table>> = Mutex::new(None);

/// The RAM tables may have changed (the EC copied a mode's table in, or Control Center took over): the next custom
/// curve writes all of itself.
pub fn forget_written() {
    *WRITTEN.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// Reads the built-in table of each mode from the EC (FanTable_Manager1p5.RefreshDefaultFanTableAll). Takes a few
/// seconds; meanwhile the Fn keys go to the firmware and the fans follow the EC's own tables.
///
/// The request bytes are also the GPU's last three duties: the EC copies the mode's table as soon as 0xF5D holds 0xFD
/// and 0xF5E 0xC9, over the request. It often leaves the 0xC9 in 0xF5E, so a request starts with both cleared (or
/// the copy would start early, under the next write), and it's done once 0xF5F and 0xF5D have changed.
pub fn read_defaults(ec: &Registers) -> Result<Vec<(Mode, Table)>> {
    set_custom(ec, false)?;
    forget_written();
    let tables = hotkeys::without_app_present(ec, || -> Result<Vec<(Mode, Table)>> {
        let mut tables = Vec::new();
        for mode in Mode::ALL {
            let request = match mode {
                Mode::Turbo => 1,
                Mode::Gaming => 2,
                Mode::Office => 3,
            };
            ec.write(TABLE_STATUS1, 0)?;
            ec.write(TABLE_STATUS2, 0)?;
            ec.write(TABLE_REQUEST, request)?;
            ec.write(TABLE_STATUS1, 0xFD)?;
            ec.write(TABLE_STATUS2, 0xC9)?;
            let mut ready = false;
            for _ in 0..6 {
                std::thread::sleep(Duration::from_millis(500));
                if ec.read(TABLE_REQUEST)? != request && ec.read(TABLE_STATUS1)? != 0xFD {
                    ready = true;
                    break;
                }
            }
            if !ready {
                bail!("The EC didn't hand over its {} fan table", mode.name());
            }
            let mut table = Table { cpu: read_steps(ec, CPU_UP, CPU_DOWN, CPU_DUTY)?, gpu: read_steps(ec, GPU_UP, GPU_DOWN, GPU_DUTY)? };
            if ec.read(TABLE_STATUS2)? == 0xC9 {
                // The marker, not a duty: that step (past the used ones) takes the one before's.
                let step = (TABLE_STATUS2 - GPU_DUTY) as usize;
                table.gpu[step].duty = table.gpu[step - 1].duty;
            }
            if !table.is_valid() {
                bail!("The EC's {} fan table doesn't look right", mode.name());
            }
            tables.push((mode, table));
        }
        Ok(tables)
    })??;
    Ok(tables)
}

/// Makes the fans follow `table` (MyFanTableCtrl.SetFanTable): off while it's written, then on.
pub fn apply_custom(ec: &Registers, table: &Table) -> Result<()> {
    if !table.is_valid() {
        bail!("That fan curve isn't valid");
    }
    let writes = changed_writes(WRITTEN.lock().unwrap_or_else(|e| e.into_inner()).as_ref(), table);
    if !writes.is_empty() {
        set_custom(ec, false)?;
        forget_written();
    }
    ec.update(OEM_BYTE5, 0x80, 0)?; // one table per fan, not shared
    for (address, value) in writes {
        ec.write(address, value)?;
    }
    *WRITTEN.lock().unwrap_or_else(|e| e.into_inner()) = Some(table.clone());
    set_custom(ec, true)
}

/// Turns the RAM tables on or off; off, the EC uses its own table for the mode. On Linux the helper is also told to
/// turn them off if the app goes away with them on.
pub fn set_custom(ec: &Registers, on: bool) -> Result<()> {
    #[cfg(target_os = "linux")]
    if on {
        ec.on_disconnect(RAM_FAN_CONTROL, RAM_FAN_BIT, 0)?;
    }
    ec.update(RAM_FAN_CONTROL, RAM_FAN_BIT, if on { RAM_FAN_BIT } else { 0 })?;
    #[cfg(target_os = "linux")]
    if !on {
        ec.clear_on_disconnect(RAM_FAN_CONTROL)?;
    }
    Ok(())
}

/// The EC while a custom curve is on, so a crash can turn it off: with nothing watching the EC's safety protection,
/// the fans should follow the EC's own curve.
static CUSTOM: Mutex<Option<Ec>> = Mutex::new(None);

/// Notes whether a custom curve is on, for `release_current`.
pub fn note_custom(ec: Option<&Ec>) {
    *CUSTOM.lock().unwrap_or_else(|e| e.into_inner()) = ec.cloned();
}

/// Gives the fans back to the EC's own curve if a custom one is on; for crash and exit handlers.
pub fn release_current() {
    let current = CUSTOM.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(ec) = current {
        let _ = ec.run_for(Duration::from_secs(2), |ec| set_custom(ec, false));
    }
}

pub fn custom_active(ec: &Registers) -> Result<bool> {
    ec.bit(RAM_FAN_CONTROL, 2)
}

/// The EC turned its fan safety protection on: Control Center then goes back to the default tables.
pub fn safety_protection(ec: &Registers) -> Result<bool> {
    ec.bit(SAFETY, 3)
}

#[derive(Clone, Copy, Debug, Default)]
pub struct FanReading {
    pub duty: u8, // percent
    pub rpm: u16,
}

/// The fans' current speeds (Control Center's FanInfo).
pub fn readings(ec: &Registers) -> Result<(FanReading, FanReading)> {
    let rpm = |(high, low): (u16, u16)| -> Result<u16> { Ok((ec.read(high)? as u16) << 8 | ec.read(low)? as u16) };
    Ok((
        FanReading { duty: (ec.read(CPU_FAN_DUTY)? / 2).min(100), rpm: rpm(CPU_FAN_RPM)? },
        FanReading { duty: (ec.read(GPU_FAN_DUTY)? / 2).min(100), rpm: rpm(GPU_FAN_RPM)? },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gaming() -> Vec<Step> {
        let ups = [0, 53, 57, 59, 61, 63, 65, 67, 75, 80];
        let duties = [0, 30, 30, 35, 45, 48, 50, 55, 70, 85];
        (0..STEPS)
            .map(|i| Step { up: *ups.get(i).unwrap_or(&255), down: ups.get(i).map_or(255, |u| u.saturating_sub(3)), duty: *duties.get(i).unwrap_or(&85) })
            .collect()
    }

    #[test]
    fn duty_stays_safe_and_rising() {
        let defaults = gaming();
        let mut steps = defaults.clone();
        set_duty(&mut steps, &defaults, 4, 0);
        assert_eq!(steps[4].duty, 15); // 45 - 30
        assert!(steps.windows(2).all(|w| w[0].duty <= w[1].duty));
        set_duty(&mut steps, &defaults, 8, 10);
        assert_eq!(steps[8].duty, 70); // 75 °C: not below the default
        set_duty(&mut steps, &defaults, 2, 90);
        assert_eq!(steps[2].duty, 90);
        assert!(steps[3..].iter().all(|s| s.duty >= 90));
        assert_eq!(used_steps(&steps), 10);
    }

    #[test]
    fn only_changed_registers_are_written() {
        let table = Table { cpu: gaming(), gpu: gaming() };
        // Nothing known about the RAM tables: all of both, 47 registers each.
        let all = changed_writes(None, &table);
        assert_eq!(all.len(), 94);
        assert!(all.contains(&(0xF24, 90))); // CPU step 4: 45% × 2
        // Each EC call takes over 100 ms on Windows: the same table again writes nothing...
        assert!(changed_writes(Some(&table), &table).is_empty());
        // ...and one dragged point writes its one register.
        let mut dragged = table.clone();
        dragged.cpu[4].duty = 60;
        assert_eq!(changed_writes(Some(&table), &dragged), vec![(0xF24, 120)]);
        let mut moved = table.clone();
        moved.gpu[2].up = 58; // the temperature stepping up to GPU step 2 sits at 0xF31
        assert_eq!(changed_writes(Some(&table), &moved), vec![(0xF31, 58)]);
    }
}
