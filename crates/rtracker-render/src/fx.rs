use rtracker_core::FxNode;

use crate::dsp::Biquad;

pub fn apply_fx(node: &FxNode, buf: &mut [f32], sample_rate: u32) {
    match *node {
        FxNode::Bitcrush { bits } => bitcrush(buf, bits),
        FxNode::Reverse => buf.reverse(),
        FxNode::SampleRateReduce { factor } => sample_rate_reduce(buf, factor),
        FxNode::Stutter { slice_samples, repeats } => stutter(buf, slice_samples, repeats),
        FxNode::CombDelay { delay_samples, feedback } => comb_delay(buf, delay_samples, feedback),
        FxNode::Lowpass { cutoff_hz, q } => {
            biquad(buf, Biquad::lowpass(sample_rate as f32, cutoff_hz, q))
        }
        FxNode::Highpass { cutoff_hz, q } => {
            biquad(buf, Biquad::highpass(sample_rate as f32, cutoff_hz, q))
        }
        FxNode::Bandpass { center_hz, q } => {
            biquad(buf, Biquad::bandpass(sample_rate as f32, center_hz, q))
        }
    }
}

fn biquad(buf: &mut [f32], mut filter: Biquad) {
    for s in buf.iter_mut() {
        *s = filter.process(*s);
    }
}

fn bitcrush(buf: &mut [f32], bits: u8) {
    let bits = bits.clamp(1, 16) as i32;
    let levels = (1i32 << (bits - 1)) as f32;
    for s in buf.iter_mut() {
        let clamped = s.clamp(-1.0, 1.0);
        let q = (clamped * levels).round() / levels;
        *s = q;
    }
}

fn sample_rate_reduce(buf: &mut [f32], factor: u32) {
    if factor <= 1 {
        return;
    }
    let factor = factor as usize;
    let mut held = 0.0f32;
    for (i, s) in buf.iter_mut().enumerate() {
        if i % factor == 0 {
            held = *s;
        }
        *s = held;
    }
}

fn stutter(buf: &mut [f32], slice_samples: u64, repeats: u32) {
    if slice_samples == 0 || repeats == 0 || buf.is_empty() {
        return;
    }
    let slice = (slice_samples as usize).min(buf.len());
    let source: Vec<f32> = buf[..slice].to_vec();
    let total = (slice * repeats as usize).min(buf.len());
    for i in 0..total {
        buf[i] = source[i % slice];
    }
}

fn comb_delay(buf: &mut [f32], delay_samples: u64, feedback: f32) {
    if delay_samples == 0 || buf.is_empty() {
        return;
    }
    let delay = delay_samples as usize;
    let fb = feedback.clamp(-0.99, 0.99);
    let mut line = vec![0.0f32; delay];
    let mut idx = 0usize;
    for s in buf.iter_mut() {
        let delayed = line[idx];
        let out = *s + fb * delayed;
        line[idx] = out;
        *s = out;
        idx += 1;
        if idx >= delay {
            idx = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reverse_reverses() {
        let mut b = vec![1.0f32, 2.0, 3.0, 4.0];
        apply_fx(&FxNode::Reverse, &mut b, 48000);
        assert_eq!(b, vec![4.0, 3.0, 2.0, 1.0]);
    }

    #[test]
    fn stutter_repeats_first_slice() {
        let mut b: Vec<f32> = (0..10).map(|i| i as f32).collect();
        apply_fx(&FxNode::Stutter { slice_samples: 3, repeats: 3 }, &mut b, 48000);
        // First slice = [0,1,2], repeated 3 times = [0,1,2,0,1,2,0,1,2], last sample untouched.
        assert_eq!(&b[..9], &[0.0, 1.0, 2.0, 0.0, 1.0, 2.0, 0.0, 1.0, 2.0]);
        assert_eq!(b[9], 9.0);
    }

    #[test]
    fn comb_delay_repeats_impulse() {
        let mut b = vec![0.0f32; 20];
        b[0] = 1.0;
        apply_fx(&FxNode::CombDelay { delay_samples: 5, feedback: 0.5 }, &mut b, 48000);
        // Impulse should re-appear at n=5, n=10, n=15 with decaying amp.
        assert!((b[0] - 1.0).abs() < 1e-6);
        assert!((b[5] - 0.5).abs() < 1e-6);
        assert!((b[10] - 0.25).abs() < 1e-6);
        assert!((b[15] - 0.125).abs() < 1e-6);
    }

    /// RMS of a unit sine at `freq` after passing through `node`. Used to check
    /// that filters pass/reject the bands they should.
    fn filtered_rms(node: FxNode, freq: f32, sr: u32) -> f32 {
        use std::f32::consts::TAU;
        let n = sr as usize; // one second
        let mut buf: Vec<f32> = (0..n).map(|i| (TAU * freq * i as f32 / sr as f32).sin()).collect();
        apply_fx(&node, &mut buf, sr);
        // Skip the filter's startup transient.
        let tail = &buf[sr as usize / 10..];
        (tail.iter().map(|s| s * s).sum::<f32>() / tail.len() as f32).sqrt()
    }

    #[test]
    fn lowpass_attenuates_above_cutoff_and_passes_below() {
        let low = filtered_rms(FxNode::Lowpass { cutoff_hz: 1000.0, q: 0.707 }, 100.0, 48000);
        let high = filtered_rms(FxNode::Lowpass { cutoff_hz: 1000.0, q: 0.707 }, 10000.0, 48000);
        assert!(low > 0.6, "low tone should pass, rms {low}");
        assert!(high < 0.1, "high tone should be cut, rms {high}");
    }

    #[test]
    fn highpass_attenuates_below_cutoff_and_passes_above() {
        let low = filtered_rms(FxNode::Highpass { cutoff_hz: 1000.0, q: 0.707 }, 100.0, 48000);
        let high = filtered_rms(FxNode::Highpass { cutoff_hz: 1000.0, q: 0.707 }, 10000.0, 48000);
        assert!(high > 0.6, "high tone should pass, rms {high}");
        assert!(low < 0.1, "low tone should be cut, rms {low}");
    }

    #[test]
    fn bandpass_passes_center_rejects_far_bands() {
        let node = FxNode::Bandpass { center_hz: 1000.0, q: 2.0 };
        let center = filtered_rms(node, 1000.0, 48000);
        let below = filtered_rms(node, 60.0, 48000);
        let above = filtered_rms(node, 12000.0, 48000);
        assert!(center > below * 3.0 && center > above * 3.0, "center {center} below {below} above {above}");
    }

    #[test]
    fn bitcrush_quantizes() {
        let mut b = vec![0.1f32, 0.51, -0.6];
        apply_fx(&FxNode::Bitcrush { bits: 2 }, &mut b, 48000);
        // 2-bit signed → levels = 2, step 0.5.
        for s in &b {
            let nearest = (s * 2.0).round() / 2.0;
            assert!((s - nearest).abs() < 1e-6);
        }
    }
}
