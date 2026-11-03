use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use spek_core::{decode_with, AudioInfo, ColumnPump, DecodeStop, WindowKind};

#[derive(Clone)]
pub struct LiveSettings {
    pub width: usize,
    pub channel: usize,
    pub window: WindowKind,
    pub fft_bits: u32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Stamp {
    width: usize,
    nfft: usize,
    window: WindowKind,
    total: usize,
    channel: usize,
}

pub enum JobEvent {
    Progress {
        ticket: u64,
        text: String,
        fraction: f32,
        indeterminate: bool,
        rate: u32,
        duration: f64,
    },
    Columns {
        ticket: u64,
        width: usize,
        bands: usize,
        rate: u32,
        col0: usize,
        db: Vec<f32>,
        done_cols: usize,
    },
    Done {
        ticket: u64,
        info: AudioInfo,
        channels: Vec<Arc<Vec<f32>>>,
    },
    Failed {
        ticket: u64,
        message: String,
    },
    AnalyzeFinished {
        ticket: u64,
    },
    ExportProgress {
        ticket: u64,
        text: String,
        fraction: f32,
    },
    Exported {
        ticket: u64,
        path: String,
    },
    ExportFailed {
        ticket: u64,
        message: String,
    },
}

pub fn spawn_decode(
    ticket: u64,
    path: PathBuf,
    stream: usize,
    threads: u32,
    settings: Arc<Mutex<LiveSettings>>,
    cancel: Arc<AtomicBool>,
    tx: Sender<JobEvent>,
) {
    thread::spawn(move || {
        decode_job(ticket, path, stream, threads, settings, cancel, tx);
    });
}

pub fn spawn_analyze(
    ticket: u64,
    samples: Vec<f32>,
    rate: u32,
    width: usize,
    nfft: usize,
    window: WindowKind,
    cancel: Arc<AtomicBool>,
    tx: Sender<JobEvent>,
) {
    thread::spawn(move || {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        if let Err(message) = draw_all(
            ticket,
            &samples,
            rate,
            width,
            nfft,
            window,
            &cancel,
            &tx,
            "繪製頻譜",
        ) {
            let _ = tx.send(JobEvent::Failed { ticket, message });
            return;
        }
        if !cancel.load(Ordering::Relaxed) {
            let _ = tx.send(JobEvent::AnalyzeFinished { ticket });
        }
    });
}

fn decode_job(
    ticket: u64,
    path: PathBuf,
    stream: usize,
    threads: u32,
    settings: Arc<Mutex<LiveSettings>>,
    cancel: Arc<AtomicBool>,
    tx: Sender<JobEvent>,
) {
    let mut channels: Vec<Vec<f32>> = Vec::new();
    let mut preview: Vec<f32> = Vec::new();
    let mut preview_channel = usize::MAX - 1;
    let mut pump: Option<ColumnPump> = None;
    let mut stamp: Option<Stamp> = None;
    let mut last_ui = Instant::now() - Duration::from_secs(1);
    let mut last_unknown = Instant::now() - Duration::from_secs(1);
    let mut rate = 0u32;
    let mut estimated: Option<usize> = None;

    let decoded = decode_with(&path, stream, threads, &cancel, &mut |event| {
        if cancel.load(Ordering::Relaxed) {
            return false;
        }
        if event.info.sample_rate > 0 {
            rate = event.info.sample_rate;
        }
        if event.estimated_total.is_some() {
            estimated = event.estimated_total;
        }
        append_channels(&mut channels, event.fresh);
        let current = settings.lock().ok().map(|guard| guard.clone());
        let Some(current) = current else {
            return false;
        };
        if current.channel != preview_channel {
            preview = mix_channel(&channels, current.channel);
            preview_channel = current.channel;
            pump = None;
            stamp = None;
        } else {
            append_preview(&mut preview, event.fresh, current.channel);
        }
        let nfft = 1usize << current.fft_bits.min(14).max(8);
        if let Some(total) = estimated.filter(|total| *total >= 16 && current.width >= 8) {
            let next = Stamp {
                width: current.width,
                nfft,
                window: current.window,
                total,
                channel: current.channel,
            };
            if stamp != Some(next) {
                match ColumnPump::new(total, rate.max(event.info.sample_rate), current.width, nfft, current.window)
                {
                    Ok(created) => {
                        pump = Some(created);
                        stamp = Some(next);
                    }
                    Err(message) => {
                        let _ = tx.send(JobEvent::Failed { ticket, message });
                        return false;
                    }
                }
            }
            if let Some(pump) = pump.as_mut() {
                if feed_pump(
                    ticket,
                    pump,
                    &preview,
                    &cancel,
                    &tx,
                    &Bar::Decode {
                        samples: preview.len(),
                        estimated,
                    },
                )
                .is_err()
                {
                    return false;
                }
            }
        } else if current.width >= 8
            && preview.len() >= nfft
            && preview.len() <= 12_000_000
            && last_unknown.elapsed() >= Duration::from_millis(280)
        {
            last_unknown = Instant::now();
            let _ = draw_all(
                ticket,
                &preview,
                rate,
                current.width,
                nfft,
                current.window,
                &cancel,
                &tx,
                "解碼",
            );
        }
        if last_ui.elapsed() >= Duration::from_millis(80) {
            let (text, fraction, indeterminate) = decode_progress(
                preview.len(),
                estimated,
                rate,
                pump.as_ref().map(|pump| pump.columns_done()).unwrap_or(0),
                pump.as_ref().map(|pump| pump.width).unwrap_or(current.width),
            );
            let duration = estimated
                .filter(|_| rate > 0)
                .map(|total| total as f64 / rate as f64)
                .unwrap_or_else(|| {
                    if rate > 0 {
                        preview.len() as f64 / rate as f64
                    } else {
                        0.0
                    }
                });
            let _ = tx.send(JobEvent::Progress {
                ticket,
                text,
                fraction,
                indeterminate,
                rate,
                duration,
            });
            last_ui = Instant::now();
        }
        !cancel.load(Ordering::Relaxed)
    });

    if cancel.load(Ordering::Relaxed) {
        return;
    }
    let mut info = match decoded {
        Ok(info) => info,
        Err(DecodeStop::Cancelled) => return,
        Err(DecodeStop::Failed(message)) => {
            let _ = tx.send(JobEvent::Failed { ticket, message });
            return;
        }
    };
    if cancel.load(Ordering::Relaxed) {
        return;
    }
    let current = settings.lock().ok().map(|guard| guard.clone());
    if let Some(current) = current {
        if current.channel != preview_channel {
            preview = mix_channel(&channels, current.channel);
        }
        let nfft = 1usize << current.fft_bits.min(14).max(8);
        let next = Stamp {
            width: current.width,
            nfft,
            window: current.window,
            total: preview.len(),
            channel: current.channel,
        };
        let already = stamp == Some(next)
            && pump
                .as_ref()
                .is_some_and(|pump| pump.columns_done() >= pump.width && pump.width > 0);
        if !already && current.width >= 8 && preview.len() >= 16 {
            if let Err(message) = draw_all(
                ticket,
                &preview,
                info.sample_rate,
                current.width,
                nfft,
                current.window,
                &cancel,
                &tx,
                "繪製頻譜",
            ) {
                let _ = tx.send(JobEvent::Failed { ticket, message });
                return;
            }
        }
    }
    if info.duration > 0.05 {
        if let Ok(meta) = std::fs::metadata(&path) {
            info.bit_rate = Some((meta.len() as f64 * 8.0 / info.duration) as u32);
        }
    }
    let channels = channels.into_iter().map(Arc::new).collect();
    let _ = tx.send(JobEvent::Done {
        ticket,
        info,
        channels,
    });
}

fn draw_all(
    ticket: u64,
    samples: &[f32],
    rate: u32,
    width: usize,
    nfft: usize,
    window: WindowKind,
    cancel: &AtomicBool,
    tx: &Sender<JobEvent>,
    _label: &str,
) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) || width < 8 || samples.len() < 16 || rate == 0 {
        return Ok(());
    }
    let mut pump = ColumnPump::new(samples.len(), rate, width, nfft, window)?;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Ok(());
        }
        let before = pump.columns_done();
        feed_pump(ticket, &mut pump, samples, cancel, tx, &Bar::Draw)?;
        if pump.columns_done() >= pump.width || pump.columns_done() == before {
            break;
        }
    }
    Ok(())
}

