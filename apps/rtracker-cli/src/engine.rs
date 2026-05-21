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

        let stream = build_stream(
            &device,
            &stream_config,
            sample_format,
            channels,
            buffer.clone(),
            playhead.clone(),
            paused.clone(),
        )?;
        stream.play().context("start stream")?;

        Ok(Self {
            device_sr,
            device_name,
            channels,
            buffer,
            playhead,
            paused,
            _stream: stream,
        })
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

fn build_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    fmt: SampleFormat,
    channels: usize,
    buffer: Arc<ArcSwap<Vec<f32>>>,
    playhead: Arc<AtomicU64>,
    paused: Arc<AtomicBool>,
) -> Result<cpal::Stream> {
    let err_fn = |e| tracing::error!(error = %e, "stream error");
    let chans = channels;
    let stream = match fmt {
        SampleFormat::F32 => {
            let buf = buffer.clone();
            let ph = playhead.clone();
            let pa = paused.clone();
            device.build_output_stream(
                config,
                move |out: &mut [f32], _| fill(out, chans, &buf, &ph, &pa, |x| x),
                err_fn,
                None,
            )?
        }
        SampleFormat::I16 => {
            let buf = buffer.clone();
            let ph = playhead.clone();
            let pa = paused.clone();
            device.build_output_stream(
                config,
                move |out: &mut [i16], _| {
                    fill(out, chans, &buf, &ph, &pa, |x| {
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
            device.build_output_stream(
                config,
                move |out: &mut [u16], _| {
                    fill(out, chans, &buf, &ph, &pa, |x| {
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

fn fill<T, F>(
    out: &mut [T],
    channels: usize,
    buf: &Arc<ArcSwap<Vec<f32>>>,
    playhead: &Arc<AtomicU64>,
    paused: &Arc<AtomicBool>,
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

    let mut head = playhead.load(Ordering::Relaxed) as usize % len;
    if head & 1 != 0 {
        head -= 1; // keep frame-aligned
    }

    for frame in out.chunks_mut(channels) {
        let l = src[head];
        let r = src[head + 1];
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
}
