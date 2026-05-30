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
        VoiceDef::Fm { modulator_ratio, modulation_index, .. } => {
            render_fm(event, ctx.sample_rate, *modulator_ratio, *modulation_index, out)
        }
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

/// Two-operator FM: a single modulator sine phase-modulates a carrier sine.
/// `modulator_ratio` is the modulator frequency relative to the carrier;
/// `modulation_index` is the peak phase deviation in radians. The carrier
/// frequency tracks the event's pitch envelope.
fn render_fm(
    event: &Event,
    sample_rate: u32,
    modulator_ratio: f32,
    modulation_index: f32,
    out: &mut [f32],
) {
    let sr = sample_rate as f32;
    let mut carrier_phase = 0.0f32;
    let mut mod_phase = 0.0f32;
    for (i, s) in out.iter_mut().enumerate() {
        let f = freq_at(event, i as u64);
        *s = (carrier_phase + modulation_index * mod_phase.sin()).sin();
        carrier_phase = wrap_tau(carrier_phase + TAU * f / sr);
        mod_phase = wrap_tau(mod_phase + TAU * f * modulator_ratio / sr);
    }
}

fn wrap_tau(phase: f32) -> f32 {
    if phase > TAU {
        phase - TAU
    } else {
        phase
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
    // A one-sample source can't be interpolated or folded — just hold it.
    if len == 1 {
        for s in out.iter_mut() {
            *s = src[0];
        }
        return;
    }
    let mut pos = 0.0f64;
    for s in out.iter_mut() {
        let (i_lo, frac) = match loop_mode {
            SampleLoopMode::OneShot => (pos.floor() as usize, (pos - pos.floor()) as f32),
            SampleLoopMode::Loop => {
                let p = pos % len as f64;
                (p.floor() as usize, (p - p.floor()) as f32)
            }
            SampleLoopMode::PingPong => {
                // Fold position into a triangle over [0, len-1]: forward then
                // backward, so the loop reverses at each end instead of jumping.
                let period = 2.0 * (len - 1) as f64;
                let q = pos % period;
                let folded = if q <= (len - 1) as f64 { q } else { period - q };
                (folded.floor() as usize, (folded - folded.floor()) as f32)
            }
        };
        let (a, b) = match loop_mode {
            SampleLoopMode::OneShot => {
                if i_lo + 1 >= len {
                    if i_lo < len { (src[i_lo], 0.0) } else { (0.0, 0.0) }
                } else {
                    (src[i_lo], src[i_lo + 1])
                }
            }
            // Looping modes keep i_lo in range; the next sample wraps/clamps.
            SampleLoopMode::Loop => (src[i_lo % len], src[(i_lo + 1) % len]),
            SampleLoopMode::PingPong => (src[i_lo.min(len - 1)], src[(i_lo + 1).min(len - 1)]),
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
    fn fm_voice_produces_bounded_signal() {
        let def = VoiceDef::Fm { modulator_ratio: 2.0, modulation_index: 3.0, default_pan: 0.0 };
        let mut buf = vec![0.0; 4800];
        render_voice(&def, &ev(220.0, 4800), &empty_ctx(), &mut buf);
        assert!(nonzero(&buf));
        assert!(buf.iter().all(|s| s.abs() <= 1.0001));
    }

    #[test]
    fn fm_zero_index_is_a_plain_sine() {
        // With modulation_index 0 the carrier is unmodulated, so it must match a
        // plain sine of the same frequency.
        let def = VoiceDef::Fm { modulator_ratio: 2.0, modulation_index: 0.0, default_pan: 0.0 };
        let mut fm = vec![0.0; 480];
        render_voice(&def, &ev(440.0, 480), &empty_ctx(), &mut fm);
        let mut sine = vec![0.0; 480];
        render_sine(&ev(440.0, 480), 48000, &mut sine);
        for (a, b) in fm.iter().zip(sine.iter()) {
            assert!((a - b).abs() < 1e-5);
        }
    }

    #[test]
    fn pingpong_reverses_at_the_end() {
        // A monotonically rising ramp played at 2× should run up to the end and
        // then come back down, rather than jumping back to the start.
        let src: Vec<f32> = (0..8).map(|i| i as f32).collect();
        let mut out = vec![0.0; 16];
        let mut e = ev(0.0, 16);
        e.pitch_ratio = Some(2.0);
        render_sample(&e, &src, SampleLoopMode::PingPong, &mut out);
        // It rises, peaks near the top, then descends — so the max is interior,
        // not at the final sample.
        let max_idx = out.iter().enumerate().max_by(|a, b| a.1.partial_cmp(b.1).unwrap()).unwrap().0;
        assert!(max_idx < out.len() - 1, "expected interior peak, got idx {max_idx}");
        assert!(*out.last().unwrap() < out[max_idx], "tail should descend after the fold");
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
