use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_void};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::audio::{AudioInfo, DecodeEvent};

const CODEC_LEN: usize = 160;

#[repr(C)]
struct SpekInfo {
    sample_rate: c_int,
    bits_per_sample: c_int,
    stream_count: c_int,
    stream_index: c_int,
    bit_rate: c_int,
    channels: c_int,
    total_samples: i64,
    codec: [u8; CODEC_LEN],
}

extern "C" {
    fn spek_ffmpeg_decode(
        path: *const c_char,
        stream_index: c_int,
        threads: c_int,
        callback: SpekFrameFn,
        user: *mut c_void,
        err: *mut c_char,
        err_len: c_int,
    ) -> c_int;
}

type SpekFrameFn = unsafe extern "C" fn(
    user: *mut c_void,
    planes: *const *const f32,
    channels: c_int,
    nb_samples: c_int,
    info: *const SpekInfo,
) -> c_int;

struct Bridge<'a> {
    cancel: &'a AtomicBool,
    on_event: &'a mut dyn FnMut(DecodeEvent<'_>) -> bool,
    samples: usize,
    stop: bool,
    last: Option<AudioInfo>,
}

pub fn decode_with(
    path: &Path,
    stream_index: usize,
    threads: u32,
    cancel: &AtomicBool,
    on_event: &mut dyn FnMut(DecodeEvent<'_>) -> bool,
) -> Result<AudioInfo, DecodeStop> {
    if stream_index > i32::MAX as usize {
        return Err(DecodeStop::Failed("音訊串流編號太大".into()));
    }
    let text = path
        .to_str()
        .ok_or_else(|| DecodeStop::Failed("檔案路徑不是 UTF-8，FFmpeg 無法開啟".into()))?;
    let c_path = CString::new(text).map_err(|_| DecodeStop::Failed("檔案路徑含有空字元".into()))?;
    let mut bridge = Bridge {
        cancel,
        on_event,
        samples: 0,
        stop: false,
        last: None,
    };
    let mut err = [0u8; 512];
    let code = unsafe {
        spek_ffmpeg_decode(
            c_path.as_ptr(),
            stream_index as c_int,
            threads.max(1).min(256) as c_int,
            trampoline,
            &mut bridge as *mut Bridge<'_> as *mut c_void,
            err.as_mut_ptr() as *mut c_char,
            err.len() as c_int,
        )
    };
    if code == 1 || bridge.stop || cancel.load(Ordering::Relaxed) {
        return Err(DecodeStop::Cancelled);
    }
    if code != 0 {
        let message = cstr_buf(&err).unwrap_or_else(|| "FFmpeg 解碼失敗".into());
        return Err(DecodeStop::Failed(message));
    }
    if bridge.samples == 0 {
        return Err(DecodeStop::Failed("FFmpeg 沒有解出音訊資料".into()));
    }
    let mut info = bridge
        .last
        .ok_or_else(|| DecodeStop::Failed("FFmpeg 沒有解出音訊資料".into()))?;
    info.sample_count = bridge.samples;
    if info.sample_rate > 0 {
        info.duration = bridge.samples as f64 / info.sample_rate as f64;
    }
    Ok(info)
}

#[derive(Debug)]
pub enum DecodeStop {
    Cancelled,
    Failed(String),
}

unsafe extern "C" fn trampoline(
    user: *mut c_void,
    planes: *const *const f32,
    channels: c_int,
    nb_samples: c_int,
    info: *const SpekInfo,
) -> c_int {
    if user.is_null() || info.is_null() {
        return 1;
    }
    let bridge = &mut *(user as *mut Bridge<'_>);
    if bridge.cancel.load(Ordering::Relaxed) || bridge.stop {
        return 1;
    }
    let info = &*info;
    let channel_count = channels.max(0) as usize;
    let count = nb_samples.max(0) as usize;
    let mut fresh = Vec::new();
    if count > 0 && channel_count > 0 {
        if planes.is_null() {
            return 1;
        }
        for index in 0..channel_count {
            let ptr = *planes.add(index);
            if ptr.is_null() {
                return 1;
            }
            fresh.push(std::slice::from_raw_parts(ptr, count).to_vec());
        }
        bridge.samples = bridge.samples.saturating_add(count);
    }
    let sample_rate = u32::try_from(info.sample_rate).unwrap_or(0);
    let estimated = (info.total_samples > 0).then_some(info.total_samples as usize);
    let duration = if sample_rate > 0 {
        if let Some(total) = estimated {
            total as f64 / sample_rate as f64
        } else {
            bridge.samples as f64 / sample_rate as f64
        }
    } else {
        0.0
    };
    let event = DecodeEvent {
        info: AudioInfo {
            codec: cstr_buf(&info.codec).unwrap_or_else(|| "FFmpeg".into()),
            sample_rate,
            channels: if channel_count > 0 {
                channel_count
            } else {
                info.channels.max(0) as usize
            },
            bits_per_sample: (info.bits_per_sample > 0).then_some(info.bits_per_sample as u32),
            bit_rate: (info.bit_rate > 0).then_some(info.bit_rate as u32),
            duration,
            sample_count: bridge.samples,
            stream_index: info.stream_index.max(0) as usize,
            stream_count: info.stream_count.max(0) as usize,
            engine: "FFmpeg".into(),
        },
        estimated_total: estimated,
        fresh: &fresh,
        samples: bridge.samples,
    };
    bridge.last = Some(event.info.clone());
    let keep = (bridge.on_event)(event);
    if !keep {
        bridge.stop = true;
        return 1;
    }
    0
}

fn cstr_buf(buf: &[u8]) -> Option<String> {
    let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
    if end == 0 {
        return None;
    }
    CStr::from_bytes_with_nul(&buf[..=end])
        .ok()
        .map(|text| text.to_string_lossy().into_owned())
        .filter(|text| !text.is_empty())
}
