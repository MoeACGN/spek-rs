// Column spectrogram. Window functions and the per-column averaging rule follow Spek:
// non-overlapping FFTs of `nfft` samples, averaged inside each pixel column.
// Magnitude is 10*log10(|X|^2 / N^2), the same unnormalised scale Spek used.

use realfft::{RealFftPlanner, RealToComplex};
use rustfft::num_complex::Complex;

use crate::palette::{render_rgba, save_png, Palette, RgbaImage};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowKind {
    Hann,
    Hamming,
    BlackmanHarris,
}

impl WindowKind {
    pub fn next(self) -> Self {
        match self {
            Self::Hann => Self::Hamming,
            Self::Hamming => Self::BlackmanHarris,
            Self::BlackmanHarris => Self::Hann,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Self::Hann => Self::BlackmanHarris,
            Self::Hamming => Self::Hann,
            Self::BlackmanHarris => Self::Hamming,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Hann => "Hann",
            Self::Hamming => "Hamming",
            Self::BlackmanHarris => "Blackman-Harris",
        }
    }
}

pub struct Spectrum {
    pub width: usize,
    pub bands: usize,
    pub sample_rate: u32,
    pub db: Vec<f32>,
}

/// Walks samples in Spek's column order and can stop after a budget of new
/// columns, so a long file can be drawn while it is still being decoded.
pub struct ColumnPump {
    total: usize,
    pub width: usize,
    pub bands: usize,
    nfft: usize,
    frames_per: i64,
    error_per: i64,
    error_base: i64,
    n2: f32,
    pub sample_rate: u32,
    fft: std::sync::Arc<dyn RealToComplex<f32>>,
    input: Vec<f32>,
    output: Vec<rustfft::num_complex::Complex<f32>>,
    win: Vec<f32>,
    db: Vec<f32>,
    acc: Vec<f32>,
    num_fft: i64,
    frames: i64,
    acc_error: i64,
    col: usize,
    index: usize,
}

impl ColumnPump {
    pub fn new(
        total_samples: usize,
        sample_rate: u32,
        width: usize,
        nfft: usize,
        window: WindowKind,
    ) -> Result<Self, String> {
        if total_samples == 0 {
            return Err("沒有取樣資料".into());
        }
        if nfft < 16 || !nfft.is_power_of_two() {
            return Err("FFT 長度必須是 16 以上的 2 的次方".into());
        }
        if sample_rate == 0 {
            return Err("取樣率無效".into());
        }
        let mut width = width.max(1);
        if total_samples < width {
            width = total_samples;
        }
        let total = total_samples as i64;
        let columns = width as i64;
        let frames_per = total / columns;
        let error_per = total % columns;
        if frames_per <= 0 {
            return Err("音訊太短，無法分成頻譜欄".into());
        }
        let bands = nfft / 2 + 1;
        let mut planner = RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(nfft);
        Ok(Self {
            total: total_samples,
            width,
            bands,
            nfft,
            frames_per,
            error_per,
            error_base: columns,
            n2: (nfft as f32) * (nfft as f32),
            sample_rate,
            input: fft.make_input_vec(),
            output: fft.make_output_vec(),
            fft,
            win: window_table(window, nfft),
            db: vec![-200.0f32; width * bands],
            acc: vec![0.0f32; bands],
            num_fft: 0,
            frames: 0,
            acc_error: 0,
            col: 0,
            index: 0,
        })
    }

    pub fn columns_done(&self) -> usize {
        self.col
    }

    pub fn db(&self) -> &[f32] {
        &self.db
    }

    pub fn column_db(&self, from: usize, to: usize) -> &[f32] {
        let from = from.min(self.col).min(self.width);
        let to = to.min(self.col).min(self.width).max(from);
        &self.db[from * self.bands..to * self.bands]
    }

    /// Process newly available samples. `max_new_cols` caps how many columns
    /// are finished in this call so the caller can publish them before continuing.
    pub fn pump(&mut self, samples: &[f32], max_new_cols: usize) -> Result<usize, String> {
        if max_new_cols == 0 {
            return Ok(0);
        }
        let start_col = self.col;
        let limit = samples.len().min(self.total);
        while self.index < limit && self.col < self.width {
            if self.col - start_col >= max_new_cols {
                break;
            }
            let i = self.index;
            self.index += 1;
            self.frames += 1;
            let int_full = self.acc_error < self.error_base && self.frames == self.frames_per;
            let int_over = self.acc_error >= self.error_base && self.frames == self.frames_per + 1;
            if self.frames % self.nfft as i64 == 0 || ((int_full || int_over) && self.num_fft == 0)
            {
                let end = i + 1;
                let start = end as isize - self.nfft as isize;
                for k in 0..self.nfft {
                    let idx = start + k as isize;
                    let sample = if idx >= 0 {
                        samples.get(idx as usize).copied().unwrap_or(0.0)
                    } else {
                        0.0
                    };
                    self.input[k] = sample * self.win[k];
                }
                self.fft
                    .process(&mut self.input, &mut self.output)
                    .map_err(|err| err.to_string())?;
                accumulate(&self.output, &mut self.acc, self.n2);
                self.num_fft += 1;
            }
            if int_full || int_over {
                if int_over {
                    self.acc_error -= self.error_base;
                } else {
                    self.acc_error += self.error_per;
                }
                if self.num_fft > 0 {
                    let inv = 1.0 / self.num_fft as f32;
                    let base = self.col * self.bands;
                    for (band, bin) in self.acc.iter().enumerate() {
                        self.db[base + band] = *bin * inv;
                    }
                }
                self.acc.fill(0.0);
                self.num_fft = 0;
                self.frames = 0;
                self.col += 1;
            }
        }
        Ok(self.col - start_col)
    }
}

