//! Whole-screen color adjustments, as Control Center's Display page makes them: a color matrix over everything on
//! screen, through the Magnification API's full-screen color effect. Windows drops the effect when this process
//! exits, so the screen never stays tinted after the app closes or crashes.

use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Result, bail};
use windows::Win32::UI::Magnification::{MAGCOLOREFFECT, MagInitialize, MagSetFullscreenColorEffect};

use crate::settings::DisplayValues;

/// A color matrix as the Magnification API takes it: rows are the input red, green, blue, alpha and a constant 1;
/// columns are the output red, green, blue, alpha and unused.
pub type Matrix = [[f32; 5]; 5];

pub const IDENTITY: Matrix = [
    [1.0, 0.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 0.0, 1.0],
];

/// The neutral color temperature: no change.
pub const NEUTRAL_KELVIN: i32 = 6500;

fn multiply(a: &Matrix, b: &Matrix) -> Matrix {
    let mut out = [[0.0; 5]; 5];
    for (i, row) in out.iter_mut().enumerate() {
        for (j, cell) in row.iter_mut().enumerate() {
            *cell = (0..5).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    out
}

/// Saturation `s` (1 = unchanged, 0 = gray), with the luminance weights Control Center uses.
fn saturation(s: f32) -> Matrix {
    let (r, g, b) = (0.3086 * (1.0 - s), 0.6094 * (1.0 - s), 0.0820 * (1.0 - s));
    let mut m = IDENTITY;
    m[0][..3].copy_from_slice(&[r + s, r, r]);
    m[1][..3].copy_from_slice(&[g, g + s, g]);
    m[2][..3].copy_from_slice(&[b, b, b + s]);
    m
}

/// Contrast `c` (1 = unchanged) around the middle gray.
fn contrast(c: f32) -> Matrix {
    let mut m = IDENTITY;
    for i in 0..3 {
        m[i][i] = c;
        m[4][i] = (1.0 - c) / 2.0;
    }
    m
}

fn gains(red: f32, green: f32, blue: f32) -> Matrix {
    let mut m = IDENTITY;
    m[0][0] = red;
    m[1][1] = green;
    m[2][2] = blue;
    m
}

/// A light source's color at `kelvin`, 0-1 per channel (Tanner Helland's fit to the blackbody curve).
fn blackbody(kelvin: f32) -> [f32; 3] {
    let t = kelvin / 100.0;
    let red = if t <= 66.0 { 255.0 } else { 329.698_73 * (t - 60.0).powf(-0.133_204_76) };
    let green = if t <= 66.0 { 99.470_8 * t.ln() - 161.119_57 } else { 288.122_17 * (t - 60.0).powf(-0.075_514_85) };
    let blue = if t >= 66.0 {
        255.0
    } else if t <= 19.0 {
        0.0
    } else {
        138.517_73 * (t - 10.0).ln() - 305.044_8
    };
    [red, green, blue].map(|v| v.clamp(0.0, 255.0) / 255.0)
}

/// How much of each channel to keep for a color temperature, relative to 6500 K. The strongest channel keeps all of
/// it, so nothing gets brighter than it was.
fn temperature(kelvin: i32) -> [f32; 3] {
    let (warm, neutral) = (blackbody(kelvin as f32), blackbody(NEUTRAL_KELVIN as f32));
    let relative = [0, 1, 2].map(|i| warm[i] / neutral[i].max(0.001));
    let strongest = relative.iter().copied().fold(0.001, f32::max);
    relative.map(|v| v / strongest)
}

/// The matrix for a set of adjustments: saturation, then contrast, then the color temperature and channel levels.
pub fn matrix(values: &DisplayValues) -> Matrix {
    let percent = |v: i32| v as f32 / 100.0;
    let [r, g, b] = temperature(values.temperature);
    let channels = gains(r * percent(values.red), g * percent(values.green), b * percent(values.blue));
    multiply(&multiply(&saturation(percent(values.saturation)), &contrast(percent(values.contrast))), &channels)
}

static INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Puts a color matrix over the whole screen; `IDENTITY` takes it away. Called on the UI thread.
pub fn apply(matrix: &Matrix) -> Result<()> {
    if !INITIALIZED.load(Ordering::Acquire) {
        if !unsafe { MagInitialize() }.as_bool() {
            bail!("Windows' magnification service couldn't start");
        }
        INITIALIZED.store(true, Ordering::Release);
    }
    let effect = MAGCOLOREFFECT { transform: matrix.concat().try_into().unwrap_or([0.0; 25]) };
    if !unsafe { MagSetFullscreenColorEffect(&effect) }.as_bool() {
        bail!("Windows didn't take the color adjustment");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: &Matrix, b: &Matrix) -> bool {
        a.iter().flatten().zip(b.iter().flatten()).all(|(x, y)| (x - y).abs() < 0.002)
    }

    #[test]
    fn neutral_is_identity_and_adjustments_act() {
        let neutral = DisplayValues::default();
        assert!(close(&matrix(&neutral), &IDENTITY));

        // No saturation: every output channel is the same mix of the inputs.
        let gray = matrix(&DisplayValues { saturation: 0, ..neutral });
        assert!(gray.iter().take(3).all(|row| (row[0] - row[1]).abs() < 1e-6 && (row[1] - row[2]).abs() < 1e-6));

        // Warmer: blue drops most, red keeps everything.
        let warm = matrix(&DisplayValues { temperature: 4500, ..neutral });
        assert!((warm[0][0] - 1.0).abs() < 1e-3 && warm[2][2] < warm[1][1] && warm[1][1] < 1.0);

        // More contrast: darks go darker (a negative offset).
        let punchy = matrix(&DisplayValues { contrast: 120, ..neutral });
        assert!(punchy[4][0] < 0.0 && punchy[0][0] > 1.0);
    }
}
