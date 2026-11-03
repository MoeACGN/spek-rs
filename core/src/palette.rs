// Palettes ported from Spek (spectrum, SoX, mono).

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Palette {
    Sox,
    Spectrum,
    Mono,
}

impl Palette {
    pub fn next(self) -> Self {
        match self {
            Self::Sox => Self::Spectrum,
            Self::Spectrum => Self::Mono,
            Self::Mono => Self::Sox,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Self::Sox => Self::Mono,
            Self::Spectrum => Self::Sox,
            Self::Mono => Self::Spectrum,
        }
    }
}

pub struct RgbaImage {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

pub fn map_color(palette: Palette, level: f64) -> [u8; 3] {
    let rgb = match palette {
        Palette::Spectrum => spectrum(level.clamp(0.0, 1.0)),
        Palette::Sox => sox(level.clamp(0.0, 1.0)),
        Palette::Mono => mono(level.clamp(0.0, 1.0)),
    };
    [rgb.0, rgb.1, rgb.2]
}

pub fn render_rgba(
    db: &[f32],
    width: usize,
    bands: usize,
    palette: Palette,
    urange: f32,
    lrange: f32,
) -> RgbaImage {
    let mut rgba = vec![0u8; width * bands * 4];
    let range = (urange - lrange).max(1.0e-6);
    for x in 0..width {
        for y in 0..bands {
            let value = db[x * bands + y].clamp(lrange, urange);
            let level = ((value - lrange) / range) as f64;
            let [r, g, b] = map_color(palette, level);
            let row = bands - 1 - y;
            let i = (row * width + x) * 4;
            rgba[i] = r;
            rgba[i + 1] = g;
            rgba[i + 2] = b;
            rgba[i + 3] = 255;
        }
    }
    RgbaImage {
        width,
        height: bands,
        rgba,
    }
}

pub fn save_png(path: &std::path::Path, image: &RgbaImage) -> Result<(), String> {
    image::save_buffer(
        path,
        &image.rgba,
        image.width as u32,
        image.height as u32,
        image::ColorType::Rgba8,
    )
    .map_err(|err| err.to_string())
}

fn spectrum(mut level: f64) -> (u8, u8, u8) {
    level *= 0.6625;
    let (mut r, mut g, mut b) = (0.0, 0.0, 0.0);
    if (0.0..0.15).contains(&level) {
        r = (0.15 - level) / (0.15 + 0.075);
        b = 1.0;
    } else if (0.15..0.275).contains(&level) {
        g = (level - 0.15) / (0.275 - 0.15);
        b = 1.0;
    } else if (0.275..0.325).contains(&level) {
        g = 1.0;
        b = (0.325 - level) / (0.325 - 0.275);
    } else if (0.325..0.5).contains(&level) {
        r = (level - 0.325) / (0.5 - 0.325);
        g = 1.0;
    } else if level >= 0.5 {
        r = 1.0;
        g = if level < 0.6625 {
            (0.6625 - level) / (0.6625 - 0.5)
        } else {
            0.0
        };
    }
    let mut cf = 1.0;
    if (0.0..0.1).contains(&level) {
        cf = level / 0.1;
    }
    cf *= 255.0;
    (
        (r * cf + 0.5) as u8,
        (g * cf + 0.5) as u8,
        (b * cf + 0.5) as u8,
    )
}

fn sox(level: f64) -> (u8, u8, u8) {
    let r = if (0.13..0.73).contains(&level) {
        ((level - 0.13) / 0.60 * std::f64::consts::FRAC_PI_2).sin()
    } else if level >= 0.73 {
        1.0
    } else {
        0.0
    };
    let g = if (0.6..0.91).contains(&level) {
        ((level - 0.6) / 0.31 * std::f64::consts::FRAC_PI_2).sin()
    } else if level >= 0.91 {
        1.0
    } else {
        0.0
    };
    let b = if level < 0.60 {
        0.5 * (level / 0.6 * std::f64::consts::PI).sin()
    } else if level >= 0.78 {
        (level - 0.78) / 0.22
    } else {
        0.0
    };
    (
        (r * 255.0 + 0.5) as u8,
        (g * 255.0 + 0.5) as u8,
        (b * 255.0 + 0.5) as u8,
    )
}

fn mono(level: f64) -> (u8, u8, u8) {
    let v = (level * 255.0 + 0.5) as u8;
    (v, v, v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mono_endpoints() {
        assert_eq!(map_color(Palette::Mono, 0.0), [0, 0, 0]);
        assert_eq!(map_color(Palette::Mono, 1.0), [255, 255, 255]);
    }

    #[test]
    fn sox_full_scale_is_bright() {
        let [r, g, b] = map_color(Palette::Sox, 1.0);
        assert!(r > 200 && g > 200 && b > 200);
    }
}
