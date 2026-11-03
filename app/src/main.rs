// Spek 1.5 — acoustic spectrum analyser.
// Derived from Spek by Alexander Kojevnikov and contributors.
// Licensed under GPL-3.0-or-later.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod cpu;
mod dialog;
mod fonts;
mod job;
mod poster;
mod report;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use std::path::{Path, PathBuf};
use eframe::egui;
use spek_core::{decode, render_rgba, window_db, AudioInfo, Palette, Spectrum, WindowKind};

fn main() -> eframe::Result<()> {
    report::install();
    let args = match Args::parse(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(err) => {
            report::popup(&err);
            std::process::exit(2);
        }
    };
    if args.help {
        println!("{HELP}");
        report::note(HELP);
        std::process::exit(0);
    }
    if args.version {
        println!("spek 1.5.0");
        std::process::exit(0);
    }
    if let Some(out) = args.export.clone() {
        let Some(input) = args.input.clone() else {
            report::popup("--export 需要音訊檔路徑");
            std::process::exit(2);
        };
        if let Err(err) = export_file(&input, &out, args.width, args.fft_bits, args.channel) {
            report::popup(&err);
            std::process::exit(1);
        }
        std::process::exit(0);
    }

    let icon = load_icon();
    report::note("建立視窗");
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Spek 1.5")
            .with_inner_size([1360.0, 760.0])
            .with_min_inner_size([1024.0, 560.0])
            .with_icon(icon)
            .with_visible(true)
            .with_drag_and_drop(true),
        renderer: eframe::Renderer::Glow,
        // run_app_on_demand (the default) returns immediately on some Windows 10
        // systems, so the window flashes and the process is gone. The blocking
        // event loop is the one that stays open.
        run_and_return: false,
        centered: true,
        ..Default::default()
    };
    let initial = args.input;
    let started = Instant::now();
    report::note("進入事件迴圈");
    let result = eframe::run_native(
        "Spek",
        options,
        Box::new(move |cc| Ok(Box::new(SpekApp::new(cc, initial)))),
    );
    if result.is_err() || started.elapsed() < Duration::from_millis(700) {
        let detail = match &result {
            Ok(()) => "視窗事件迴圈提早結束，沒有留下錯誤碼。".to_string(),
            Err(err) => err.to_string(),
        };
        report::popup(&format!(
            "Spek 無法維持視窗。\n{detail}\n\n已改用 OpenGL。若仍失敗，請把 spek.log 留著。"
        ));
    }
    result
}

struct Args {
    help: bool,
    version: bool,
    export: Option<PathBuf>,
    width: usize,
    fft_bits: u32,
    channel: usize,
    input: Option<PathBuf>,
}

impl Args {
    fn parse<I>(args: I) -> Result<Self, String>
    where
        I: IntoIterator<Item = String>,
    {
        let args: Vec<String> = args.into_iter().collect();
        let mut parsed = Args {
            help: false,
            version: false,
            export: None,
            width: 1280,
            fft_bits: 11,
            channel: 0,
            input: None,
        };
        let mut i = 0;
        while i < args.len() {
            let arg = &args[i];
            match arg.as_str() {
                "-h" | "--help" => parsed.help = true,
                "-V" | "--version" => parsed.version = true,
                "--export" => {
                    i += 1;
                    parsed.export = Some(PathBuf::from(need(&args, i, "--export")?));
                }
                "--width" => {
                    i += 1;
                    parsed.width = need(&args, i, "--width")?
                        .parse()
                        .map_err(|_| "--width 需要正整數".to_string())?;
                }
                "--fft" => {
                    i += 1;
                    let n: u32 = need(&args, i, "--fft")?
                        .parse()
                        .map_err(|_| "--fft 需要 256 到 16384 的 2 的次方".to_string())?;
                    if !(256..=16384).contains(&n) || !n.is_power_of_two() {
                        return Err("--fft 需要 256 到 16384 的 2 的次方".into());
                    }
                    parsed.fft_bits = n.trailing_zeros();
                }
                "--channel" => {
                    i += 1;
                    let text = need(&args, i, "--channel")?;
                    parsed.channel = if text == "mix" {
                        usize::MAX
                    } else {
                        text.parse::<usize>()
                            .map_err(|_| "--channel 需要數字或 mix".to_string())?
                            .saturating_sub(1)
                    };
                }
                other if other.starts_with('-') => {
                    return Err(format!("未知選項 {other}"));
                }
                other => parsed.input = Some(PathBuf::from(other)),
            }
            i += 1;
        }
        Ok(parsed)
    }
}

fn need(args: &[String], index: usize, flag: &str) -> Result<String, String> {
    args.get(index)
        .cloned()
        .ok_or_else(|| format!("{flag} 缺少參數"))
}

fn export_file(
    input: &Path,
    out: &Path,
    width: usize,
    fft_bits: u32,
    channel: usize,
) -> Result<(), String> {
    let decoded = decode(input, 0)?;
    let samples = take_channel(&decoded.channels, channel)?;
    spek_core::write_png(
        out,
        &samples,
        decoded.info.sample_rate,
        width.max(8),
        1usize << fft_bits,
        WindowKind::Hann,
        Palette::Sox,
        0.0,
        -120.0,
    )
}

fn take_channel(channels: &[Vec<f32>], channel: usize) -> Result<Vec<f32>, String> {
    if channels.is_empty() {
        return Err("沒有聲道".into());
    }
    if channel == usize::MAX {
        return Ok(mix(channels));
    }
    channels
        .get(channel)
        .cloned()
        .ok_or_else(|| format!("沒有第 {} 聲道", channel + 1))
}

fn mix(channels: &[Vec<f32>]) -> Vec<f32> {
    let n = channels.iter().map(Vec::len).min().unwrap_or(0);
    let scale = 1.0 / channels.len() as f32;
    let mut out = vec![0.0f32; n];
    for ch in channels {
        for (dst, sample) in out.iter_mut().zip(ch.iter()) {
            *dst += *sample * scale;
        }
    }
    out
}

fn mix_arcs(channels: &[Arc<Vec<f32>>]) -> Vec<f32> {
    let n = channels.iter().map(|ch| ch.len()).min().unwrap_or(0);
    let scale = 1.0 / channels.len() as f32;
    let mut out = vec![0.0f32; n];
    for ch in channels {
        for (dst, sample) in out.iter_mut().zip(ch.iter()) {
            *dst += *sample * scale;
        }
    }
    out
}

struct Loaded {
    info: AudioInfo,
    channels: Vec<Arc<Vec<f32>>>,
}

