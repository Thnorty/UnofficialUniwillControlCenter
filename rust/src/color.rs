/// An 8-bit RGB color, as the controllers take it.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug, Hash)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const BLACK: Rgb = Rgb::new(0, 0, 0);
    pub const WHITE: Rgb = Rgb::new(255, 255, 255);

    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Rgb { r, g, b }
    }

    /// From a 32-bit ARGB value (.NET's Color.ToArgb()), which the settings file uses.
    pub fn from_argb(argb: i32) -> Self {
        let v = argb as u32;
        Rgb::new((v >> 16) as u8, (v >> 8) as u8, v as u8)
    }

    pub fn to_argb(self) -> i32 {
        (0xFF00_0000u32 | (self.r as u32) << 16 | (self.g as u32) << 8 | self.b as u32) as i32
    }

    pub fn is_black(self) -> bool {
        self == Rgb::BLACK
    }

    pub fn scale(self, amount: f32) -> Rgb {
        let a = amount.clamp(0.0, 1.0);
        Rgb::new((self.r as f32 * a) as u8, (self.g as f32 * a) as u8, (self.b as f32 * a) as u8)
    }

    pub fn lerp(self, other: Rgb, t: f32) -> Rgb {
        let t = t.clamp(0.0, 1.0);
        let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t) as u8;
        Rgb::new(mix(self.r, other.r), mix(self.g, other.g), mix(self.b, other.b))
    }

    /// The brighter of each channel, for light added together.
    pub fn max(self, other: Rgb) -> Rgb {
        Rgb::new(self.r.max(other.r), self.g.max(other.g), self.b.max(other.b))
    }

    pub fn distance(self, other: Rgb) -> i32 {
        (self.r as i32 - other.r as i32).abs() + (self.g as i32 - other.g as i32).abs() + (self.b as i32 - other.b as i32).abs()
    }

    pub fn to_slint(self) -> slint::Color {
        slint::Color::from_rgb_u8(self.r, self.g, self.b)
    }

    pub fn from_slint(color: slint::Color) -> Rgb {
        Rgb::new(color.red(), color.green(), color.blue())
    }

    pub fn hex(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.r, self.g, self.b)
    }

    pub fn parse_hex(text: &str) -> Option<Rgb> {
        let digits = text.trim().trim_start_matches('#');
        let digits: String = if digits.len() == 3 { digits.chars().flat_map(|c| [c, c]).collect() } else { digits.to_string() };
        if digits.len() != 6 {
            return None;
        }
        let v = u32::from_str_radix(&digits, 16).ok()?;
        Some(Rgb::new((v >> 16) as u8, (v >> 8) as u8, v as u8))
    }
}

/// A color from a palette spread over 0-1, blending between neighbours.
pub fn gradient(palette: &[Rgb], t: f32) -> Rgb {
    match palette.len() {
        0 => Rgb::BLACK,
        1 => palette[0],
        n => {
            let position = t.clamp(0.0, 1.0) * (n - 1) as f32;
            let index = (position as usize).min(n - 2);
            palette[index].lerp(palette[index + 1], position - index as f32)
        }
    }
}

/// Blue when cool, through green and yellow, to red when hot: the status effects' scale.
pub const HEAT_SCALE: [Rgb; 5] = [
    Rgb::new(0, 80, 255),
    Rgb::new(0, 255, 120),
    Rgb::new(255, 220, 0),
    Rgb::new(255, 90, 0),
    Rgb::new(255, 0, 0),
];
