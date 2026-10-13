//! What plugins can read from Windows: the temperatures, what's playing and the volume, the microphone, and the
//! notifications in the notification center. Read on the plugin thread, which has COM on (see host::run). The readers are kept between
//! calls, so a script that asks often stays cheap.

use std::cell::RefCell;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSessionManager as MediaManager, GlobalSystemMediaTransportControlsSessionPlaybackStatus as PlaybackStatus,
};
use windows::UI::Notifications::Management::{UserNotificationListener, UserNotificationListenerAccessStatus};
use windows::UI::Notifications::NotificationKinds;
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{EDataFlow, IMMDeviceEnumerator, MMDeviceEnumerator, eCapture, eMultimedia, eRender};
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};
use windows::Win32::System::Registry::{HKEY, HKEY_CURRENT_USER, KEY_READ, RRF_RT_REG_QWORD, RegCloseKey, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW};
use windows::core::{HSTRING, PCWSTR, PWSTR, w};

use crate::hardware::status::SystemStatus;
use crate::lighting::engine::Inputs;

/// What's playing, as Windows' media controls show it.
pub struct Playing {
    /// The program playing it, such as "Spotify" or "Chrome".
    pub app: String,
    pub title: String,
    pub artist: String,
    pub playing: bool,
    /// Seconds into it, and its length (0 when the program doesn't say).
    pub position: f64,
    pub duration: f64,
}

pub struct Notification {
    pub id: u32,
    /// The app's name, such as "WhatsApp".
    pub app: String,
    /// When it arrived, in seconds since 1970.
    pub time: f64,
}

#[derive(Default)]
struct Readers {
    status: SystemStatus,
    status_read: Option<Instant>,
    media: Option<MediaManager>,
    media_tried: Option<Instant>,
    volume: Option<IAudioEndpointVolume>,
    volume_found: Option<Instant>,
    microphone: Option<IAudioEndpointVolume>,
    microphone_found: Option<Instant>,
    listener: Option<UserNotificationListener>,
}

thread_local! {
    static READERS: RefCell<Readers> = RefCell::new(Readers::default());
}

/// Windows' time (100 ns steps since 1601) in seconds since 1970.
fn unix_seconds(universal: i64) -> f64 {
    (universal - 116_444_736_000_000_000) as f64 / 10_000_000.0
}

fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

/// The CPU's and the GPU's temperature in °C, read at most once a second.
pub fn temperatures() -> (Option<f32>, Option<f32>) {
    READERS.with_borrow_mut(|r| {
        if r.status_read.is_none_or(|at| at.elapsed() >= Duration::from_millis(900)) {
            r.status.update(Inputs::CPU_TEMPERATURE | Inputs::GPU_TEMPERATURE);
            r.status_read = Some(Instant::now());
        }
        (r.status.cpu_temperature, r.status.gpu_temperature)
    })
}

/// The session Windows shows in its media controls; None when nothing is open there.
pub fn playing() -> Option<Playing> {
    READERS.with_borrow_mut(|r| {
        if r.media.is_none() {
            if r.media_tried.is_some_and(|at| at.elapsed() < Duration::from_secs(10)) {
                return None;
            }
            r.media_tried = Some(Instant::now());
            r.media = MediaManager::RequestAsync().and_then(|request| request.join()).ok();
        }
        let session = r.media.as_ref()?.GetCurrentSession().ok()?;
        let info = session.GetPlaybackInfo().ok()?;
        let playing = info.PlaybackStatus().ok()? == PlaybackStatus::Playing;
        let rate = info.PlaybackRate().and_then(|rate| rate.Value()).unwrap_or(1.0);
        let (mut position, mut duration) = (0.0, 0.0);
        if let Ok(timeline) = session.GetTimelineProperties()
            && let (Ok(start), Ok(end), Ok(at)) = (timeline.StartTime(), timeline.EndTime(), timeline.Position())
        {
            duration = (end.Duration - start.Duration).max(0) as f64 / 10_000_000.0;
            position = (at.Duration - start.Duration).max(0) as f64 / 10_000_000.0;
            // Programs report the position now and then; it has moved on since while playing.
            if playing && let Ok(updated) = timeline.LastUpdatedTime() && updated.UniversalTime > 0 {
                position += (now() - unix_seconds(updated.UniversalTime)).max(0.0) * rate;
            }
            if duration > 0.0 {
                position = position.min(duration);
            }
        }
        let properties = session.TryGetMediaPropertiesAsync().and_then(|request| request.join()).ok();
        let text = |value: Option<windows::core::HSTRING>| value.map(|v| v.to_string()).unwrap_or_default();
        let app = text(session.SourceAppUserModelId().ok());
        Some(Playing {
            app: app.strip_suffix(".exe").unwrap_or(&app).to_string(),
            title: text(properties.as_ref().and_then(|p| p.Title().ok())),
            artist: text(properties.as_ref().and_then(|p| p.Artist().ok())),
            playing,
            position,
            duration,
        })
    })
}