#[derive(Clone)]
struct ProgressUi {
    text: String,
    fraction: f32,
    indeterminate: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct AnalyzeKey {
    file: u64,
    width: usize,
    channel: usize,
    stream: usize,
    window: WindowKind,
    fft_bits: u32,
}

#[derive(PartialEq, Eq)]
struct PaintKey {
    analyze: AnalyzeKey,
    palette: Palette,
    urange: i32,
    lrange: i32,
    freq_lo: i32,
    freq_hi: i32,
}

struct SpekApp {
    tx: Sender<job::JobEvent>,
    rx: Receiver<job::JobEvent>,
    initial: Option<PathBuf>,
    opened_initial: bool,
    path: Option<PathBuf>,
    loaded: Option<Loaded>,
    spectrum: Option<Spectrum>,
    texture: Option<egui::TextureHandle>,
    bar: Option<egui::TextureHandle>,
    bar_palette: Option<Palette>,
    paint_key: Option<PaintKey>,
    analyze_key: Option<AnalyzeKey>,
    plot_width: usize,
    plot_height: usize,
    width_at: Option<Instant>,
    decode_ticket: u64,
    analyze_ticket: u64,
    file_ticket: u64,
    busy: String,
    error: Option<String>,
    notice: Option<String>,
    channel: usize,
    stream: usize,
    window: WindowKind,
    fft_bits: u32,
    palette: Palette,
    urange: i32,
    lrange: i32,
    show_help: bool,
    settings: Arc<Mutex<job::LiveSettings>>,
    cancel: Arc<AtomicBool>,
    decoding: bool,
    progress: Option<ProgressUi>,
    decode_threads: usize,
    thread_max: usize,
    cpu_note: String,
    spec_cols: usize,
    axis_rate: f32,
    axis_duration: f64,
    freq_lo: f32,
    freq_hi: f32,
    freq_custom: bool,
    export_scale: f32,
    export_ticket: u64,
    export_cancel: Arc<AtomicBool>,
    exporting: bool,
    toolbar_h: f32,
}

impl SpekApp {
    fn new(cc: &eframe::CreationContext<'_>, initial: Option<PathBuf>) -> Self {
        fonts::install(&cc.egui_ctx);
        install_style(&cc.egui_ctx);
        let (tx, rx) = mpsc::channel();
        let cpu = cpu::detect();
        let threads = cpu.performance_cores.max(1);
        let thread_max = cpu.logical.max(threads).clamp(1, 256);
        let cpu_note = if !cpu.counted_physical {
            format!("預設 {threads}：無法讀取實體核心，改用邏輯處理器數量")
        } else if cpu.hybrid {
            format!("預設 {threads}：高效能實體核心（已略過較慢的 E 核心）")
        } else {
            format!("預設 {threads}：實體核心")
        };
        report::note(&cpu_note);
        Self {
            tx,
            rx,
            initial,
            opened_initial: false,
            path: None,
            loaded: None,
            spectrum: None,
            texture: None,
            bar: None,
            bar_palette: None,
            paint_key: None,
            analyze_key: None,
            plot_width: 0,
            plot_height: 0,
            width_at: None,
            decode_ticket: 0,
            analyze_ticket: 0,
            file_ticket: 0,
            busy: String::new(),
            error: None,
            notice: None,
            channel: 0,
            stream: 0,
            window: WindowKind::Hann,
            fft_bits: 11,
            palette: Palette::Sox,
            urange: 0,
            lrange: -120,
            show_help: false,
            settings: Arc::new(Mutex::new(job::LiveSettings {
                width: 0,
                channel: 0,
                window: WindowKind::Hann,
                fft_bits: 11,
            })),
            cancel: Arc::new(AtomicBool::new(false)),
            decoding: false,
            progress: None,
            decode_threads: threads,
            thread_max,
            cpu_note,
            spec_cols: 0,
            axis_rate: 0.0,
            axis_duration: 0.0,
            freq_lo: 0.0,
            freq_hi: 20_000.0,
            freq_custom: false,
            export_scale: 1.0,
            export_ticket: 0,
            export_cancel: Arc::new(AtomicBool::new(false)),
            exporting: false,
            toolbar_h: 108.0,
        }
    }

    fn open_file(&mut self, path: PathBuf, stream: usize) {
        self.cancel_export();
        self.cancel.store(true, Ordering::Relaxed);
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = cancel.clone();
        self.path = Some(path.clone());
        self.stream = stream;
        self.error = None;
        self.notice = None;
        self.loaded = None;
        self.spectrum = None;
        self.texture = None;
        self.spec_cols = 0;
        self.analyze_key = None;
        self.paint_key = None;
        self.axis_rate = 0.0;
        self.axis_duration = 0.0;
        self.decode_ticket += 1;
        self.analyze_ticket += 1;
        let ticket = self.decode_ticket;
        self.decoding = true;
        self.busy = "正在解碼…".into();
        self.progress = Some(ProgressUi {
            text: "正在解碼…".into(),
            fraction: 0.0,
            indeterminate: true,
        });
        self.push_settings();
        job::spawn_decode(
            ticket,
            path,
            stream,
            self.decode_threads as u32,
            self.settings.clone(),
            cancel,
            self.tx.clone(),
        );
    }

    fn push_settings(&mut self) {
        if let Ok(mut settings) = self.settings.lock() {
            settings.width = self.plot_width;
            settings.channel = self.channel;
            settings.window = self.window;
            settings.fft_bits = self.fft_bits;
        }
    }

