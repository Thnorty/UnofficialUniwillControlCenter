//! Takes over the laptop's Fn hotkeys the way Control Center's GCUService does (App.InitalizeStartPerparation):
//! setting the "AP exist" bit in the EC makes the firmware stop handling keyboard-light keys itself and report every
//! hotkey as an AcpiTest_EventULong WMI event instead, so software can react instantly.
//!
//! The events arrive on a thread of their own; EC reads and writes go through the shared EC thread.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use wmi::WMIConnection;

use super::ec::Ec;

// Define.ECSpec
const AP_OEM_BYTE: u16 = 0x741; // ADDR_AP_OEM_BYTE
const AP_EXIST_BIT: u8 = 0x01; // MyEcCtrl.Set_APExistToEC
const TRIGGER_BYTE: u16 = 0x767; // TriggerByteFlag.WinLock_Trigger = bit 0 (MySettingManager.WinKeyLock_Trigger)
const STATUS_BYTE: u16 = 0x768; // StatusByteOneFlag.WinLock = bit 0 (MySettingManager.SetWinKey)

pub const KEYBOARD_LIGHT_DOWN: u32 = 177;
pub const KEYBOARD_LIGHT_UP: u32 = 178;
pub const KEYBOARD_LIGHT_CYCLE: u32 = 185;
pub const LIGHT_BAR_TOGGLE: u32 = 175; // HIDLightbar.SetBrinessByScanCode
pub const WIN_KEY_LOCKED: u32 = 64; // OSD_WINKEY_LOCK
pub const WIN_KEY_UNLOCKED: u32 = 65; // OSD_WINKEY_UNLOCK
pub const MODE_SWITCH: u32 = 176; // MyFanManager.ModeSwitchChanged: the performance mode key
pub const FAN_BOOST_CHANGED: u32 = 167; // FanBoostUpdate: the EC flipped Fan Boost (its Fn key)
pub const FAN_BOOST_OFF: u32 = 172; // FanBoostOffFromEC
pub const FAN_SAFETY: u32 = 187; // SafetyProtectionUpdate: the EC's fan safety protection changed

pub struct Hotkeys {
    ec: Ec,
    active: Arc<AtomicBool>,
}

/// The running instance's EC, so a crash can give the hotkeys back. If the bit stayed set with nothing listening,
/// the keyboard-light keys would do nothing until this app ran again.
static CURRENT: Mutex<Option<Ec>> = Mutex::new(None);

impl Hotkeys {
    /// Takes over the hotkeys, or returns why this machine or process can't. `pressed` is called on a background
    /// thread with the firmware's hotkey code.
    pub fn start(ec: Ec, pressed: impl Fn(u32) + Send + 'static) -> Result<Hotkeys> {
        let active = Arc::new(AtomicBool::new(false));

        // Listen before taking the keys over, so no press is lost.
        let (listening_tx, listening_rx) = mpsc::channel();
        let event_active = active.clone();
        std::thread::Builder::new()
            .name("Hotkey events".into())
            .spawn(move || event_thread(pressed, listening_tx, event_active))?;
        listening_rx.recv().map_err(|_| anyhow!("The hotkey listener stopped"))??;

        active.store(true, Ordering::Release);
        *CURRENT.lock().unwrap_or_else(|e| e.into_inner()) = Some(ec.clone());
        let hotkeys = Hotkeys { ec, active };
        let taken = hotkeys.ec.run_for(Duration::from_secs(5), |ec| {
            let value = ec.read(AP_OEM_BYTE)?;
            if value & AP_EXIST_BIT == 0 {
                ec.write(AP_OEM_BYTE, value | AP_EXIST_BIT)?;
                if ec.read(AP_OEM_BYTE)? & AP_EXIST_BIT == 0 {
                    return Err(anyhow!("The EC didn't keep the app-present bit"));
                }
            }
            Ok(())
        });
        match taken {
            Ok(()) => Ok(hotkeys),
            Err(e) => {
                hotkeys.abandon();
                Err(e.context("Can't take the Fn keys over"))
            }
        }
    }

