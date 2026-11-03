use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use ab_glyph::{Font, FontRef, Glyph, PxScale, ScaleFont, point};
use spek_core::{map_color, save_png, ColumnPump, Palette, RgbaImage, WindowKind};

/// Screen plot size times `scale`, capped so one side stays within 16384 px.
pub fn plot_pixels(screen_w: usize, screen_h: usize, scale: f32) -> (usize, usize, bool) {
    let max = 16_384.0_f32;
    let mut w = (screen_w.max(1) as f32 * scale).round().max(8.0);
    let mut h = (screen_h.max(1) as f32 * scale).round().max(8.0);
    let fit = (max / w).min(max / h).min(1.0);
    let clamped = fit < 1.0;
    w = (w * fit).round().max(8.0);
    h = (h * fit).round().max(8.0);
    (w as usize, h as usize, clamped)
}

pub struct Figure {
    pub screen_w: usize,
    pub screen_h: usize,
    pub scale: f32,
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    pub nfft: usize,
    pub window: WindowKind,
    pub palette: Palette,
    pub urange: f32,
    pub lrange: f32,
    pub freq_lo: f32,
    pub freq_hi: f32,
    pub title: String,
    pub meta: String,
    pub freq: [String; 5],
    pub db: [String; 5],
    pub time: [String; 5],
}

struct Canvas {
    w: usize,
    h: usize,
    px: Vec<u8>,
}

impl Canvas {
    fn new(w: usize, h: usize, rgb: [u8; 3]) -> Self {
        let mut px = vec![255u8; w * h * 4];
        for chunk in px.chunks_exact_mut(4) {
            chunk[0] = rgb[0];
            chunk[1] = rgb[1];
            chunk[2] = rgb[2];
        }
        Self { w, h, px }
    }

    fn blend(&mut self, x: i32, y: i32, rgb: [u8; 3], coverage: f32) {
        if x < 0 || y < 0 || x as usize >= self.w || y as usize >= self.h {
            return;
        }
        let a = coverage.clamp(0.0, 1.0);
        if a <= 0.0 {
            return;
        }
        let i = (y as usize * self.w + x as usize) * 4;
        for channel in 0..3 {
            let dst = self.px[i + channel] as f32;
            self.px[i + channel] = (dst * (1.0 - a) + rgb[channel] as f32 * a) as u8;
        }
    }

    fn fill_rect(&mut self, x: usize, y: usize, w: usize, h: usize, rgb: [u8; 3]) {
        for row in y..y.saturating_add(h).min(self.h) {
            for col in x..x.saturating_add(w).min(self.w) {
                let i = (row * self.w + col) * 4;
                self.px[i] = rgb[0];
                self.px[i + 1] = rgb[1];
                self.px[i + 2] = rgb[2];
                self.px[i + 3] = 255;
            }
        }
    }
}

pub struct Poster {
    pub image: RgbaImage,
    #[allow(dead_code)]
    pub plot_x: usize,
    #[allow(dead_code)]
    pub plot_y: usize,
    #[allow(dead_code)]
    pub plot_w: usize,
    #[allow(dead_code)]
    pub plot_h: usize,
}

pub fn write_figure(
    fig: &Figure,
    path: &Path,
    cancel: &AtomicBool,
    mut progress: impl FnMut(f32),
) -> Result<String, String> {
    let poster = render(fig, cancel, &mut progress)?;
    if cancel.load(Ordering::Relaxed) {
        return Err("已取消".into());
    }
    progress(0.98);
    save_png(path, &poster.image)?;
    let (_, _, clamped) = plot_pixels(fig.screen_w, fig.screen_h, fig.scale);
    let note = if clamped {
        format!(
            "已儲存 {}（{}×{}，已限制在單邊 16384 像素內）",
            path.display(),
            poster.image.width,
            poster.image.height
        )
    } else {
        format!(
            "已儲存 {}（{}×{}）",
            path.display(),
            poster.image.width,
            poster.image.height
        )
    };
    Ok(note)
}