    fn poll(&mut self, ctx: &egui::Context) {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                job::JobEvent::Progress {
                    ticket,
                    text,
                    fraction,
                    indeterminate,
                    rate,
                    duration,
                } => {
                    if !self.ticket_live(ticket) {
                        continue;
                    }
                    self.busy = text.clone();
                    self.progress = Some(ProgressUi {
                        text,
                        fraction,
                        indeterminate,
                    });
                    if rate > 0 {
                        self.axis_rate = rate as f32;
                    }
                    if duration > 0.0 {
                        self.axis_duration = duration;
                    }
                }
                job::JobEvent::Columns {
                    ticket,
                    width,
                    bands,
                    rate,
                    col0,
                    db,
                    done_cols,
                } => {
                    if !self.ticket_live(ticket) {
                        continue;
                    }
                    self.apply_columns(width, bands, rate, col0, &db, done_cols);
                }
                job::JobEvent::Done {
                    ticket,
                    info,
                    channels,
                } => {
                    if ticket != self.decode_ticket {
                        continue;
                    }
                    self.decoding = false;
                    self.busy.clear();
                    if self.channel != usize::MAX && self.channel >= channels.len() {
                        self.channel = 0;
                    }
                    self.stream = info.stream_index;
                    self.file_ticket = ticket;
                    self.axis_rate = info.sample_rate as f32;
                    self.axis_duration = info.duration;
                    let complete = self.spectrum_matches(info.sample_rate);
                    self.loaded = Some(Loaded { info, channels });
                    if complete {
                        self.analyze_key = Some(self.analyze_key_now());
                        self.progress = None;
                    } else {
                        self.analyze_key = None;
                        self.progress = None;
                    }
                    self.error = None;
                }
                job::JobEvent::Failed { ticket, message } => {
                    if !self.ticket_live(ticket) {
                        continue;
                    }
                    if ticket == self.decode_ticket {
                        self.decoding = false;
                    }
                    if ticket == self.analyze_ticket {
                        self.analyze_key = None;
                    }
                    self.busy.clear();
                    self.progress = None;
                    self.error = Some(message);
                }
                job::JobEvent::AnalyzeFinished { ticket } => {
                    if ticket != self.analyze_ticket {
                        continue;
                    }
                    self.busy.clear();
                    self.progress = None;
                    if self.spectrum_matches(self.axis_rate as u32) {
                        self.analyze_key = Some(self.analyze_key_now());
                    }
                }
                job::JobEvent::ExportProgress {
                    ticket,
                    text,
                    fraction,
                } => {
                    if ticket != self.export_ticket || !self.exporting {
                        continue;
                    }
                    self.busy = text.clone();
                    self.progress = Some(ProgressUi {
                        text,
                        fraction,
                        indeterminate: false,
                    });
                }
                job::JobEvent::Exported { ticket, path } => {
                    if ticket != self.export_ticket {
                        continue;
                    }
                    self.exporting = false;
                    self.busy.clear();
                    self.progress = None;
                    self.notice = Some(path);
                    self.error = None;
                }
                job::JobEvent::ExportFailed { ticket, message } => {
                    if ticket != self.export_ticket {
                        continue;
                    }
                    self.exporting = false;
                    self.busy.clear();
                    self.progress = None;
                    self.error = Some(message);
                }
            }
        }
        if self.decoding || self.progress.is_some() || self.busy_pending() {
            ctx.request_repaint_after(Duration::from_millis(30));
        }
    }

    fn ticket_live(&self, ticket: u64) -> bool {
        ticket == self.decode_ticket || ticket == self.analyze_ticket
    }

    fn busy_pending(&self) -> bool {
        self.analyze_key.is_none() && self.loaded.is_some() && self.plot_width >= 8
    }

    fn apply_columns(
        &mut self,
        width: usize,
        bands: usize,
        rate: u32,
        col0: usize,
        db: &[f32],
        done_cols: usize,
    ) {
        let mismatch = self
            .spectrum
            .as_ref()
            .map(|spec| spec.width != width || spec.bands != bands)
            .unwrap_or(true);
        if mismatch {
            self.spectrum = Some(Spectrum {
                width,
                bands,
                sample_rate: rate,
                db: vec![-200.0; width.saturating_mul(bands)],
            });
        }
        if let Some(spec) = &mut self.spectrum {
            if col0 == 0 {
                spec.db.fill(-200.0);
            }
            let start = col0.saturating_mul(bands);
            let end = (start + db.len()).min(spec.db.len());
            let copy = (end - start).min(db.len());
            spec.db[start..start + copy].copy_from_slice(&db[..copy]);
            spec.sample_rate = rate;
        }
        self.spec_cols = done_cols;
        self.paint_key = None;
        if rate > 0 {
            self.axis_rate = rate as f32;
        }
    }

    fn spectrum_matches(&self, rate: u32) -> bool {
        let Some(spec) = &self.spectrum else {
            return false;
        };
        spec.width == self.plot_width
            && self.spec_cols >= spec.width
            && spec.width > 0
            && (rate == 0 || spec.sample_rate == rate)
    }

    fn analyze_key_now(&self) -> AnalyzeKey {
        AnalyzeKey {
            file: self.file_ticket,
            width: self.plot_width,
            channel: self.channel,
            stream: self.stream,
            window: self.window,
            fft_bits: self.fft_bits,
        }
    }

    fn note_width(&mut self, width: usize, ctx: &egui::Context) {
        if width >= 8 && width != self.plot_width {
            self.plot_width = width;
            self.width_at = Some(Instant::now());
        }
        let settling = self
            .width_at
            .is_some_and(|at| at.elapsed() < Duration::from_millis(80));
        if !settling {
            self.push_settings();
        }
        if self.decoding {
            ctx.request_repaint_after(Duration::from_millis(30));
            return;
        }
        if self.plot_width < 8 || self.loaded.is_none() {
            return;
        }
        if settling {
            ctx.request_repaint_after(Duration::from_millis(30));
            return;
        }
        self.start_analyze();
    }

    fn start_analyze(&mut self) {
        if self.decoding || self.loaded.is_none() {
            return;
        }
        let key = self.analyze_key_now();
        if self.analyze_key == Some(key) {
            return;
        }
        self.cancel_export();
        self.analyze_key = Some(key);
        self.cancel.store(true, Ordering::Relaxed);
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = cancel.clone();
        self.analyze_ticket += 1;
        let ticket = self.analyze_ticket;
        self.busy = "正在繪製頻譜…".into();
        self.progress = Some(ProgressUi {
            text: "正在繪製頻譜…".into(),
            fraction: 0.0,
            indeterminate: false,
        });
        let Some(loaded) = &self.loaded else {
            return;
        };
        let samples = if self.channel == usize::MAX {
            mix_arcs(&loaded.channels)
        } else {
            match loaded.channels.get(self.channel) {
                Some(channel) => channel.as_ref().clone(),
                None => {
                    self.error = Some("聲道不存在".into());
                    self.progress = None;
                    return;
                }
            }
        };
        let rate = loaded.info.sample_rate;
        job::spawn_analyze(
            ticket,
            samples,
            rate,
            self.plot_width,
            1usize << self.fft_bits,
            self.window,
            cancel,
            self.tx.clone(),
        );
    }

    fn sync_texture(&mut self, ctx: &egui::Context) {
        let Some(spec) = &self.spectrum else {
            return;
        };
        let analyze = self.analyze_key.unwrap_or(AnalyzeKey {
            file: self.decode_ticket,
            width: spec.width,
            channel: self.channel,
            stream: self.stream,
            window: self.window,
            fft_bits: self.fft_bits,
        });
        let (flo, fhi) = self.freq_view();
        let paint = PaintKey {
            analyze,
            palette: self.palette,
            urange: self.urange,
            lrange: self.lrange,
            freq_lo: flo.round() as i32,
            freq_hi: fhi.round() as i32,
        };
        if self.paint_key != Some(paint) {
            let image = self.render_spec(spec, flo, fhi);
            let color = egui::ColorImage::from_rgba_unmultiplied(
                [image.width, image.height],
                &image.rgba,
            );
            let options = egui::TextureOptions::LINEAR;
            if let Some(tex) = &mut self.texture {
                tex.set(color, options);
            } else {
                self.texture = Some(ctx.load_texture("spectrogram", color, options));
            }
            self.paint_key = Some(PaintKey {
                analyze,
                palette: self.palette,
                urange: self.urange,
                lrange: self.lrange,
                freq_lo: flo.round() as i32,
                freq_hi: fhi.round() as i32,
            });
        }
        if self.bar_palette != Some(self.palette) {
            self.bar = Some(ctx.load_texture(
                "palette-bar",
                palette_column(self.palette),
                egui::TextureOptions::LINEAR,
            ));
            self.bar_palette = Some(self.palette);
        }
    }

    fn save_dialog(&mut self) {
        if !self.can_save() {
            self.error = Some("頻譜畫完後才能存圖".into());
            return;
        }
        let stem = self
            .path
            .as_ref()
            .and_then(|path| path.file_stem())
            .and_then(|stem| stem.to_str())
            .unwrap_or("spek");
        let Some(path) = dialog::pick_save(&format!("{stem}-spectrogram.png")) else {
            return;
        };
        self.start_export(path);
    }

    fn pick_and_open(&mut self) {
        let Some(path) = dialog::pick_open() else {
            return;
        };
        self.channel = 0;
        self.open_file(path, 0);
    }
}

