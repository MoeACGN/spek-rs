// Spek 1.5 — acoustic spectrum analyser.
// Derived from Spek by Alexander Kojevnikov and contributors.
// Licensed under GPL-3.0-or-later.

#![deny(unsafe_code)]

#[allow(unsafe_code)]
mod ffmpeg;
mod audio;
mod dsp;
mod palette;

pub use audio::{
    decode, decode_threaded, decode_with, AudioInfo, DecodeEvent, DecodeStop, Decoded,
};
pub use dsp::{analyze, window_db, write_png, ColumnPump, Spectrum, WindowKind};
pub use palette::{map_color, render_rgba, save_png, Palette, RgbaImage};