enum Bar {
    Decode { samples: usize, estimated: Option<usize> },
    Draw,
}

fn feed_pump(
    ticket: u64,
    pump: &mut ColumnPump,
    samples: &[f32],
    cancel: &AtomicBool,
    tx: &Sender<JobEvent>,
    bar: &Bar,
) -> Result<(), String> {
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Ok(());
        }
        let from = pump.columns_done();
        let made = pump.pump(samples, 28)?;
        if made == 0 {
            return Ok(());
        }
        let to = pump.columns_done();
        let _ = tx.send(JobEvent::Columns {
            ticket,
            width: pump.width,
            bands: pump.bands,
            rate: pump.sample_rate,
            col0: from,
            db: pump.column_db(from, to).to_vec(),
            done_cols: to,
        });
        let (text, fraction, indeterminate) = match *bar {
            Bar::Decode { samples, estimated } => {
                decode_progress(samples, estimated, pump.sample_rate, to, pump.width)
            }
            Bar::Draw => {
                let fraction = if pump.width == 0 {
                    0.0
                } else {
                    to as f32 / pump.width as f32
                };
                (format!("繪製頻譜 {:.0}%", fraction * 100.0), fraction, false)
            }
        };
        let duration = match *bar {
            Bar::Decode { estimated, .. } => estimated
                .filter(|_| pump.sample_rate > 0)
                .map(|total| total as f64 / pump.sample_rate as f64)
                .unwrap_or_else(|| {
                    if pump.sample_rate > 0 {
                        samples.len() as f64 / pump.sample_rate as f64
                    } else {
                        0.0
                    }
                }),
            Bar::Draw => {
                if pump.sample_rate > 0 {
                    samples.len() as f64 / pump.sample_rate as f64
                } else {
                    0.0
                }
            }
        };
        let _ = tx.send(JobEvent::Progress {
            ticket,
            text,
            fraction,
            indeterminate,
            rate: pump.sample_rate,
            duration,
        });
    }
}

