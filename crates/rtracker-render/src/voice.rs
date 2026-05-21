use rtracker_core::{Event, PitchEnv, PitchShape, SampleLoopMode, VoiceDef};
use std::f32::consts::TAU;

use crate::dsp::Biquad;
use crate::sample::SampleBank;

pub struct VoiceCtx<'a> {
    pub sample_rate: u32,
    pub bank: &'a SampleBank,
}

pub fn render_voice(def: &VoiceDef, event: &Event, ctx: &VoiceCtx, out: &mut [f32]) {
    match def {
        VoiceDef::Sine { .. } => render_sine(event, ctx.sample_rate, out),
        VoiceDef::SinePartials { partials, amplitudes, .. } => {
            render_sine_partials(event, ctx.sample_rate, partials, amplitudes, out)
        }
        VoiceDef::NoiseBandpass { q, .. } => render_noise_bandpass(event, ctx.sample_rate, *q, out),
        VoiceDef::Sample { sample_id, loop_mode } => {
            match ctx.bank.samples.get(sample_id) {
                Some(buf) => render_sample(event, buf, *loop_mode, out),
                None => silence(out),
            }
        }
        VoiceDef::Fm { .. } => silence(out),
    }
}

fn silence(out: &mut [f32]) {
    for s in out.iter_mut() {
        *s = 0.0;
    }
}

/// Compute the instantaneous frequency at sample-offset `i` within the event,
/// applying the event's pitch envelope if any.
fn freq_at(event: &Event, i: u64) -> f32 {
    let base = event.freq.unwrap_or(440.0);
    let Some(p) = &event.pitch_env else { return base };
    base * pitch_env_ratio(p, i)
}

fn pitch_env_ratio(p: &PitchEnv, i: u64) -> f32 {
    let t_total = p.time_samples.max(1);
    let t = (i.min(t_total) as f32) / (t_total as f32);
    match p.shape {
        PitchShape::Linear => p.from_ratio + (p.to_ratio - p.from_ratio) * t,
        PitchShape::Exp => {
            let from = p.from_ratio.max(0.001);
            let to = p.to_ratio.max(0.001);
            from * (to / from).powf(t)
        }
    }
}

fn render_sine(event: &Event, sample_rate: u32, out: &mut [f32]) {
    let sr = sample_rate as f32;
    let mut phase = 0.0f32;
    for (i, s) in out.iter_mut().enumerate() {
        let f = freq_at(event, i as u64);
        let dt = TAU * f / sr;
        *s = phase.sin();
        phase += dt;
        if phase > TAU {
            phase -= TAU;
        }
    }
}

fn render_sine_partials(
    event: &Event,
    sample_rate: u32,
    partials: &[f32],
    amplitudes: &[f32],
    out: &mut [f32],
) {
    for s in out.iter_mut() {
        *s = 0.0;
    }
    let sr = sample_rate as f32;
    let norm = amplitudes.iter().map(|a| a.abs()).sum::<f32>().max(1e-6);
    let mut phases = vec![0.0f32; partials.len()];
    for (i, s) in out.iter_mut().enumerate() {
        let fund = freq_at(event, i as u64);
        for (k, p) in partials.iter().enumerate() {
            let f = fund * *p;
            let dt = TAU * f / sr;
            *s += (amplitudes[k] / norm) * phases[k].sin();
            phases[k] += dt;
            if phases[k] > TAU {
                phases[k] -= TAU;
            }
        }
    }
}

fn render_noise_bandpass(event: &Event, sample_rate: u32, q: f32, out: &mut [f32]) {
    let center = event.freq.unwrap_or(1000.0);
    let mut rng = XorShift::new(event.t.wrapping_add(0x9E37_79B9_7F4A_7C15));
    let mut bp = Biquad::bandpass(sample_rate as f32, center, q.max(0.1));
    for s in out.iter_mut() {
        let n = rng.next_f32() * 2.0 - 1.0;
        *s = bp.process(n);
    }
}