impl eframe::App for SpekApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if !self.opened_initial {
            self.opened_initial = true;
            if let Some(path) = self.initial.clone() {
                self.open_file(path, 0);
            }
        }
        if let Some(path) = dropped_file(ctx) {
            self.channel = 0;
            self.open_file(path, 0);
        }
        self.poll(ctx);
        let action = shortcut_action(ctx);
        self.apply_shortcut(action);
        self.toolbar(ctx);
        self.sync_texture(ctx);
        let width = self.central(ctx);
        self.note_width(width, ctx);
        self.help_window(ctx);
        self.paint_progress(ctx);
    }
}

#[derive(Default)]
struct Shortcut {
    open: bool,
    save: bool,
    help: bool,
    channel: i32,
    window: i32,
    palette: i32,
    fft: i32,
    stream: i32,
    upper: i32,
    lower: i32,
}

fn shortcut_action(ctx: &egui::Context) -> Shortcut {
    let mut action = Shortcut::default();
    let typing = ctx.wants_keyboard_input();
    ctx.input(|input| {
        if input.modifiers.command && input.key_pressed(egui::Key::O) {
            action.open = true;
        }
        if input.modifiers.command && input.key_pressed(egui::Key::S) {
            action.save = true;
        }
        if input.key_pressed(egui::Key::F1) {
            action.help = true;
        }
        if typing || input.modifiers.command {
            return;
        }
        let dir = if input.modifiers.shift { -1 } else { 1 };
        if input.key_pressed(egui::Key::C) {
            action.channel = dir;
        } else if input.key_pressed(egui::Key::F) {
            action.window = dir;
        } else if input.key_pressed(egui::Key::P) {
            action.palette = dir;
        } else if input.key_pressed(egui::Key::W) {
            action.fft = dir;
        } else if input.key_pressed(egui::Key::S) {
            action.stream = dir;
        } else if input.key_pressed(egui::Key::U) {
            action.upper = dir;
        } else if input.key_pressed(egui::Key::L) {
            action.lower = dir;
        }
    });
    action
}

impl SpekApp {
    fn apply_shortcut(&mut self, action: Shortcut) {
        if action.open {
            self.pick_and_open();
        }
        if action.save {
            self.save_dialog();
        }
        if action.help {
            self.show_help = !self.show_help;
        }
        let channels = self.loaded.as_ref().map(|l| l.channels.len()).unwrap_or(0);
        if action.channel != 0 && channels > 0 {
            let current = if self.channel >= channels { 0 } else { self.channel };
            self.channel = if action.channel > 0 {
                (current + 1) % channels
            } else {
                (current + channels - 1) % channels
            };
        }
        if action.window > 0 {
            self.window = self.window.next();
        } else if action.window < 0 {
            self.window = self.window.prev();
        }
        if action.palette > 0 {
            self.palette = self.palette.next();
        } else if action.palette < 0 {
            self.palette = self.palette.prev();
        }
        if action.fft > 0 {
            self.fft_bits = (self.fft_bits + 1).min(14);
        } else if action.fft < 0 {
            self.fft_bits = (self.fft_bits - 1).max(8);
        }
        if action.upper > 0 {
            self.urange = (self.urange + 1).min(0);
        } else if action.upper < 0 {
            self.urange = (self.urange - 1).max(self.lrange + 1);
        }
        if action.lower > 0 {
            self.lrange = (self.lrange + 1).min(self.urange - 1);
        } else if action.lower < 0 {
            self.lrange = (self.lrange - 1).max(-140);
        }
        if action.stream != 0 {
            if let Some(loaded) = &self.loaded {
                let count = loaded.info.stream_count;
                if count > 1 {
                    let next = if action.stream > 0 {
                        (self.stream + 1) % count
                    } else {
                        (self.stream + count - 1) % count
                    };
                    if let Some(path) = self.path.clone() {
                        self.open_file(path, next);
                    }
                }
            }
        }
    }

