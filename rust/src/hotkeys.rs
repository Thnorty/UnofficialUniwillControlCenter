//! Takes over the laptop's Fn hotkeys the way Control Center's GCUService does (App.InitalizeStartPerparation):
//! setting the "AP exist" bit in the EC makes the firmware stop handling keyboard-light keys itself and report every
//! hotkey as an AcpiTest_EventULong WMI event instead, so software can react instantly. EC access goes through the
//! BIOS WMI method AcpiTest_MULong.GetSetULong, which needs administrator rights.
//!
//! WMI runs on two threads of its own: one for EC reads and writes, one waiting for hotkey events.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use wmi::{Variant, WMIConnection};

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

enum Command {
    EnsureApExist,
    ToggleWinKeyLock,
    IsWinKeyLocked(Sender<Result<bool>>),
    Release(Sender<()>),
}

pub struct Hotkeys {
    commands: Sender<Command>,
    active: Arc<AtomicBool>,
}

/// The running instance's command channel, so a crash can give the hotkeys back. If the bit stayed set with nothing
/// listening, the keyboard-light keys would do nothing until this app ran again.
static CURRENT: Mutex<Option<Sender<Command>>> = Mutex::new(None);

impl Hotkeys {
    /// Takes over the hotkeys, or returns why this machine or process can't. `pressed` is called on a background
    /// thread with the firmware's hotkey code.
    pub fn start(pressed: impl Fn(u32) + Send + 'static) -> Result<Hotkeys> {
        let active = Arc::new(AtomicBool::new(false));
        let (commands, receiver) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let ec_active = active.clone();
        std::thread::Builder::new()
            .name("EC".into())
            .spawn(move || ec_thread(receiver, ready_tx, ec_active))?;
        ready_rx.recv().map_err(|_| anyhow!("The EC thread stopped"))??;

        // Listen before taking the keys over, so no press is lost.
        let (listening_tx, listening_rx) = mpsc::channel();
        let event_active = active.clone();
        std::thread::Builder::new()
            .name("Hotkey events".into())
            .spawn(move || event_thread(pressed, listening_tx, event_active))?;
        listening_rx.recv().map_err(|_| anyhow!("The hotkey listener stopped"))??;

        active.store(true, Ordering::Release);
        commands.send(Command::EnsureApExist).ok();
        *CURRENT.lock().unwrap_or_else(|e| e.into_inner()) = Some(commands.clone());
        Ok(Hotkeys { commands, active })
    }

    /// Sets the bit if something cleared it. GCUService re-sets it after resume from sleep, so this is called
    /// periodically and on resume.
    pub fn ensure_ap_exist(&self) {
        self.commands.send(Command::EnsureApExist).ok();
    }

    /// Asks the EC to flip the Windows key lock; it reports the result as a 64/65 hotkey event.
    pub fn toggle_win_key_lock(&self) {
        self.commands.send(Command::ToggleWinKeyLock).ok();
    }

    /// Reads the Windows key lock on the EC thread; the answer arrives on the returned channel.
    pub fn query_win_key_lock(&self) -> Receiver<Result<bool>> {
        let (tx, rx) = mpsc::channel();
        self.commands.send(Command::IsWinKeyLocked(tx)).ok();
        rx
    }

    /// Stops handling hotkeys without touching the EC, for when Control Center's service takes over the bit.
    pub fn abandon(self) {
        self.active.store(false, Ordering::Release);
        clear_current();
    }

    /// Hands the hotkeys back to the firmware and waits (briefly) until that's done.
    pub fn release(self) {
        release_through(&self.commands);
        self.active.store(false, Ordering::Release);
        clear_current();
    }
}