pub fn render(
    fig: &Figure,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Result<Poster, String> {
    let (plot_w, plot_h, _) = plot_pixels(fig.screen_w, fig.screen_h, fig.scale);
    if fig.samples.is_empty() || fig.sample_rate == 0 {
        return Err("沒有可繪製的音訊".into());
    }
    if !(fig.freq_hi > fig.freq_lo) {
        return Err("頻率範圍無效".into());
    }
    let mut pump = ColumnPump::new(
        fig.samples.len(),
        fig.sample_rate,
        plot_w,
        fig.nfft,
        fig.window,
    )?;
    let total = fig.samples.len();
    let mut got = 0usize;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("已取消".into());
        }
        if pump.columns_done() >= pump.width {
            break;
        }
        if got < total {
            got = (got + 200_000).min(total);
            pump.pump(&fig.samples[..got], 48)?;
        } else {
            pump.pump(&fig.samples, usize::MAX)?;
            break;
        }
        let frac = pump.columns_done() as f32 / pump.width.max(1) as f32;
        progress(frac * 0.86);
    }
    if cancel.load(Ordering::Relaxed) {
        return Err("已取消".into());
    }
    let font = FontRef::try_from_slice(crate::fonts::source_han()).map_err(|err| err.to_string())?;
    let scale = fig.scale.max(0.25);
    let ink = [186, 198, 210];
    let title_px = 15.0 * scale;
    let body_px = 12.5 * scale;
    let pad = (10.0 * scale).round().max(4.0) as usize;
    let gap = (8.0 * scale).round().max(2.0) as usize;
    let bar_w = (14.0 * scale).round().max(4.0) as usize;
    let freq_w = fig
        .freq
        .iter()
        .map(|text| text_width(&font, text, body_px))
        .fold(0.0_f32, f32::max);
    let db_w = fig
        .db
        .iter()
        .map(|text| text_width(&font, text, body_px))
        .fold(0.0_f32, f32::max);
    let left = freq_w.ceil() as usize + pad;
    let db_col = db_w.ceil() as usize + pad;
    let text_h = (body_px * 1.35).ceil().max(1.0) as usize;
    let bottom = text_h + pad;
    let figure_w = left + plot_w + gap + bar_w + db_col;
    let text_max = (figure_w - pad * 2).max(8) as f32;
    let title_lines = wrap_lines(&font, &fig.title, title_px, text_max);
    let meta_lines = wrap_lines(&font, &fig.meta, body_px, text_max);
    let title_step = title_px * 1.35;
    let body_step = body_px * 1.35;
    let header = pad
        + (title_lines.len() as f32 * title_step).ceil() as usize
        + 4
        + (meta_lines.len() as f32 * body_step).ceil() as usize
        + pad;
    let width = figure_w;
    let height = header + plot_h + bottom;
    let mut canvas = Canvas::new(width, height, [16, 20, 26]);
    for (index, line) in title_lines.iter().enumerate() {
        draw_text(
            &mut canvas,
            &font,
            line,
            pad as f32,
            pad as f32 + index as f32 * title_step,
            title_px,
            [232, 238, 244],
        );
    }
    let meta_top = pad as f32 + title_lines.len() as f32 * title_step + 4.0;
    for (index, line) in meta_lines.iter().enumerate() {
        draw_text(
            &mut canvas,
            &font,
            line,
            pad as f32,
            meta_top + index as f32 * body_step,
            body_px,
            ink,
        );
    }
    let plot_x = left;
    let plot_y = header;
    let cols = pump.width;
    let bands = pump.bands;
    let rate = pump.sample_rate;
    paint_spectrum(
        &mut canvas,
        pump.db(),
        cols,
        bands,
        rate,
        fig,
        plot_x,
        plot_y,
        plot_w,
        plot_h,
        cancel,
        progress,
    )?;
    stroke(&mut canvas, plot_x, plot_y, plot_w, plot_h, [70, 84, 98]);
    paint_bar(
        &mut canvas,
        plot_x + plot_w + gap,
        plot_y,
        bar_w,
        plot_h,
        fig.palette,
    );
    let label_pad = (4.0 * scale).max(1.0);
    for step in 0..5 {
        let t = step as f32 / 4.0;
        let y = plot_y as f32 + plot_h as f32 - t * plot_h as f32;
        let align_y = if step == 0 {
            y - text_h as f32
        } else if step == 4 {
            y
        } else {
            y - text_h as f32 * 0.5
        };
        let text = &fig.freq[step];
        let tw = text_width(&font, text, body_px);
        draw_text(
            &mut canvas,
            &font,
            text,
            plot_x as f32 - label_pad - tw,
            align_y,
            body_px,
            ink,
        );
        let db_text = &fig.db[step];
        let db_y = plot_y as f32 + t * plot_h as f32;
        let db_top = if step == 0 {
            db_y
        } else if step == 4 {
            db_y - text_h as f32
        } else {
            db_y - text_h as f32 * 0.5
        };
        draw_text(
            &mut canvas,
            &font,
            db_text,
            (plot_x + plot_w + gap + bar_w) as f32 + label_pad,
            db_top,
            body_px,
            ink,
        );
        let time = &fig.time[step];
        let time_w = text_width(&font, time, body_px);
        let x = plot_x as f32 + t * plot_w as f32;
        let time_x = if step == 0 {
            x
        } else if step == 4 {
            x - time_w
        } else {
            x - time_w * 0.5
        };
        draw_text(
            &mut canvas,
            &font,
            time,
            time_x,
            (plot_y + plot_h) as f32 + label_pad,
            body_px,
            ink,
        );
    }
    progress(0.96);
    Ok(Poster {
        image: RgbaImage {
            width: canvas.w,
            height: canvas.h,
            rgba: canvas.px,
        },
        plot_x,
        plot_y,
        plot_w,
        plot_h,
    })
}