/// Resample one spectrum into a fixed Hertz window.
/// Row 0 is `lo_hz`. Rows past Nyquist stay at -200 dB so two files can share
/// the same vertical scale even when one of them does not reach the top.
/// Returns `None` when the window is the full 0..=Nyquist span (use `db` as-is).
pub fn window_db(
    db: &[f32],
    width: usize,
    bands: usize,
    sample_rate: u32,
    lo_hz: f32,
    hi_hz: f32,
) -> Option<(Vec<f32>, usize)> {
    if width == 0 || bands < 2 || sample_rate == 0 || db.len() < width * bands {
        return None;
    }
    let nyquist = sample_rate as f32 / 2.0;
    if !(nyquist.is_finite() && nyquist > 0.0) {
        return None;
    }
    let mut lo = if lo_hz.is_finite() { lo_hz } else { 0.0 };
    let mut hi = if hi_hz.is_finite() { hi_hz } else { nyquist };
    if hi < lo {
        std::mem::swap(&mut lo, &mut hi);
    }
    if hi - lo < 1.0 {
        hi = lo + 1.0;
    }
    if lo <= 0.01 && (hi - nyquist).abs() <= 0.5 {
        return None;
    }
    let max_bin = (bands - 1) as f32;
    let bin_lo = (lo / nyquist) * max_bin;
    let bin_hi = (hi / nyquist) * max_bin;
    let out_h = ((bin_hi - bin_lo).round().max(1.0) as usize + 1).clamp(2, 4096);
    let mut out = vec![-200.0f32; width * out_h];
    let last = (out_h - 1) as f32;
    for row in 0..out_h {
        let hz = lo + (hi - lo) * (row as f32 / last);
        if hz > nyquist {
            continue;
        }
        let bin = ((hz / nyquist) * max_bin).clamp(0.0, max_bin);
        let i0 = (bin.floor() as usize).min(bands - 1);
        let i1 = (i0 + 1).min(bands - 1);
        let t = if i0 == i1 { 0.0 } else { bin - i0 as f32 };
        for x in 0..width {
            let base = x * bands;
            let a = db[base + i0];
            let b = db[base + i1];
            out[x * out_h + row] = a + (b - a) * t;
        }
    }
    Some((out, out_h))
}

pub fn analyze(
    samples: &[f32],
    sample_rate: u32,
    width: usize,
    nfft: usize,
    window: WindowKind,
) -> Result<Spectrum, String> {
    let mut pump = ColumnPump::new(samples.len(), sample_rate, width, nfft, window)?;
    pump.pump(samples, usize::MAX)?;
    Ok(Spectrum {
        width: pump.width,
        bands: pump.bands,
        sample_rate,
        db: pump.db,
    })
}

pub fn render_spectrum(
    spec: &Spectrum,
    palette: Palette,
    urange: f32,
    lrange: f32,
) -> RgbaImage {
    render_rgba(&spec.db, spec.width, spec.bands, palette, urange, lrange)
}

pub fn write_png(
    path: &std::path::Path,
    samples: &[f32],
    sample_rate: u32,
    width: usize,
    nfft: usize,
    window: WindowKind,
    palette: Palette,
    urange: f32,
    lrange: f32,
) -> Result<(), String> {
    let spec = analyze(samples, sample_rate, width, nfft, window)?;
    let image = render_spectrum(&spec, palette, urange, lrange);
    save_png(path, &image)
}

fn accumulate(output: &[Complex<f32>], acc: &mut [f32], n2: f32) {
    for (bin, acc) in output.iter().zip(acc.iter_mut()) {
        let mag = (bin.re * bin.re + bin.im * bin.im) / n2;
        *acc += if mag <= 1.0e-20 {
            -200.0
        } else {
            10.0 * mag.log10()
        };
    }
}

