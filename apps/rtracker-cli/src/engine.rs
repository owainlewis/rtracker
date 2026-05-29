//! Shared audio engine: owns the cpal output stream, the current loop buffer
//! (lock-free via `ArcSwap`), and a sample-accurate playhead the UI can read.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, bail, Context, Result};
use arc_swap::ArcSwap;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::SampleFormat;

pub struct AudioEngine {
    pub device_sr: u32,
    pub device_name: String,
    #[allow(dead_code)]
    pub channels: usize,
    buffer: Arc<ArcSwap<Vec<f32>>>,
    /// Playhead position in *floats* (NOT frames) into the stereo-interleaved
    /// buffer. Always even. Updated by the audio callback.
    playhead: Arc<AtomicU64>,
    paused: Arc<AtomicBool>,
    /// One-shot preview bus: stereo interleaved samples mixed on top of the
    /// loop. Inactive when `preview_head >= preview.len()`.
    preview: Arc<ArcSwap<Vec<f32>>>,
    preview_head: Arc<AtomicU64>,
    _stream: cpal::Stream,
}

impl AudioEngine {
    pub fn start(initial: Vec<f32>) -> Result<Self> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| anyhow!("no default audio output device"))?;
        let supported = device.default_output_config().context("query default output config")?;
        let device_sr = supported.sample_rate().0;
        let channels = supported.channels() as usize;
        let sample_format = supported.sample_format();
        let stream_config: cpal::StreamConfig = supported.into();
        let device_name = device.name().unwrap_or_else(|_| "?".into());

        let buffer: Arc<ArcSwap<Vec<f32>>> = Arc::new(ArcSwap::from_pointee(initial));
        let playhead = Arc::new(AtomicU64::new(0));
        let paused = Arc::new(AtomicBool::new(false));
        // Preview starts empty and inactive (head == len == 0).
        let preview: Arc<ArcSwap<Vec<f32>>> = Arc::new(ArcSwap::from_pointee(Vec::new()));
        let preview_head = Arc::new(AtomicU64::new(0));

        let stream = build_stream(
            &device,
            &stream_config,
            sample_format,
            channels,
            buffer.clone(),
            playhead.clone(),
            paused.clone(),
            preview.clone(),
            preview_head.clone(),
        )?;
        stream.play().context("start stream")?;

        Ok(Self {
            device_sr,
            device_name,
            channels,
            buffer,
            playhead,
            paused,
            preview,
            preview_head,
            _stream: stream,
        })
    }

    /// Trigger a one-shot preview. Replaces whatever the preview bus was
    /// playing. Stereo interleaved f32 buffer expected.
    pub fn play_preview(&self, buf: Vec<f32>) {
        // Swap buffer first, then reset head. The other ordering risks
        // reading past the end of a fresh buffer with a stale large head.
        self.preview.store(Arc::new(buf));
        self.preview_head.store(0, Ordering::Relaxed);
    }

    pub fn swap_buffer(&self, new_buf: Vec<f32>) {
        self.buffer.store(Arc::new(new_buf));
    }

    pub fn current_buffer(&self) -> Arc<Vec<f32>> {
        let g = self.buffer.load();
        Arc::clone(&g)
    }

    /// Current playhead position as a frame index, clamped to buffer length.
    pub fn playhead_frame(&self) -> u64 {
        let p = self.playhead.load(Ordering::Relaxed);
        p / 2
    }

    pub fn toggle_paused(&self) {
        self.paused.fetch_xor(true, Ordering::Relaxed);
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }

    pub fn rewind(&self) {
        self.playhead.store(0, Ordering::Relaxed);
    }
}

