use std::path::Path;
use std::sync::atomic::AtomicBool;

pub use crate::ffmpeg::DecodeStop;

#[derive(Clone, Debug)]
pub struct AudioInfo {
    pub codec: String,
    pub sample_rate: u32,
    pub channels: usize,
    pub bits_per_sample: Option<u32>,
    pub bit_rate: Option<u32>,
    pub duration: f64,
    pub sample_count: usize,
    pub stream_index: usize,
    pub stream_count: usize,
    pub engine: String,
}

pub struct Decoded {
    pub info: AudioInfo,
    pub channels: Vec<Vec<f32>>,
}

pub struct DecodeEvent<'a> {
    pub info: AudioInfo,
    /// Header estimate of the whole file, when the container knows it.
    pub estimated_total: Option<usize>,
    pub fresh: &'a [Vec<f32>],
    pub samples: usize,
}

pub fn decode(path: &Path, stream_index: usize) -> Result<Decoded, String> {
    decode_threaded(path, stream_index, 1)
}

pub fn decode_threaded(path: &Path, stream_index: usize, threads: u32) -> Result<Decoded, String> {
    let mut channels: Vec<Vec<f32>> = Vec::new();
    let cancel = AtomicBool::new(false);
    let info = decode_with(path, stream_index, threads, &cancel, &mut |event| {
        append_channels(&mut channels, event.fresh);
        true
    })
    .map_err(|err| match err {
        DecodeStop::Cancelled => "解碼已取消".into(),
        DecodeStop::Failed(message) => message,
    })?;
    if channels.is_empty() || channels[0].is_empty() {
        return Err("FFmpeg 解出的聲道是空的".into());
    }
    let decoded = with_file_bitrate(
        Decoded {
            info,
            channels,
        },
        path,
    );
    Ok(decoded)
}

pub fn decode_with(
    path: &Path,
    stream_index: usize,
    threads: u32,
    cancel: &AtomicBool,
    on_event: &mut dyn FnMut(DecodeEvent<'_>) -> bool,
) -> Result<AudioInfo, DecodeStop> {
    crate::ffmpeg::decode_with(path, stream_index, threads, cancel, on_event)
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

fn with_file_bitrate(mut decoded: Decoded, path: &Path) -> Decoded {
    let file_len = std::fs::metadata(path).ok().map(|meta| meta.len()).unwrap_or(0);
    if decoded.info.duration > 0.05 && file_len > 0 {
        decoded.info.bit_rate = Some((file_len as f64 * 8.0 / decoded.info.duration) as u32);
    }
    decoded
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::Ordering;

    #[test]
    fn decodes_bundled_fixtures() {
        let root = PathBuf::from("/workspace/spek/tests/samples");
        for name in [
            "2ch-44100Hz-16bps.wav",
            "2ch-48000Hz-16bps.flac",
            "2ch-44100Hz-128cbr.mp3",
            "2ch-44100Hz-q5.ogg",
            "2ch-44100Hz-v1.wma",
            "2ch-44100Hz-v2.wma",
            "1ch-96000Hz-24bps.ape",
            "2ch-48000Hz-16bps.ape",
            "1ch-96000Hz-24bps.wv",
            "2ch-48000Hz-16bps.wv",
            "2ch-44100Hz.ac3",
            "2ch-44100Hz.dts",
            "2ch-44100Hz-std.mpc",
            "2ch-44100Hz-16bps.m4a",
            "2ch-44100Hz-q100.m4a",
        ] {
            let path = root.join(name);
            let decoded = decode(&path, 0).unwrap_or_else(|err| panic!("{name}: {err}"));
            assert!(
                decoded.info.sample_count > 1000,
                "{name} samples {}",
                decoded.info.sample_count
            );
            assert!(decoded.info.sample_rate >= 44_100, "{name}");
            assert!(!decoded.channels.is_empty(), "{name}");
            assert_eq!(decoded.info.engine, "FFmpeg", "{name} {}", decoded.info.codec);
        }
    }

    #[test]
    fn ffmpeg_decodes_a_loud_tone_and_unicode_path() {
        let dir = std::env::temp_dir();
        let wav = dir.join("spek-聲譜-tone.wav");
        let ffmpeg = "/usr/local/bin/ffmpeg";
        let status = std::process::Command::new(ffmpeg)
            .args([
                "-y",
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=1000:sample_rate=44100:duration=0.4",
                "-c:a",
                "pcm_s16le",
            ])
            .arg(&wav)
            .status()
            .unwrap();
        assert!(status.success());
        let decoded = decode(&wav, 0).unwrap();
        let peak = decoded.channels[0]
            .iter()
            .fold(0.0f32, |peak, sample| peak.max(sample.abs()));
        assert_eq!(decoded.info.engine, "FFmpeg");
        assert!(peak > 0.05, "s16 peak {peak}");
        let _ = std::fs::remove_file(&wav);
    }

    #[test]
    fn progress_grows_and_threads_match() {
        let dir = std::env::temp_dir();
        let wav = dir.join("spek-progress.wav");
        let ffmpeg = "/usr/local/bin/ffmpeg";
        let status = std::process::Command::new(ffmpeg)
            .args([
                "-y",
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:sample_rate=44100:duration=1.2",
                "-c:a",
                "pcm_s16le",
            ])
            .arg(&wav)
            .status()
            .unwrap();
        assert!(status.success());
        let cancel = AtomicBool::new(false);
        let mut seen = Vec::new();
        let mut estimate = None;
        let info = decode_with(&wav, 0, 2, &cancel, &mut |event| {
            if event.samples > 0 {
                seen.push(event.samples);
            }
            if estimate.is_none() {
                estimate = event.estimated_total;
            }
            true
        })
        .unwrap();
        assert!(seen.len() >= 2, "callbacks {}", seen.len());
        assert!(seen.windows(2).all(|pair| pair[1] >= pair[0]));
        assert_eq!(*seen.last().unwrap(), info.sample_count);
        assert!(estimate.unwrap_or(0) > 40_000, "estimate {estimate:?}");
        let single = decode_threaded(&wav, 0, 1).unwrap();
        let multi = decode_threaded(&wav, 0, 4).unwrap();
        assert_eq!(single.info.sample_count, multi.info.sample_count);
        assert_eq!(single.channels[0].len(), multi.channels[0].len());
        assert!(!cancel.load(Ordering::Relaxed));
        let _ = std::fs::remove_file(&wav);
    }
}
