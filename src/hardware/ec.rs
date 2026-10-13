//! The embedded controller's RAM, read and written through the BIOS WMI method AcpiTest_MULong.GetSetULong, the way
//! Control Center's service does (WMIEC.WMIReadECRAM / WMIWriteECRAM). It needs administrator rights.
//!
//! All access goes through one thread, so a read-modify-write of a register is never interleaved with another, and
//! WMI's COM objects stay on the thread that made them.

use std::sync::mpsc::{self, Sender};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use wmi::{IWbemClassWrapper, Variant, WMIConnection};

type Job = Box<dyn FnOnce(&Registers) + Send>;

/// A handle to the EC thread; clones share it. The thread ends when the last handle is dropped.
#[derive(Clone)]
pub struct Ec {
    jobs: Sender<Job>,
}

impl Ec {
    /// Starts the EC thread and checks that the EC answers.
    pub fn connect() -> Result<Ec> {
        let (jobs, receiver) = mpsc::channel::<Job>();
        let (ready_tx, ready_rx) = mpsc::channel();
        std::thread::Builder::new().name("EC".into()).spawn(move || {
            let registers = match Registers::connect() {
                Ok(registers) => {
                    ready_tx.send(Ok(())).ok();
                    registers
                }
                Err(e) => {
                    ready_tx.send(Err(e)).ok();
                    return;
                }
            };
            for job in receiver {
                job(&registers);
            }
        })?;
        ready_rx.recv().map_err(|_| anyhow!("The EC thread stopped"))??;
        Ok(Ec { jobs })
    }

    /// Runs `job` on the EC thread and waits at most `timeout` for its result.
    pub fn run_for<R: Send + 'static>(&self, timeout: Duration, job: impl FnOnce(&Registers) -> Result<R> + Send + 'static) -> Result<R> {
        let (tx, rx) = mpsc::channel();
        self.jobs
            .send(Box::new(move |registers| {
                tx.send(job(registers)).ok();
            }))
            .map_err(|_| anyhow!("The EC thread stopped"))?;
        rx.recv_timeout(timeout).map_err(|_| anyhow!("The EC didn't answer"))?
    }

    /// Runs `job` on the EC thread without waiting.
    pub fn spawn(&self, job: impl FnOnce(&Registers) + Send + 'static) {
        self.jobs.send(Box::new(job)).ok();
    }
}

/// The EC's RAM, on the EC thread.
pub struct Registers {
    wmi: WMIConnection,
    path: String,
    input: IWbemClassWrapper, // GetSetULong's input parameters
}

impl Registers {
    fn connect() -> Result<Registers> {
        let wmi = WMIConnection::with_namespace_path("ROOT\\WMI").context("Can't open the BIOS WMI interface")?;
        // AcpiTest_MULong.InstanceName='ACPI\PNP0C14\1_1' on this laptop; ask WMI for the exact path. It's a system
        // property, which the crate's query results leave out, so it's read from the object itself.
        let instance = wmi
            .exec_query("SELECT * FROM AcpiTest_MULong")
            .context("No EC interface (AcpiTest_MULong)")?
            .next()
            .ok_or_else(|| anyhow!("No EC interface (AcpiTest_MULong)"))??;
        let path = match instance.get_property("__RELPATH")? {
            Variant::String(path) => path,
            other => return Err(anyhow!("Unexpected EC interface path {other:?}")),
        };
        let input = wmi.get_object("AcpiTest_MULong")?.get_method("GetSetULong")?.ok_or_else(|| anyhow!("No GetSetULong method"))?;
        let registers = Registers { wmi, path, input };
        registers.read(0x741)?; // proves the method works
        Ok(registers)
    }

    fn invoke(&self, data: u64) -> Result<u64> {
        let input = self.input.spawn_instance()?;
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

    /// Bit 40 selects a read.
    pub fn read(&self, address: u16) -> Result<u8> {
        Ok((self.invoke((1u64 << 40) + address as u64)? & 0xFF) as u8)
    }

    /// A write packs the value above the address.
    pub fn write(&self, address: u16, value: u8) -> Result<()> {
        self.invoke(((value as u64) << 16) + address as u64).map(|_| ())
    }

    /// Sets the bits in `mask` to those of `value`, leaving the others.
    pub fn update(&self, address: u16, mask: u8, value: u8) -> Result<()> {
        let current = self.read(address)?;
        let next = (current & !mask) | (value & mask);
        if next != current { self.write(address, next) } else { Ok(()) }
    }

    pub fn bit(&self, address: u16, bit: u8) -> Result<bool> {
        Ok(self.read(address)? & (1 << bit) != 0)
    }
}