    fn toolbar(&mut self, ctx: &egui::Context) {
        let mut open = false;
        let mut save = false;
        let mut stream_pick: Option<usize> = None;
        let panel = egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.set_min_height(32.0);
                if ui
                    .add_sized(egui::vec2(72.0, 28.0), egui::Button::new("開啟"))
                    .clicked()
                {
                    open = true;
                }
                if ui
                    .add_enabled(
                        self.can_save(),
                        egui::Button::new("另存圖片").min_size(egui::vec2(96.0, 28.0)),
                    )
                    .clicked()
                {
                    save = true;
                }
                self.export_scale_control(ui);
                if ui
                    .add_sized(egui::vec2(64.0, 28.0), egui::Button::new("說明"))
                    .clicked()
                {
                    self.show_help = true;
                }
                ui.separator();
                let name = self
                    .path
                    .as_ref()
                    .and_then(|path| path.file_name())
                    .and_then(|name| name.to_str())
                    .unwrap_or("尚未開啟檔案");
                ui.add(
                    egui::Label::new(egui::RichText::new(name).strong()).truncate(),
                );
            });
            ui.horizontal(|ui| {
                ui.set_min_height(32.0);
                let count = self.loaded.as_ref().map(|l| l.channels.len()).unwrap_or(0);
                let channel_text = if self.channel == usize::MAX {
                    "混合".to_string()
                } else if count == 0 {
                    "—".to_string()
                } else {
                    format!("{}/{}", self.channel + 1, count)
                };
                egui::ComboBox::from_label("聲道")
                    .width(72.0)
                    .selected_text(channel_text)
                    .show_ui(ui, |ui| {
                        for index in 0..count {
                            ui.selectable_value(&mut self.channel, index, format!("{}", index + 1));
                        }
                        if count > 1 {
                            ui.selectable_value(&mut self.channel, usize::MAX, "混合");
                        }
                    });
                egui::ComboBox::from_label("窗函數")
                    .width(148.0)
                    .selected_text(window_label(self.window))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.window, WindowKind::Hann, "Hann（漢恩）");
                        ui.selectable_value(&mut self.window, WindowKind::Hamming, "Hamming（漢明）");
                        ui.selectable_value(
                            &mut self.window,
                            WindowKind::BlackmanHarris,
                            "Blackman-Harris",
                        );
                    });
                egui::ComboBox::from_label("FFT")
                    .width(72.0)
                    .selected_text(format!("{}", 1u32 << self.fft_bits))
                    .show_ui(ui, |ui| {
                        for bits in 8..=14 {
                            ui.selectable_value(&mut self.fft_bits, bits, format!("{}", 1u32 << bits));
                        }
                    });
                egui::ComboBox::from_label("色彩")
                    .width(108.0)
                    .selected_text(palette_label(self.palette))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.palette, Palette::Sox, "SoX（預設）");
                        ui.selectable_value(&mut self.palette, Palette::Spectrum, "彩虹");
                        ui.selectable_value(&mut self.palette, Palette::Mono, "灰階");
                    });
                let streams = self
                    .loaded
                    .as_ref()
                    .map(|loaded| loaded.info.stream_count)
                    .unwrap_or(0);
                let mut stream = self.stream;
                ui.add_enabled_ui(streams > 1, |ui| {
                    egui::ComboBox::from_label("串流")
                        .width(88.0)
                        .selected_text(if streams > 1 {
                            format!("{}/{}", stream + 1, streams)
                        } else {
                            "1/1".to_string()
                        })
                        .show_ui(ui, |ui| {
                            for index in 0..streams {
                                ui.selectable_value(&mut stream, index, format!("串流 {}", index + 1));
                            }
                        });
                });
                if streams > 1 && stream != self.stream {
                    stream_pick = Some(stream);
                }
                let mut threads = self.decode_threads as u32;
                ui.allocate_ui_with_layout(
                    egui::vec2(64.0, 28.0),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.add(
                            egui::DragValue::new(&mut threads)
                                .range(1..=self.thread_max as u32)
                                .speed(0.15),
                        );
                    },
                );
                ui.label("解碼執行緒").on_hover_text(&self.cpu_note);
                if threads as usize != self.decode_threads {
                    self.decode_threads = threads as usize;
                    self.notice = Some(format!(
                        "解碼執行緒改為 {}，下次開啟檔案時套用",
                        self.decode_threads
                    ));
                }
            });
            ui.horizontal(|ui| {
                ui.set_min_height(32.0);
                ui.spacing_mut().slider_width = 96.0;
                ui.add(egui::Slider::new(&mut self.urange, -140..=0).text("上限 dB"));
                ui.add(egui::Slider::new(&mut self.lrange, -140..=0).text("下限 dB"));
                self.freq_controls(ui);
                if self.lrange >= self.urange {
                    self.lrange = self.urange - 1;
                }
            });
            ui.add_space(4.0);
        });
        self.toolbar_h = panel.response.rect.height();
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.set_min_height(22.0);
                ui.label(egui::RichText::new("Spek 1.5").strong());
                ui.separator();
                ui.weak("Ctrl+O 開啟 · Ctrl+S 存圖 · C 聲道 · F 窗 · P 色彩 · W FFT · F1 說明");
                if let Some(err) = &self.error {
                    ui.separator();
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(err).color(egui::Color32::from_rgb(255, 150, 130)),
                        )
                        .truncate(),
                    );
                } else if let Some(note) = &self.notice {
                    ui.separator();
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(note).color(egui::Color32::from_rgb(150, 210, 170)),
                        )
                        .truncate(),
                    );
                }
            });
            ui.horizontal(|ui| {
                ui.set_min_height(22.0);
                ui.add(egui::Label::new(self.info_line()).truncate());
            });
        });
        if open {
            self.pick_and_open();
        }
        if save {
            self.save_dialog();
        }
        if let Some(stream) = stream_pick {
            if let Some(path) = self.path.clone() {
                self.open_file(path, stream);
            }
        }
    }

    fn central(&mut self, ctx: &egui::Context) -> usize {
        let mut width = self.plot_width;
        egui::CentralPanel::default().show(ctx, |ui| {
            let avail = ui.available_size();
            let font_id = egui::FontId::proportional(12.5);
            let db_text_w = ui.fonts(|fonts| {
                let measure = |text: String| {
                    fonts
                        .layout_no_wrap(text, font_id.clone(), egui::Color32::WHITE)
                        .size()
                        .x
                };
                measure(self.urange.to_string()).max(measure(self.lrange.to_string()))
            });
            let db_w = db_text_w + 10.0;
            let gap = 8.0;
            let bar_w = 14.0;
            let bottom = 28.0;
            let mut left = 78.0;
            let mut plot_w = avail.x - left - gap - bar_w - db_w;
            if plot_w < 8.0 {
                let shrink = (8.0 - plot_w).min((left - 48.0).max(0.0));
                left -= shrink;
                plot_w = avail.x - left - gap - bar_w - db_w;
            }
            plot_w = plot_w.max(1.0);
            let plot_h = (avail.y - bottom).max(8.0);
            width = plot_w.floor() as usize;
            self.plot_height = plot_h.floor() as usize;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                let (freq_rect, _) =
                    ui.allocate_exact_size(egui::vec2(left, plot_h), egui::Sense::hover());
                let (plot, _) =
                    ui.allocate_exact_size(egui::vec2(plot_w, plot_h), egui::Sense::hover());
                let _ = ui.allocate_exact_size(egui::vec2(gap, plot_h), egui::Sense::hover());
                let (bar, _) =
                    ui.allocate_exact_size(egui::vec2(bar_w, plot_h), egui::Sense::hover());
                let (db_rect, _) =
                    ui.allocate_exact_size(egui::vec2(db_w, plot_h), egui::Sense::hover());
                self.paint(ui, plot, freq_rect, bar, db_rect);
            });
        });
        width
    }

    fn paint(
        &self,
        ui: &mut egui::Ui,
        plot: egui::Rect,
        freq: egui::Rect,
        bar: egui::Rect,
        db: egui::Rect,
    ) {
        let painter = ui.painter();
        let frame = egui::Color32::from_rgb(8, 10, 14);
        let ink = egui::Color32::from_rgb(186, 198, 210);
        let font = egui::FontId::proportional(12.5);
        if let Some(tex) = &self.texture {
            painter.image(
                tex.id(),
                plot,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        } else {
            painter.rect_filled(plot, 0.0, frame);
            let hint = if let Some(err) = &self.error {
                err.clone()
            } else if !self.busy.is_empty() {
                self.busy.clone()
            } else {
                "把音訊檔拖放到這裡，或按「開啟」".into()
            };
            painter.text(
                plot.center(),
                egui::Align2::CENTER_CENTER,
                hint,
                egui::FontId::proportional(18.0),
                ink,
            );
        }
        painter.rect_stroke(
            plot,
            0.0,
            egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(70, 84, 98)),
            egui::StrokeKind::Outside,
        );
        if let Some(tex) = &self.bar {
            painter.image(
                tex.id(),
                bar,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }

        let window = painter.clip_rect().shrink(1.0);
        let (flo, fhi) = self.freq_view();
        if fhi > flo {
            let freq_bounds = egui::Rect::from_min_max(
                egui::pos2(window.left(), plot.top().max(window.top())),
                egui::pos2(freq.right().min(window.right()), plot.bottom().min(window.bottom())),
            );
            let mut used = Vec::new();
            for step in [0, 4, 2, 1, 3] {
                let t = step as f32 / 4.0;
                let y = plot.bottom() - t * plot.height();
                let align = if step == 0 {
                    egui::Align2::RIGHT_BOTTOM
                } else if step == 4 {
                    egui::Align2::RIGHT_TOP
                } else {
                    egui::Align2::RIGHT_CENTER
                };
                place_text(
                    painter,
                    freq_text(flo + (fhi - flo) * t),
                    font.clone(),
                    ink,
                    egui::pos2(freq.right() - 4.0, y),
                    align,
                    freq_bounds,
                    &mut used,
                );
            }
        }
        let db_bounds = egui::Rect::from_min_max(
            egui::pos2(db.left().max(window.left()), bar.top().max(window.top())),
            egui::pos2(window.right(), bar.bottom().min(window.bottom())),
        );
        let mut used = Vec::new();
        for step in [0, 4, 2, 1, 3] {
            let t = step as f32 / 4.0;
            let value = self.urange as f32 + (self.lrange - self.urange) as f32 * t;
            let y = bar.top() + t * bar.height();
            let align = if step == 0 {
                egui::Align2::LEFT_TOP
            } else if step == 4 {
                egui::Align2::LEFT_BOTTOM
            } else {
                egui::Align2::LEFT_CENTER
            };
            place_text(
                painter,
                format!("{}", value.round() as i32),
                font.clone(),
                ink,
                egui::pos2(db.left() + 4.0, y),
                align,
                db_bounds,
                &mut used,
            );
        }
        let duration = self.axis_seconds();
        if duration > 0.0 {
            let time_bounds = egui::Rect::from_min_max(
                egui::pos2(plot.left().max(window.left()), plot.bottom()),
                egui::pos2(plot.right().min(window.right()), window.bottom()),
            );
            let mut used = Vec::new();
            for step in [0, 4, 2, 1, 3] {
                let t = step as f32 / 4.0;
                let x = plot.left() + t * plot.width();
                let align = if step == 0 {
                    egui::Align2::LEFT_TOP
                } else if step == 4 {
                    egui::Align2::RIGHT_TOP
                } else {
                    egui::Align2::CENTER_TOP
                };
                place_text(
                    painter,
                    format_time(duration * t as f64),
                    font.clone(),
                    ink,
                    egui::pos2(x, plot.bottom() + 4.0),
                    align,
                    time_bounds,
                    &mut used,
                );
            }
        }
    }

    fn freq_controls_ready(&self) -> bool {
        self.loaded.is_some()
            && !self.decoding
            && self.analyze_key.is_some()
            && self.progress.is_none()
            && self.axis_nyquist() > 0.0
    }

    fn freq_cap(&self) -> f32 {
        self.axis_nyquist().max(192_000.0)
    }

    /// Hertz window drawn from the bottom of the plot to the top.
    /// A custom window is kept across files so 1 kHz stays on the same line.
    fn freq_view(&self) -> (f32, f32) {
        let nyq = self.axis_nyquist();
        if !self.freq_custom {
            return (0.0, nyq.max(0.0));
        }
        let cap = self.freq_cap();
        let mut lo = self.freq_lo.clamp(0.0, cap);
        let mut hi = self.freq_hi.clamp(0.0, cap);
        if hi - lo < 1.0 {
            hi = (lo + 1.0).min(cap);
            if hi - lo < 1.0 {
                lo = (hi - 1.0).max(0.0);
            }
        }
        (lo, hi)
    }

    fn freq_controls(&mut self, ui: &mut egui::Ui) {
        let ready = self.freq_controls_ready();
        let nyq = self.axis_nyquist();
        let cap = if nyq > 0.0 { self.freq_cap() } else { 192_000.0 };
        let (view_lo, view_hi) = if nyq > 0.0 {
            self.freq_view()
        } else {
            (0.0, 0.0)
        };
        let hint = if ready {
            "畫完後可改。同一段赫茲會對齊到同一高度，換檔案也沿用，方便比較。超過這個檔案的頻率會留空。"
        } else {
            "頻譜畫完後才能設定頻率範圍"
        };
        let speed = (cap / 500.0).max(1.0);
        let mut lo = view_lo;
        let mut hi = view_hi;
        ui.label("頻率下限").on_hover_text(hint);
        let lo_r = ui
            .allocate_ui_with_layout(
                egui::vec2(118.0, 28.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.add_enabled(
                        ready,
                        egui::DragValue::new(&mut lo)
                            .range(0.0..=cap)
                            .speed(speed)
                            .max_decimals(0)
                            .suffix(" Hz"),
                    )
                },
            )
            .inner
            .on_hover_text(hint);
        ui.label("頻率上限").on_hover_text(hint);
        let hi_r = ui
            .allocate_ui_with_layout(
                egui::vec2(118.0, 28.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.add_enabled(
                        ready,
                        egui::DragValue::new(&mut hi)
                            .range(0.0..=cap)
                            .speed(speed)
                            .max_decimals(0)
                            .suffix(" Hz"),
                    )
                },
            )
            .inner
            .on_hover_text(hint);
        if ui
            .add_enabled(ready, egui::Button::new("全頻"))
            .on_hover_text("恢復 0 到這個檔案的最高頻率")
            .clicked()
        {
            self.freq_custom = false;
        }
        if ready && (lo_r.changed() || hi_r.changed()) {
            let mut lo = lo.clamp(0.0, cap);
            let mut hi = hi.clamp(0.0, cap);
            if hi - lo < 1.0 {
                if hi_r.changed() && !lo_r.changed() {
                    lo = (hi - 1.0).max(0.0);
                } else {
                    hi = (lo + 1.0).min(cap);
                }
            }
            self.freq_lo = lo;
            self.freq_hi = hi;
            self.freq_custom = true;
        }
    }

    fn render_spec(&self, spec: &Spectrum, lo: f32, hi: f32) -> spek_core::RgbaImage {
        let urange = self.urange as f32;
        let lrange = self.lrange as f32;
        if self.freq_custom {
            if let Some((db, bands)) = window_db(
                &spec.db,
                spec.width,
                spec.bands,
                spec.sample_rate,
                lo,
                hi,
            ) {
                return render_rgba(&db, spec.width, bands, self.palette, urange, lrange);
            }
        }
        render_rgba(
            &spec.db,
            spec.width,
            spec.bands,
            self.palette,
            urange,
            lrange,
        )
    }

    fn axis_nyquist(&self) -> f32 {
        if let Some(loaded) = &self.loaded {
            if loaded.info.sample_rate > 0 {
                return loaded.info.sample_rate as f32 / 2.0;
            }
        }
        if self.axis_rate > 0.0 {
            self.axis_rate / 2.0
        } else {
            0.0
        }
    }

    fn axis_seconds(&self) -> f64 {
        if let Some(loaded) = &self.loaded {
            if loaded.info.duration > 0.0 {
                return loaded.info.duration;
            }
        }
        self.axis_duration
    }

    fn can_save(&self) -> bool {
        self.loaded.is_some()
            && !self.decoding
            && !self.exporting
            && self.analyze_key.is_some()
            && self.progress.is_none()
            && self.plot_width >= 8
            && self.plot_height >= 8
    }

    fn cancel_export(&mut self) {
        self.export_cancel.store(true, Ordering::Relaxed);
        self.export_ticket += 1;
        self.exporting = false;
    }

    fn info_line(&self) -> String {
        let Some(loaded) = &self.loaded else {
            return "尚未開啟檔案".into();
        };
        let mut line = describe(loaded, self.channel, self.window, 1usize << self.fft_bits);
        let (lo, hi) = self.freq_view();
        if hi > lo {
            line.push_str(&format!(" · 顯示 {}–{}", freq_text(lo), freq_text(hi)));
        }
        line.push_str(&format!(" · {} 到 {} dB", self.urange, self.lrange));
        line
    }

    fn export_scale_control(&mut self, ui: &mut egui::Ui) {
        let label = scale_label(self.export_scale);
        egui::ComboBox::from_label("存圖")
            .width(72.0)
            .selected_text(label)
            .show_ui(ui, |ui| {
                for scale in [0.5_f32, 1.0, 2.0, 4.0, 8.0] {
                    ui.selectable_value(&mut self.export_scale, scale, scale_label(scale));
                }
            })
            .response
            .on_hover_text("依目前畫面的寬高重算頻譜後存檔，不是把圖放大或縮小");
    }

    fn start_export(&mut self, path: PathBuf) {
        let Some(loaded) = &self.loaded else {
            return;
        };
        let samples = if self.channel == usize::MAX {
            mix_arcs(&loaded.channels)
        } else {
            match loaded.channels.get(self.channel) {
                Some(channel) => channel.as_ref().clone(),
                None => {
                    self.error = Some("聲道不存在".into());
                    return;
                }
            }
        };
        let (flo, fhi) = self.freq_view();
        if !(fhi > flo) {
            self.error = Some("頻率範圍無效".into());
            return;
        }
        self.export_cancel.store(true, Ordering::Relaxed);
        let cancel = Arc::new(AtomicBool::new(false));
        self.export_cancel = cancel.clone();
        self.export_ticket += 1;
        let ticket = self.export_ticket;
        self.exporting = true;
        let label = scale_label(self.export_scale);
        let text = format!("正在以 {label} 繪製存檔…");
        self.busy = text.clone();
        self.progress = Some(ProgressUi {
            text,
            fraction: 0.0,
            indeterminate: false,
        });
        self.error = None;
        let title = self
            .path
            .as_ref()
            .and_then(|path| path.file_name())
            .and_then(|name| name.to_str())
            .unwrap_or("Spek")
            .to_string();
        let fig = poster::Figure {
            screen_w: self.plot_width,
            screen_h: self.plot_height,
            scale: self.export_scale,
            samples,
            sample_rate: loaded.info.sample_rate,
            nfft: 1usize << self.fft_bits,
            window: self.window,
            palette: self.palette,
            urange: self.urange as f32,
            lrange: self.lrange as f32,
            freq_lo: flo,
            freq_hi: fhi,
            title,
            meta: format!(
                "{} · 存圖 {label} · 計算 10·log10(|X|²/N²)",
                self.info_line()
            ),
            freq: std::array::from_fn(|step| {
                let t = step as f32 / 4.0;
                freq_text(flo + (fhi - flo) * t)
            }),
            db: std::array::from_fn(|step| {
                let t = step as f32 / 4.0;
                let value = self.urange as f32 + (self.lrange - self.urange) as f32 * t;
                format!("{}", value.round() as i32)
            }),
            time: {
                let duration = self.axis_seconds();
                std::array::from_fn(|step| format_time(duration * step as f64 / 4.0))
            },
        };
        let tx = self.tx.clone();
        thread::spawn(move || {
            let send_progress = |fraction: f32| {
                let text = format!("正在以 {label} 繪製存檔 {:.0}%", fraction * 100.0);
                let _ = tx.send(job::JobEvent::ExportProgress {
                    ticket,
                    text,
                    fraction,
                });
            };
            match poster::write_figure(&fig, &path, &cancel, send_progress) {
                Ok(message) => {
                    let _ = tx.send(job::JobEvent::Exported {
                        ticket,
                        path: message,
                    });
                }
                Err(message) if message == "已取消" => {}
                Err(message) => {
                    let _ = tx.send(job::JobEvent::ExportFailed { ticket, message });
                }
            }
        });
    }

    fn paint_progress(&mut self, ctx: &egui::Context) {
        let Some(progress) = self.progress.clone() else {
            return;
        };
        let exporting = self.exporting;
        let mut cancel_export = false;
        egui::Area::new(egui::Id::new("draw-progress"))
            .order(egui::Order::Foreground)
            .anchor(
                egui::Align2::CENTER_TOP,
                egui::vec2(0.0, self.toolbar_h + 8.0),
            )
            .interactable(exporting)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_min_width(420.0);
                    ui.set_max_width(520.0);
                    ui.label(&progress.text);
                    ui.add(
                        egui::ProgressBar::new(progress.fraction)
                            .animate(progress.indeterminate)
                            .desired_width(420.0)
                            .fill(egui::Color32::from_rgb(32, 140, 96)),
                    );
                    if exporting && ui.button("取消存檔").clicked() {
                        cancel_export = true;
                    }
                });
            });
        if cancel_export {
            self.cancel_export();
            self.progress = None;
            self.busy.clear();
            self.notice = Some("已取消存檔".into());
        }
    }

    fn help_window(&mut self, ctx: &egui::Context) {
        if !self.show_help {
            return;
        }
        let mut open = true;
        egui::Window::new("Spek 1.5 說明")
            .open(&mut open)
            .resizable(true)
            .default_width(460.0)
            .show(ctx, |ui| {
                ui.label(HELP);
            });
        self.show_help = open;
    }
}