fn window_table(kind: WindowKind, n: usize) -> Vec<f32> {
    let cf = std::f32::consts::TAU / (n as f32 - 1.0);
    let coss: Vec<f32> = (0..n).map(|i| (cf * i as f32).cos()).collect();
    (0..n)
        .map(|i| match kind {
            WindowKind::Hann => 0.5 * (1.0 - coss[i]),
            WindowKind::Hamming => 0.53836 - 0.46164 * coss[i],
            WindowKind::BlackmanHarris => {
                0.35875 - 0.48829 * coss[i] + 0.14128 * coss[(2 * i) % n]
                    - 0.01168 * coss[(3 * i) % n]
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sine_peaks_near_1khz() {
        let sr = 44_100u32;
        let n = sr as usize;
        let freq = 1000.0f32;
        let samples: Vec<f32> = (0..n)
            .map(|i| (std::f32::consts::TAU * freq * i as f32 / sr as f32).sin() * 0.8)
            .collect();
        let spec = analyze(&samples, sr, 16, 2048, WindowKind::Hann).unwrap();
        let mut acc = vec![0.0f32; spec.bands];
        for x in 0..spec.width {
            for b in 0..spec.bands {
                acc[b] += spec.db[x * spec.bands + b];
            }
        }
        let peak = (8..spec.bands)
            .max_by(|&a, &b| acc[a].partial_cmp(&acc[b]).unwrap())
            .unwrap();
        let expected = (freq * 2048.0 / sr as f32).round() as usize;
        assert!(
            (peak as isize - expected as isize).abs() <= 1,
            "peak bin {peak} expected {expected} db {}",
            acc[peak] / spec.width as f32
        );
        assert!(acc[peak] / spec.width as f32 > -40.0);
    }

    #[test]
    fn rendered_sine_is_bright_on_its_bin() {
        use crate::palette::Palette;
        let sr = 44_100u32;
        let samples: Vec<f32> = (0..sr as usize)
            .map(|i| (std::f32::consts::TAU * 1000.0 * i as f32 / sr as f32).sin() * 0.8)
            .collect();
        let spec = analyze(&samples, sr, 32, 2048, WindowKind::Hann).unwrap();
        let image = render_spectrum(&spec, Palette::Sox, 0.0, -120.0);
        let expected = (1000.0 * 2048.0 / sr as f32).round() as usize;
        let row = spec.bands - 1 - expected;
        let x = spec.width / 2;
        let at = |row: usize| {
            let i = (row * spec.width + x) * 4;
            image.rgba[i] as u16 + image.rgba[i + 1] as u16 + image.rgba[i + 2] as u16
        };
        assert!(at(row) > at(2) + 40, "tone {} quiet {}", at(row), at(2));
    }

    #[test]
    fn incremental_columns_match_one_shot() {
        let sr = 44_100u32;
        let samples: Vec<f32> = (0..sr as usize)
            .map(|i| (std::f32::consts::TAU * 1000.0 * i as f32 / sr as f32).sin() * 0.8)
            .collect();
        let spec = analyze(&samples, sr, 40, 1024, WindowKind::Hann).unwrap();
        let mut pump = ColumnPump::new(samples.len(), sr, 40, 1024, WindowKind::Hann).unwrap();
        let mut got = 0usize;
        let mut drawn = 0usize;
        while got < samples.len() || drawn < spec.width {
            got = (got + 4096).min(samples.len());
            let before = pump.columns_done();
            pump.pump(&samples[..got], 3).unwrap();
            drawn = pump.columns_done();
            if drawn == before && got == samples.len() {
                break;
            }
        }
        assert_eq!(pump.columns_done(), spec.width);
        assert_eq!(pump.db(), spec.db.as_slice());
    }

    #[test]
    fn shared_frequency_window_lines_up() {
        let mut low = vec![-200.0f32; 1025];
        let bin = (1000.0f32 / 22_050.0 * 1024.0).round() as usize;
        low[bin] = 0.0;
        let (db, h) = window_db(&low, 1, 1025, 44_100, 0.0, 20_000.0).unwrap();
        let row = max_row(&db);
        let frac = row as f32 / (h - 1) as f32;
        assert!((frac - 0.05).abs() < 0.02, "44.1k fraction {frac}");

        let mut high = vec![-200.0f32; 1025];
        let bin = (1000.0f32 / 48_000.0 * 1024.0).round() as usize;
        high[bin] = 0.0;
        let (db, h) = window_db(&high, 1, 1025, 96_000, 0.0, 20_000.0).unwrap();
        let row = max_row(&db);
        let frac = row as f32 / (h - 1) as f32;
        assert!((frac - 0.05).abs() < 0.02, "96k fraction {frac}");

        assert!(window_db(&low, 1, 1025, 44_100, 0.0, 22_050.0).is_none());
        let (above, h) = window_db(&low, 1, 1025, 44_100, 0.0, 40_000.0).unwrap();
        assert!(above[h - 1] < -100.0, "past nyquist stays dark");
        let tone = max_row(&above);
        let frac = tone as f32 / (h - 1) as f32;
        assert!(
            (frac - 1000.0 / 40_000.0).abs() < 0.02,
            "stretched {frac}"
        );
    }

    fn max_row(db: &[f32]) -> usize {
        db.iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap()
            .0
    }
}