/// The default speakers' (`eRender`) or microphone's (`eCapture`) volume control.
fn default_volume(flow: EDataFlow) -> windows::core::Result<IAudioEndpointVolume> {
    unsafe {
        let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        enumerator.GetDefaultAudioEndpoint(flow, eMultimedia)?.Activate(CLSCTX_ALL, None)
    }
}

/// The default speakers' volume (0-1) and whether they're muted. The device is looked up again every few seconds,
/// so a newly chosen one is followed.
pub fn volume() -> Option<(f32, bool)> {
    READERS.with_borrow_mut(|r| {
        if r.volume_found.is_none_or(|at| at.elapsed() >= Duration::from_secs(3)) {
            r.volume = default_volume(eRender).ok();
            r.volume_found = Some(Instant::now());
        }
        let volume = r.volume.as_ref()?;
        unsafe { Some((volume.GetMasterVolumeLevelScalar().ok()?, volume.GetMute().ok()?.as_bool())) }
    })
}

fn with_microphone<T>(f: impl FnOnce(&IAudioEndpointVolume) -> Option<T>) -> Option<T> {
    READERS.with_borrow_mut(|r| {
        if r.microphone_found.is_none_or(|at| at.elapsed() >= Duration::from_secs(3)) {
            r.microphone = default_volume(eCapture).ok();
            r.microphone_found = Some(Instant::now());
        }
        f(r.microphone.as_ref()?)
    })
}

/// Whether the default microphone is muted in Windows; None without a microphone.
pub fn microphone_muted() -> Option<bool> {
    with_microphone(|mic| unsafe { mic.GetMute().ok().map(|m| m.as_bool()) })
}

/// Mutes or unmutes the default microphone; false if there's none or Windows refused.
pub fn set_microphone_muted(muted: bool) -> bool {
    with_microphone(|mic| unsafe { mic.SetMute(muted, std::ptr::null()).ok() }).is_some()
}

/// Whether an app is using the microphone now, as Windows' privacy indicator shows it: one of the apps' usage records
/// (in the capability access manager's consent store) has started and not stopped.
pub fn microphone_in_use() -> bool {
    const STORE: &str = r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\microphone";
    [STORE.to_string(), format!(r"{STORE}\NonPackaged")].iter().any(|path| unsafe {
        let mut key = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, &HSTRING::from(path.as_str()), Some(0), KEY_READ, &mut key).is_err() {
            return false;
        }
        let mut name = [0u16; 512];
        let mut in_use = false;
        for index in 0..1000 {
            let mut length = name.len() as u32;
            if RegEnumKeyExW(key, index, Some(PWSTR(name.as_mut_ptr())), &mut length, None, None, None, None).is_err() {
                break;
            }
            let time = |value: PCWSTR| {
                let (mut data, mut size) = (0u64, 8u32);
                let read = RegGetValueW(key, PCWSTR(name.as_ptr()), value, RRF_RT_REG_QWORD, None, Some(&mut data as *mut u64 as _), Some(&mut size));
                read.is_ok().then_some(data)
            };
            if time(w!("LastUsedTimeStart")).is_some_and(|start| start > 0) && time(w!("LastUsedTimeStop")) == Some(0) {
                in_use = true;
                break;
            }
        }
        let _ = RegCloseKey(key);
        in_use
    })
}

/// The notifications in the notification center, oldest first; None when Windows doesn't let apps read them
/// (Settings › Privacy & security › Notifications).
pub fn notifications() -> Option<Vec<Notification>> {
    READERS.with_borrow_mut(|r| {
        if r.listener.is_none() {
            r.listener = UserNotificationListener::Current().ok();
        }
        let listener = r.listener.as_ref()?;
        if listener.GetAccessStatus().ok()? != UserNotificationListenerAccessStatus::Allowed {
            return None;
        }
        let list = listener.GetNotificationsAsync(NotificationKinds::Toast).and_then(|request| request.join()).ok()?;
        let mut found: Vec<Notification> = list
            .into_iter()
            .filter_map(|n| {
                let app = n.AppInfo().and_then(|info| info.DisplayInfo()).and_then(|display| display.DisplayName()).map(|name| name.to_string());
                Some(Notification { id: n.Id().ok()?, app: app.unwrap_or_default(), time: n.CreationTime().map_or(0.0, |t| unix_seconds(t.UniversalTime)) })
            })
            .collect();
        found.sort_by(|a, b| a.time.total_cmp(&b.time));
        Some(found)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Prints what this PC reads now: `cargo test readings -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn reads_this_pc() {
        let _ = unsafe { windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED) };
        println!("temperatures {:?}", temperatures());
        println!("volume {:?}", volume());
        println!("microphone muted {:?}, in use {}", microphone_muted(), microphone_in_use());
        if let Some(p) = playing() {
            println!("playing {} / {} / {} playing={} {:.1}/{:.1}", p.app, p.title, p.artist, p.playing, p.position, p.duration);
        }
        for n in notifications().unwrap_or_default() {
            println!("notification {} {} {:.0}", n.id, n.app, n.time);
        }
    }
}