fn dropped_file(ctx: &egui::Context) -> Option<PathBuf> {
    ctx.input(|input| {
        input
            .raw
            .dropped_files
            .iter()
            .find_map(|file| file.path.clone())
    })
}

fn install_style(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.window_fill = egui::Color32::from_rgb(16, 20, 26);
    visuals.panel_fill = egui::Color32::from_rgb(20, 25, 32);
    visuals.extreme_bg_color = egui::Color32::from_rgb(10, 12, 16);
    visuals.faint_bg_color = egui::Color32::from_rgb(28, 35, 44);
    visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(36, 46, 58);
    visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(48, 72, 64);
    visuals.widgets.active.bg_fill = egui::Color32::from_rgb(32, 110, 78);
    visuals.selection.bg_fill = egui::Color32::from_rgb(28, 96, 72);
    visuals.hyperlink_color = egui::Color32::from_rgb(130, 190, 255);
    ctx.set_visuals(visuals);
    ctx.style_mut(|style| {
        style.spacing.item_spacing = egui::vec2(10.0, 6.0);
        style.spacing.button_padding = egui::vec2(12.0, 5.0);
        for (text, size) in [
            (egui::TextStyle::Body, 15.0),
            (egui::TextStyle::Button, 15.0),
            (egui::TextStyle::Small, 13.0),
            (egui::TextStyle::Heading, 20.0),
        ] {
            style
                .text_styles
                .insert(text, egui::FontId::proportional(size));
        }
    });
}