fn clear_current() {
    *CURRENT.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

fn release_through(commands: &Sender<Command>) {
    let (tx, rx) = mpsc::channel();
    if commands.send(Command::Release(tx)).is_ok() {
        let _ = rx.recv_timeout(Duration::from_secs(2));
    }
}

/// Gives the hotkeys back if they're taken; for crash and exit handlers.
pub fn release_current() {
    let current = CURRENT.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(commands) = current {
        release_through(&commands);
    }
}

struct Ec {
    wmi: WMIConnection,
    path: String,
}

impl Ec {
    fn connect() -> Result<Ec> {
        let wmi = WMIConnection::with_namespace_path("ROOT\\WMI").context("Can't open the BIOS WMI interface")?;
        // AcpiTest_MULong.InstanceName='ACPI\PNP0C14\1_1' on this laptop; ask WMI for the exact path.
        let rows: Vec<HashMap<String, Variant>> = wmi.raw_query("SELECT __RELPATH FROM AcpiTest_MULong").context("No EC interface (AcpiTest_MULong)")?;
        let path = rows
            .iter()
            .find_map(|row| match row.get("__RELPATH") {
                Some(Variant::String(path)) => Some(path.clone()),
                _ => None,
            })
            .ok_or_else(|| anyhow!("No EC interface (AcpiTest_MULong)"))?;
        Ok(Ec { wmi, path })
    }

    fn invoke(&self, data: u64) -> Result<u64> {
        let class = self.wmi.get_object("AcpiTest_MULong")?;
        let input = class.get_method("GetSetULong")?.ok_or_else(|| anyhow!("No GetSetULong method"))?.spawn_instance()?;
        input.put_property("Data", data)?;
        let output = self.wmi.exec_method(&self.path, "GetSetULong", Some(&input))?.ok_or_else(|| anyhow!("No answer from the EC"))?;
        match output.get_property("Return")? {
            Variant::UI8(v) => Ok(v),
            Variant::I8(v) => Ok(v as u64),
            Variant::UI4(v) => Ok(v as u64),
            Variant::I4(v) => Ok(v as u64),
            Variant::String(s) => Ok(s.parse()?),
            other => Err(anyhow!("Unexpected EC answer {other:?}")),
        }
    }

    // WMIEC.WMIReadECRAM / WMIWriteECRAM: bit 40 selects a read; a write packs the value above the address.
    fn read(&self, address: u16) -> Result<u8> {
        Ok((self.invoke((1u64 << 40) + address as u64)? & 0xFF) as u8)
    }

    fn write(&self, address: u16, value: u8) -> Result<()> {
        self.invoke(((value as u64) << 16) + address as u64).map(|_| ())
    }
}

fn ec_thread(commands: Receiver<Command>, ready: Sender<Result<()>>, active: Arc<AtomicBool>) {
    let ec = match Ec::connect().and_then(|ec| ec.read(AP_OEM_BYTE).map(|_| ec)) {
        Ok(ec) => {
            ready.send(Ok(())).ok();
            ec
        }
        Err(e) => {
            ready.send(Err(e)).ok();
            return;
        }
    };
    for command in commands {
        match command {
            Command::EnsureApExist => {
                if active.load(Ordering::Acquire)
                    && let Ok(value) = ec.read(AP_OEM_BYTE)
                    && value & AP_EXIST_BIT == 0
                {
                    let _ = ec.write(AP_OEM_BYTE, value | AP_EXIST_BIT);
                }
            }
            Command::ToggleWinKeyLock => {
                if active.load(Ordering::Acquire)
                    && let Ok(value) = ec.read(TRIGGER_BYTE)
                {
                    let _ = ec.write(TRIGGER_BYTE, value | 0x01);
                }
            }
            Command::IsWinKeyLocked(reply) => {
                reply.send(ec.read(STATUS_BYTE).map(|v| v & 0x01 != 0)).ok();
            }
            Command::Release(reply) => {
                if active.load(Ordering::Acquire)
                    && let Ok(value) = ec.read(AP_OEM_BYTE)
                {
                    let _ = ec.write(AP_OEM_BYTE, value & !AP_EXIST_BIT);
                }
                reply.send(()).ok();
                return;
            }
        }
    }
}

fn event_thread(pressed: impl Fn(u32), listening: Sender<Result<()>>, active: Arc<AtomicBool>) {
    let wmi = match WMIConnection::with_namespace_path("ROOT\\WMI").context("Can't open the BIOS WMI interface") {
        Ok(wmi) => wmi,
        Err(e) => {
            listening.send(Err(e)).ok();
            return;
        }
    };
    let events = wmi.raw_notification::<HashMap<String, Variant>>("SELECT * FROM AcpiTest_EventULong").context("Can't listen for hotkeys");
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
        let code = match event.get("ULong") {
            Some(Variant::UI4(v)) => *v,
            Some(Variant::I4(v)) => *v as u32,
            Some(Variant::UI8(v)) => *v as u32,
            Some(Variant::String(s)) => s.parse().unwrap_or(0),
            _ => continue,
        };
        pressed(code);
    }
}