    /// Sets the bit if something cleared it. GCUService re-sets it after resume from sleep, so this is called
    /// periodically and on resume.
    pub fn ensure_ap_exist(&self) {
        let active = self.active.clone();
        self.ec.spawn(move |ec| {
            if active.load(Ordering::Acquire)
                && let Ok(value) = ec.read(AP_OEM_BYTE)
                && value & AP_EXIST_BIT == 0
            {
                let _ = ec.write(AP_OEM_BYTE, value | AP_EXIST_BIT);
            }
        });
    }

    /// Asks the EC to flip the Windows key lock; it reports the result as a 64/65 hotkey event.
    pub fn toggle_win_key_lock(&self) {
        let active = self.active.clone();
        self.ec.spawn(move |ec| {
            if active.load(Ordering::Acquire) {
                let _ = ec.update(TRIGGER_BYTE, 0x01, 0x01);
            }
        });
    }

    /// Reads the Windows key lock on the EC thread; the answer arrives on the returned channel.
    pub fn query_win_key_lock(&self) -> Receiver<Result<bool>> {
        let (tx, rx) = mpsc::channel();
        self.ec.spawn(move |ec| {
            tx.send(ec.bit(STATUS_BYTE, 0)).ok();
        });
        rx
    }

    /// Stops handling hotkeys without touching the EC, for when Control Center's service takes over the bit.
    pub fn abandon(self) {
        self.active.store(false, Ordering::Release);
        clear_current();
    }

    /// Hands the hotkeys back to the firmware and waits (briefly) until that's done.
    pub fn release(self) {
        release_through(&self.ec);
        self.active.store(false, Ordering::Release);
        clear_current();
    }
}

fn clear_current() {
    *CURRENT.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

fn release_through(ec: &Ec) {
    let _ = ec.run_for(Duration::from_secs(2), |ec| ec.update(AP_OEM_BYTE, AP_EXIST_BIT, 0));
}

/// Gives the hotkeys back if they're taken; for crash and exit handlers.
pub fn release_current() {
    let current = CURRENT.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(ec) = current {
        release_through(&ec);
    }
}

/// While the firmware's fan tables are read the EC must think no app is running (FanTable_Manager1p5.
/// RefreshDefaultFanTableAll); this clears the bit for `job` and puts it back after. Fn key presses meanwhile go to
/// the firmware.
pub fn without_app_present<R>(ec: &super::ec::Registers, job: impl FnOnce() -> R) -> Result<R> {
    let set = ec.bit(AP_OEM_BYTE, 0)?;
    if set {
        ec.update(AP_OEM_BYTE, AP_EXIST_BIT, 0)?;
    }
    let result = job();
    if set {
        ec.update(AP_OEM_BYTE, AP_EXIST_BIT, AP_EXIST_BIT)?;
    }
    Ok(result)
}

/// Only the code is read, so the event's other properties can't make it fail to convert.
#[derive(serde::Deserialize)]
#[serde(rename = "AcpiTest_EventULong")]
struct HotkeyEvent {
    #[serde(rename = "ULong")]
    code: u32,
}

fn event_thread(pressed: impl Fn(u32), listening: Sender<Result<()>>, active: Arc<AtomicBool>) {
    let wmi = match WMIConnection::with_namespace_path("ROOT\\WMI").context("Can't open the BIOS WMI interface") {
        Ok(wmi) => wmi,
        Err(e) => {
            listening.send(Err(e)).ok();
            return;
        }
    };
    let events = wmi.raw_notification::<HotkeyEvent>("SELECT * FROM AcpiTest_EventULong").context("Can't listen for hotkeys");
    let events = match events {
        Ok(events) => {
            listening.send(Ok(())).ok();
            events
        }
        Err(e) => {
            listening.send(Err(e)).ok();
            return;
        }
    };
    for event in events {
        let Ok(event) = event else { continue };
        if !active.load(Ordering::Acquire) {
            if CURRENT.lock().map(|c| c.is_none()).unwrap_or(true) {
                return; // given back; this listener is done
            }
            continue;
        }
        pressed(event.code);
    }
}
