//! The Windows Core Audio pieces the sound effects use: the output peak meter (Music) and a loopback capture of
//! what's playing (Spectrum). Used on the effects thread, which has COM initialized.

use anyhow::Result;
use windows::Win32::Media::Audio::Endpoints::IAudioMeterInformation;
use windows::Win32::Media::Audio::{
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK, IAudioCaptureClient, IAudioClient,
    IMMDeviceEnumerator, MMDeviceEnumerator, WAVEFORMATEX, WAVEFORMATEXTENSIBLE, eMultimedia, eRender,
};
use windows::Win32::Media::KernelStreaming::WAVE_FORMAT_EXTENSIBLE;
use windows::Win32::Media::Multimedia::{KSDATAFORMAT_SUBTYPE_IEEE_FLOAT, WAVE_FORMAT_IEEE_FLOAT};
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance, CoTaskMemFree};

const WAVE_FORMAT_PCM: u16 = 1;

fn default_output() -> Result<windows::Win32::Media::Audio::IMMDevice> {
    unsafe {
        let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        Ok(enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia)?)
    }
}

/// The peak meter of the default playback device.
pub struct AudioMeter(IAudioMeterInformation);

impl AudioMeter {
    pub fn open_default_output() -> Result<AudioMeter> {
        unsafe { Ok(AudioMeter(default_output()?.Activate(CLSCTX_ALL, None)?)) }
    }

    pub fn read_peak(&self) -> Result<f32> {
        unsafe { Ok(self.0.GetPeakValue()?) }
    }
}

/// Records what the default playback device is playing (WASAPI loopback), mixed down to mono, for the spectrum.
/// Read it regularly with read_into; nothing is buffered when nothing plays.
pub struct LoopbackCapture {
    client: IAudioClient,
    capture: IAudioCaptureClient,
    channels: usize,
    bits: u16,
    is_float: bool,
    pub sample_rate: u32,
}

impl LoopbackCapture {
    pub fn open_default_output() -> Result<Option<LoopbackCapture>> {
        unsafe {
            let client: IAudioClient = default_output()?.Activate(CLSCTX_ALL, None)?;
            let format = client.GetMixFormat()?;
            let result = (|| -> Result<Option<LoopbackCapture>> {
                let f: WAVEFORMATEX = *format;
                let tag = f.wFormatTag;
                let is_float = tag == WAVE_FORMAT_IEEE_FLOAT as u16
                    || (tag == WAVE_FORMAT_EXTENSIBLE as u16 && {
                        let ext = &*(format as *const WAVEFORMATEXTENSIBLE);
                        let sub = std::ptr::addr_of!(ext.SubFormat).read_unaligned();
                        sub == KSDATAFORMAT_SUBTYPE_IEEE_FLOAT
                    });
                let bits = f.wBitsPerSample;
                let pcm = tag == WAVE_FORMAT_PCM || tag == WAVE_FORMAT_EXTENSIBLE as u16;
                if !(is_float && bits == 32) && !(!is_float && pcm && (bits == 16 || bits == 32)) {
                    return Ok(None);
                }
                // 1 s buffer, in 100 ns units.
                client.Initialize(AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK, 10_000_000, 0, format, None)?;
                let capture: IAudioCaptureClient = client.GetService()?;
                client.Start()?;
                Ok(Some(LoopbackCapture {
                    client: client.clone(),
                    capture,
                    channels: f.nChannels.max(1) as usize,
                    bits,
                    is_float,
                    sample_rate: f.nSamplesPerSec,
                }))
            })();
            CoTaskMemFree(Some(format as _));
            result
        }
    }

    /// Appends everything captured since the last call to `samples` as mono floats.
    pub fn read_into(&self, samples: &mut SampleHistory) -> Result<()> {
        unsafe {
            loop {
                if self.capture.GetNextPacketSize()? == 0 {
                    return Ok(());
                }
                let mut data = std::ptr::null_mut();
                let mut frames = 0u32;
                let mut flags = 0u32;
                self.capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
                let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0;
                let bytes = (self.bits / 8) as usize;
                for frame in 0..frames as usize {
                    let mut sum = 0.0f32;
                    if !silent {
                        for channel in 0..self.channels {
                            let p = data.add((frame * self.channels + channel) * bytes);
                            sum += if self.is_float {
                                (p as *const f32).read_unaligned()
                            } else if self.bits == 16 {
                                (p as *const i16).read_unaligned() as f32 / 32768.0
                            } else {
                                (p as *const i32).read_unaligned() as f32 / 2_147_483_648.0
                            };
                        }
                    }
                    samples.add(sum / self.channels as f32);
                }
                self.capture.ReleaseBuffer(frames)?;
            }
        }
    }
}

impl Drop for LoopbackCapture {
    fn drop(&mut self) {
        unsafe {
            let _ = self.client.Stop();
        }
    }
}

/// The most recent audio samples, oldest first.
pub struct SampleHistory {
    ring: Vec<f32>,
    next: usize,
}

impl SampleHistory {
    pub fn new(length: usize) -> Self {
        SampleHistory { ring: vec![0.0; length], next: 0 }
    }

    pub fn add(&mut self, sample: f32) {
        self.ring[self.next] = sample;
        self.next = (self.next + 1) % self.ring.len();
    }

    pub fn clear(&mut self) {
        self.ring.fill(0.0);
    }

    pub fn copy_to(&self, destination: &mut [f32]) {
        let n = self.ring.len();
        for (i, d) in destination.iter_mut().enumerate().take(n) {
            *d = self.ring[(self.next + i) % n];
        }
    }
}