fn render_sample(event: &Event, src: &[f32], loop_mode: SampleLoopMode, out: &mut [f32]) {
    if src.is_empty() {
        silence(out);
        return;
    }
    let pitch_ratio = event.pitch_ratio.unwrap_or(1.0).max(0.001) as f64;
    let len = src.len();
    let mut pos = 0.0f64;
    for s in out.iter_mut() {
        let i_lo = pos.floor() as usize;
        let frac = (pos - pos.floor()) as f32;
        let (a, b) = match loop_mode {
            SampleLoopMode::OneShot => {
                if i_lo + 1 >= len {
                    if i_lo < len { (src[i_lo], 0.0) } else { (0.0, 0.0) }
                } else {
                    (src[i_lo], src[i_lo + 1])
                }
            }
            SampleLoopMode::Loop | SampleLoopMode::PingPong => {
                // PingPong currently degrades to forward Loop — Phase 2-and-a-half.
                let a = src[i_lo % len];
                let b = src[(i_lo + 1) % len];
                (a, b)
            }
        };
        *s = a * (1.0 - frac) + b * frac;
        pos += pitch_ratio;
    }
}

struct XorShift(u64);
impl XorShift {
    fn new(seed: u64) -> Self {
        Self(if seed == 0 { 0xDEAD_BEEF_CAFE_BABE } else { seed })
    }
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn next_f32(&mut self) -> f32 {
        ((self.next_u64() >> 40) as f32) / ((1u32 << 24) as f32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rtracker_core::Envelope;

    fn ev(freq: f32, dur: u64) -> Event {
        Event {
            t: 0,
            voice: "v".into(),
            freq: Some(freq),
            dur,
            amp: 1.0,
            pan: 0.0,
            envelope: Envelope::Gate,
            fx: vec![],
            pitch_ratio: None,
            pitch_env: None,
        }
    }

    fn empty_ctx() -> VoiceCtx<'static> {
        // SAFETY: we leak a SampleBank with no entries — fine for tests.
        let bank: &'static SampleBank = Box::leak(Box::new(SampleBank::empty()));
        VoiceCtx { sample_rate: 48000, bank }
    }

    fn nonzero(buf: &[f32]) -> bool {
        buf.iter().any(|s| s.abs() > 1e-6)
    }

    #[test]
    fn sine_voice_produces_signal() {
        let mut buf = vec![0.0; 480];
        render_voice(&VoiceDef::Sine { default_pan: 0.0 }, &ev(440.0, 480), &empty_ctx(), &mut buf);
        assert!(nonzero(&buf));
        assert!(buf.iter().all(|s| s.abs() <= 1.0001));
    }

    #[test]
    fn partials_voice_produces_signal_within_unit() {
        let def = VoiceDef::SinePartials {
            partials: vec![1.0, 2.0, 3.0],
            amplitudes: vec![1.0, 0.5, 0.25],
            default_pan: 0.0,
        };
        let mut buf = vec![0.0; 480];
        render_voice(&def, &ev(220.0, 480), &empty_ctx(), &mut buf);
        assert!(nonzero(&buf));
        assert!(buf.iter().all(|s| s.abs() <= 1.0001));
    }

    #[test]
    fn noise_bandpass_voice_produces_signal() {
        let def = VoiceDef::NoiseBandpass { q: 5.0, default_pan: 0.0 };
        let mut buf = vec![0.0; 4800];
        render_voice(&def, &ev(1000.0, 4800), &empty_ctx(), &mut buf);
        assert!(nonzero(&buf));
    }

    #[test]
    fn pitch_env_lowers_frequency_over_time() {
        // base 100 Hz, ratio 4→1 over 100 samples: starts at 400 Hz, ends at 100 Hz.
        let mut e = ev(100.0, 200);
        e.pitch_env = Some(PitchEnv {
            from_ratio: 4.0, to_ratio: 1.0,
            time_samples: 100, shape: PitchShape::Exp,
        });
        let start = freq_at(&e, 0);
        let mid   = freq_at(&e, 50);
        let end   = freq_at(&e, 100);
        let after = freq_at(&e, 150);
        assert!((start - 400.0).abs() < 0.01);
        assert!(mid > 100.0 && mid < 400.0);
        assert!((end - 100.0).abs() < 0.01);
        assert!((after - 100.0).abs() < 0.01);    // stays at to_ratio after time_samples
    }
}