fn paint_spectrum(
    canvas: &mut Canvas,
    db: &[f32],
    cols: usize,
    bands: usize,
    rate: u32,
    fig: &Figure,
    origin_x: usize,
    origin_y: usize,
    plot_w: usize,
    plot_h: usize,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Result<(), String> {
    if cols == 0 || bands < 2 || rate == 0 || db.len() < cols * bands {
        return Err("頻譜資料不完整".into());
    }
    let nyquist = rate as f32 / 2.0;
    let max_bin = (bands - 1) as f32;
    let lo = fig.freq_lo;
    let hi = fig.freq_hi;
    let urange = fig.urange;
    let lrange = fig.lrange;
    let span = (urange - lrange).max(1.0e-6);
    for x in 0..plot_w {
        if x % 64 == 0 && cancel.load(Ordering::Relaxed) {
            return Err("已取消".into());
        }
        let col = (x * cols / plot_w).min(cols - 1);
        let base = col * bands;
        for y in 0..plot_h {
            let t = (y as f32 + 0.5) / plot_h as f32;
            let hz = hi - t * (hi - lo);
            let value = if hz > nyquist || hz < 0.0 {
                -200.0
            } else {
                let bin = (hz / nyquist * max_bin).clamp(0.0, max_bin);
                let i0 = bin.floor() as usize;
                let i1 = (i0 + 1).min(bands - 1);
                let mix = if i0 == i1 { 0.0 } else { bin - i0 as f32 };
                let a = db[base + i0];
                let b = db[base + i1];
                a + (b - a) * mix
            };
            let level = ((value.clamp(lrange, urange) - lrange) / span) as f64;
            let rgb = map_color(fig.palette, level);
            let i = ((origin_y + y) * canvas.w + origin_x + x) * 4;
            canvas.px[i] = rgb[0];
            canvas.px[i + 1] = rgb[1];
            canvas.px[i + 2] = rgb[2];
            canvas.px[i + 3] = 255;
        }
        if x % 128 == 0 {
            progress(0.86 + 0.1 * (x as f32 / plot_w as f32));
        }
    }
    Ok(())
}

fn paint_bar(canvas: &mut Canvas, x: usize, y: usize, w: usize, h: usize, palette: Palette) {
    if h < 2 {
        return;
    }
    for row in 0..h {
        let level = 1.0 - row as f64 / (h - 1) as f64;
        let rgb = map_color(palette, level);
        canvas.fill_rect(x, y + row, w, 1, rgb);
    }
}

fn stroke(canvas: &mut Canvas, x: usize, y: usize, w: usize, h: usize, rgb: [u8; 3]) {
    canvas.fill_rect(x, y, w, 1, rgb);
    canvas.fill_rect(x, y + h.saturating_sub(1), w, 1, rgb);
    canvas.fill_rect(x, y, 1, h, rgb);
    canvas.fill_rect(x + w.saturating_sub(1), y, 1, h, rgb);
}

fn wrap_lines(font: &FontRef<'_>, text: &str, size: f32, max_width: f32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        let mut next = current.clone();
        next.push(ch);
        if text_width(font, &next, size) > max_width && !current.is_empty() {
            lines.push(std::mem::take(&mut current));
            current.push(ch);
        } else {
            current = next;
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn text_width(font: &FontRef<'_>, text: &str, size: f32) -> f32 {
    let scale = PxScale::from(size);
    text.chars()
        .map(|ch| font.as_scaled(scale).h_advance(font.glyph_id(ch)))
        .sum()
}

fn draw_text(
    canvas: &mut Canvas,
    font: &FontRef<'_>,
    text: &str,
    x: f32,
    top: f32,
    size: f32,
    rgb: [u8; 3],
) {
    let scale = PxScale::from(size);
    let em = font.units_per_em().unwrap_or(1000.0);
    let ascent = font.ascent_unscaled() / em * size;
    let mut pen_x = x;
    let baseline = top + ascent;
    let scaled = font.as_scaled(scale);
    for ch in text.chars() {
        let id = font.glyph_id(ch);
        let glyph = Glyph {
            id,
            scale,
            position: point(pen_x, baseline),
        };
        if let Some(outlined) = font.outline_glyph(glyph) {
            let bounds = outlined.px_bounds();
            outlined.draw(|gx, gy, coverage| {
                let px = bounds.min.x + gx as f32;
                let py = bounds.min.y + gy as f32;
                canvas.blend(px.floor() as i32, py.floor() as i32, rgb, coverage);
            });
        }
        pen_x += scaled.h_advance(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_changes_pixel_size_and_caps() {
        assert_eq!(plot_pixels(100, 80, 2.0).0, 200);
        assert_eq!(plot_pixels(100, 80, 0.5).1, 40);
        let (w, h, clamped) = plot_pixels(10_000, 8_000, 8.0);
        assert!(clamped);
        assert!(w <= 16_384 && h <= 16_384);
    }

    #[test]
    fn export_redraws_to_the_screen_grid() {
        let sr = 8_000u32;
        let samples: Vec<f32> = (0..sr as usize)
            .map(|i| (std::f32::consts::TAU * 1_000.0 * i as f32 / sr as f32).sin() * 0.8)
            .collect();
        let fig = sample_figure(samples, 40, 32, 1.0);
        let cancel = AtomicBool::new(false);
        let poster = render(&fig, &cancel, &mut |_| {}).unwrap();
        assert_eq!((poster.plot_w, poster.plot_h), (40, 32));
        assert!(poster.image.width > poster.plot_w);
        assert!(poster.image.height > poster.plot_h);
        let at = |x: usize, y: usize| {
            let i = ((poster.plot_y + y) * poster.image.width + poster.plot_x + x) * 4;
            poster.image.rgba[i] as u16
                + poster.image.rgba[i + 1] as u16
                + poster.image.rgba[i + 2] as u16
        };
        let tone_y = (0.75_f32 * 31.0).round() as usize;
        assert!(
            at(20, tone_y) > at(20, 1) + 30,
            "tone {} quiet {}",
            at(20, tone_y),
            at(20, 1)
        );
        let bigger = sample_figure(
            (0..sr as usize)
                .map(|i| (std::f32::consts::TAU * 1_000.0 * i as f32 / sr as f32).sin() * 0.8)
                .collect(),
            40,
            32,
            2.0,
        );
        let poster2 = render(&bigger, &cancel, &mut |_| {}).unwrap();
        assert_eq!((poster2.plot_w, poster2.plot_h), (80, 64));
    }

    fn sample_figure(samples: Vec<f32>, w: usize, h: usize, scale: f32) -> Figure {
        Figure {
            screen_w: w,
            screen_h: h,
            scale,
            samples,
            sample_rate: 8_000,
            nfft: 512,
            window: WindowKind::Hann,
            palette: Palette::Sox,
            urange: 0.0,
            lrange: -120.0,
            freq_lo: 0.0,
            freq_hi: 4_000.0,
            title: "測試".into(),
            meta: "計算 10·log10(|X|²/N²) · 聲道 1/1 · 長度 0:01".into(),
            freq: ["0 Hz", "1 kHz", "2 kHz", "3 kHz", "4 kHz"].map(str::to_string),
            db: ["0", "-30", "-60", "-90", "-120"].map(str::to_string),
            time: ["0:00", "0:00", "0:00", "0:01", "0:01"].map(str::to_string),
        }
    }
}
