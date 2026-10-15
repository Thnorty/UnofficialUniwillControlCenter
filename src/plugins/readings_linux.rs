//! What plugins can read on Linux. Plugins come later on Linux, so nothing is read yet.

/// What's playing.
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
    /// The app's name.
    pub app: String,
    /// When it arrived, in seconds since 1970.
    pub time: f64,
}

/// The CPU's and the GPU's temperature in °C.
pub fn temperatures() -> (Option<f32>, Option<f32>) {
    (None, None)
}

pub fn playing() -> Option<Playing> {
    None
}

/// The output volume (0-1) and whether it's muted.
pub fn volume() -> Option<(f32, bool)> {
    None
}

pub fn microphone_muted() -> Option<bool> {
    None
}

pub fn set_microphone_muted(_muted: bool) -> bool {
    false
}

pub fn microphone_in_use() -> bool {
    false
}

pub fn notifications() -> Option<Vec<Notification>> {
    None
}