fn load_icon() -> egui::IconData {
    match image::load_from_memory(include_bytes!("../assets/spek.png")) {
        Ok(image) => {
            let rgba = image.to_rgba8();
            let (width, height) = rgba.dimensions();
            egui::IconData {
                rgba: rgba.into_raw(),
                width,
                height,
            }
        }
        Err(err) => {
            report::note(&format!("圖示讀取失敗：{err}"));
            egui::IconData::default()
        }
    }
}

fn palette_column(palette: Palette) -> egui::ColorImage {
    let height = 256;
    let mut rgba = Vec::with_capacity(height * 4);
    for y in 0..height {
        let level = 1.0 - y as f64 / (height - 1) as f64;
        let [r, g, b] = spek_core::map_color(palette, level);
        rgba.extend_from_slice(&[r, g, b, 255]);
    }
    egui::ColorImage::from_rgba_unmultiplied([1, height], &rgba)
}

fn window_label(window: WindowKind) -> &'static str {
    match window {
        WindowKind::Hann => "Hann（漢恩）",
        WindowKind::Hamming => "Hamming（漢明）",
        WindowKind::BlackmanHarris => "Blackman-Harris",
    }
}

fn palette_label(palette: Palette) -> &'static str {
    match palette {
        Palette::Sox => "SoX（預設）",
        Palette::Spectrum => "彩虹",
        Palette::Mono => "灰階",
    }
}