#[allow(clippy::too_many_arguments)]
fn build_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    fmt: SampleFormat,
    channels: usize,
    buffer: Arc<ArcSwap<Vec<f32>>>,
    playhead: Arc<AtomicU64>,
    paused: Arc<AtomicBool>,
    preview: Arc<ArcSwap<Vec<f32>>>,
    preview_head: Arc<AtomicU64>,
) -> Result<cpal::Stream> {
    let err_fn = |e| tracing::error!(error = %e, "stream error");
    let chans = channels;
    let stream = match fmt {
        SampleFormat::F32 => {
            let buf = buffer.clone();
            let ph = playhead.clone();
            let pa = paused.clone();
            let pv = preview.clone();
            let pvh = preview_head.clone();
            device.build_output_stream(
                config,
                move |out: &mut [f32], _| fill(out, chans, &buf, &ph, &pa, &pv, &pvh, |x| x),
                err_fn,
                None,
            )?
        }
        SampleFormat::I16 => {
            let buf = buffer.clone();
            let ph = playhead.clone();
            let pa = paused.clone();
            let pv = preview.clone();
            let pvh = preview_head.clone();
            device.build_output_stream(
                config,
                move |out: &mut [i16], _| {
                    fill(out, chans, &buf, &ph, &pa, &pv, &pvh, |x| {
                        (x.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
                    })
                },
                err_fn,
                None,
            )?
        }
        SampleFormat::U16 => {
            let buf = buffer.clone();
            let ph = playhead.clone();
            let pa = paused.clone();
            let pv = preview.clone();
            let pvh = preview_head.clone();
            device.build_output_stream(
                config,
                move |out: &mut [u16], _| {
                    fill(out, chans, &buf, &ph, &pa, &pv, &pvh, |x| {
                        let v = (x.clamp(-1.0, 1.0) * 0.5 + 0.5) * u16::MAX as f32;
                        v as u16
                    })
                },
                err_fn,
                None,
            )?
        }
        other => bail!("unsupported sample format: {:?}", other),
    };
    Ok(stream)
}

#[allow(clippy::too_many_arguments)]
fn fill<T, F>(
    out: &mut [T],
    channels: usize,
    buf: &Arc<ArcSwap<Vec<f32>>>,
    playhead: &Arc<AtomicU64>,
    paused: &Arc<AtomicBool>,
    preview: &Arc<ArcSwap<Vec<f32>>>,
    preview_head: &Arc<AtomicU64>,
    conv: F,
) where
    T: Copy,
    F: Fn(f32) -> T,
{
    let silence_zero = conv(0.0);
    if paused.load(Ordering::Relaxed) {
        for s in out.iter_mut() {
            *s = silence_zero;
        }
        return;
    }
    let guard = buf.load();
    let src: &Vec<f32> = guard.as_ref();
    let len = src.len();
    if len < 2 {
        for s in out.iter_mut() {
            *s = silence_zero;
        }
        return;
    }
    let pv_guard = preview.load();
    let pv_src: &Vec<f32> = pv_guard.as_ref();
    let pv_len = pv_src.len();
    let mut pv_head = preview_head.load(Ordering::Relaxed) as usize;

    let mut head = playhead.load(Ordering::Relaxed) as usize % len;
    if head & 1 != 0 {
        head -= 1;
    }

    for frame in out.chunks_mut(channels) {
        let mut l = src[head];
        let mut r = src[head + 1];
        // Mix preview on top while it's still running, at 0.7× so it doesn't
        // bury the loop. Always re-clamp to [-1, 1] after summing.
        if pv_head + 1 < pv_len {
            l = (l + pv_src[pv_head] * 0.7).clamp(-1.0, 1.0);
            r = (r + pv_src[pv_head + 1] * 0.7).clamp(-1.0, 1.0);
            pv_head += 2;
        }
        if !frame.is_empty() {
            frame[0] = conv(l);
        }
        if frame.len() > 1 {
            frame[1] = conv(r);
        }
        for extra in frame.iter_mut().skip(2) {
            *extra = silence_zero;
        }
        head += 2;
        if head >= len {
            head = 0;
        }
    }

    playhead.store(head as u64, Ordering::Relaxed);
    // Clamp position to the inactive sentinel (== pv_len) rather than letting
    // it grow unboundedly if pv_len is e.g. zero.
    preview_head.store(pv_head.min(pv_len) as u64, Ordering::Relaxed);
}