fn decode_progress(
    samples: usize,
    total: Option<usize>,
    rate: u32,
    cols: usize,
    width: usize,
) -> (String, f32, bool) {
    let draw = if width > 0 {
        cols as f32 / width as f32
    } else {
        0.0
    };
    let clock = |count: usize| -> String {
        if rate == 0 {
            return "0:00".into();
        }
        format_clock(count as f64 / rate as f64)
    };
    if let Some(total) = total.filter(|total| *total > 0) {
        let fraction = (samples as f32 / total as f32).clamp(0.0, 1.0);
        (
            format!(
                "解碼 {:.0}%（{} / {}）· 頻譜 {:.0}%",
                fraction * 100.0,
                clock(samples.min(total)),
                clock(total),
                draw * 100.0
            ),
            fraction,
            false,
        )
    } else {
        (
            format!("解碼中（{}）· 頻譜 {:.0}%", clock(samples), draw * 100.0),
            draw.clamp(0.0, 0.99),
            true,
        )
    }
}

fn format_clock(seconds: f64) -> String {
    if !seconds.is_finite() || seconds < 0.0 {
        return "0:00".into();
    }
    let total = seconds.round() as u64;
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

fn append_channels(channels: &mut Vec<Vec<f32>>, fresh: &[Vec<f32>]) {
    if fresh.is_empty() {
        return;
    }
    if channels.is_empty() {
        *channels = fresh.to_vec();
        return;
    }
    for (dst, src) in channels.iter_mut().zip(fresh.iter()) {
        dst.extend_from_slice(src);
    }
}

fn append_preview(preview: &mut Vec<f32>, fresh: &[Vec<f32>], channel: usize) {
    if fresh.is_empty() || fresh[0].is_empty() {
        return;
    }
    if channel == usize::MAX {
        let frames = fresh[0].len();
        let scale = 1.0 / fresh.len() as f32;
        preview.reserve(frames);
        for index in 0..frames {
            let mut sample = 0.0f32;
            for plane in fresh {
                sample += plane.get(index).copied().unwrap_or(0.0) * scale;
            }
            preview.push(sample);
        }
    } else {
        let index = channel.min(fresh.len() - 1);
        preview.extend_from_slice(&fresh[index]);
    }
}

fn mix_channel(channels: &[Vec<f32>], channel: usize) -> Vec<f32> {
    if channels.is_empty() {
        return Vec::new();
    }
    if channel == usize::MAX {
        let frames = channels.iter().map(Vec::len).min().unwrap_or(0);
        let scale = 1.0 / channels.len() as f32;
        let mut mixed = vec![0.0f32; frames];
        for plane in channels {
            for (dst, sample) in mixed.iter_mut().zip(plane.iter()) {
                *dst += *sample * scale;
            }
        }
        mixed
    } else {
        channels
            .get(channel)
            .cloned()
            .unwrap_or_else(|| channels[0].clone())
    }
}