fn describe(loaded: &Loaded, channel: usize, window: WindowKind, nfft: usize) -> String {
    let info = &loaded.info;
    let mut parts = Vec::new();
    if info.stream_count > 1 {
        parts.push(format!("串流 {}/{}", info.stream_index + 1, info.stream_count));
    }
    parts.push(info.codec.clone());
    if let Some(bits) = info.bits_per_sample {
        parts.push(format!("{bits} bit"));
    }
    if let Some(rate) = info.bit_rate {
        parts.push(format!("{} kbps", (rate + 500) / 1000));
    }
    parts.push(format!("{} Hz", info.sample_rate));
    if channel == usize::MAX {
        parts.push(format!("混合 {} 聲道", loaded.channels.len()));
    } else {
        parts.push(format!("聲道 {}/{}", channel + 1, loaded.channels.len()));
    }
    parts.push(format!("長度 {}", format_time(info.duration)));
    parts.push(info.engine.clone());
    parts.push(format!("W:{nfft}"));
    parts.push(window.name().to_string());
    parts.join(" · ")
}

fn scale_label(scale: f32) -> String {
    if (scale - 0.5).abs() < 0.01 {
        "0.5x".into()
    } else {
        format!("{}x", scale.round() as i32)
    }
}

fn place_text(
    painter: &egui::Painter,
    text: String,
    font: egui::FontId,
    color: egui::Color32,
    anchor: egui::Pos2,
    align: egui::Align2,
    bounds: egui::Rect,
    used: &mut Vec<egui::Rect>,
) {
    if bounds.width() < 4.0 || bounds.height() < 4.0 {
        return;
    }
    let galley = painter.layout_no_wrap(text, font, color);
    let size = galley.size();
    let mut x = anchor.x
        - match align.x() {
            egui::Align::Center => size.x * 0.5,
            egui::Align::Max => size.x,
            egui::Align::Min => 0.0,
        };
    let mut y = anchor.y
        - match align.y() {
            egui::Align::Center => size.y * 0.5,
            egui::Align::Max => size.y,
            egui::Align::Min => 0.0,
        };
    x = x.clamp(bounds.left(), (bounds.right() - size.x).max(bounds.left()));
    y = y.clamp(bounds.top(), (bounds.bottom() - size.y).max(bounds.top()));
    let rect = egui::Rect::from_min_size(egui::pos2(x, y), size);
    if used.iter().any(|old| old.expand(1.0).intersects(rect)) {
        return;
    }
    if rect.right() > bounds.right() + 0.5 || rect.bottom() > bounds.bottom() + 0.5 {
        return;
    }
    painter.galley(rect.min, galley, color);
    used.push(rect);
}

fn freq_text(hz: f32) -> String {
    if hz >= 1000.0 {
        let khz = hz / 1000.0;
        if (khz - khz.round()).abs() < 0.05 {
            format!("{} kHz", khz.round() as i32)
        } else {
            format!("{khz:.1} kHz")
        }
    } else {
        format!("{} Hz", hz.round() as i32)
    }
}

fn format_time(seconds: f64) -> String {
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

const HELP: &str = "\
Spek 1.5 聲譜圖。
繪圖使用 OpenGL（系統內建的 opengl32）。
音訊全部由內建的 FFmpeg 9 解碼。

解碼或重算時，進度條浮在畫面上方正中，不會把選單往下推。畫完就消失。
選單分成固定的三列，縮放視窗時按鈕不會換行跳位。

右側的 dB 數字貼著色條，並且留在視窗裡面。
頻譜畫完後可以設定頻率下限與上限，畫面會立刻依這個範圍重畫。
同一段赫茲會落在同樣的高度；若這個檔案到不了上限，上面會留空。

存圖會照目前視窗裡的頻譜尺寸重算，不是把畫面放大縮小。
倍率有 0.5x、1x、2x、4x、8x。圖片含頻率、dB、時間、聲道、編碼、長度與計算方式。

解碼執行緒預設等於這臺電腦的高效能實體核心數（有 P 核心與 E 核心時，不算 E 核心）。
可以在工具列改這個數字，下次開啟檔案時套用。音訊解碼器若本身不能平行，多出來的執行緒不會被用到。

介面字形依序為：思源黑體（Adobe Source Han Sans，Traditional Chinese—Taiwan）、
微軟正黑體、新細明體。後兩者用系統已安裝的字型遞補思源黑體沒有的字。

開啟檔案後，時間由左到右，頻率由下到上。
顏色越亮，該頻帶的音量越大。預設範圍是 0 到 -120 dB。

快捷鍵
  Ctrl+O        開啟檔案
  Ctrl+S        儲存 PNG
  C / Shift+C   下一個 / 上一個聲道
  F / Shift+F   窗函數
  P / Shift+P   色彩
  W / Shift+W   FFT 大小
  S / Shift+S   音訊串流（檔案含多軌時）
  U / Shift+U   動態範圍上限
  L / Shift+L   動態範圍下限
  F1            本說明

也可以把檔案拖進視窗。

WAV、FLAC、MP3、Ogg Vorbis、AAC、ALAC、Opus、WMA、APE、WavPack、
Musepack、AC3、DTS，以及其他 FFmpeg 支援的音訊，都走同一條解碼路徑。
若視窗一閃即逝，請看同一個資料夾裡的 spek.log。
";
