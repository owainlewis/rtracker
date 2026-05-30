use std::f32::consts::FRAC_PI_2;
use std::path::Path;

use rtracker_core::{Event, Piece, VoiceDef};
use thiserror::Error;

use crate::envelope::apply_envelope;
use crate::fx::apply_fx;
use crate::sample::{SampleBank, SampleLoadError};
use crate::voice::{render_voice, VoiceCtx};

#[derive(Debug, Error)]
pub enum RenderError {
    #[error(transparent)]
    Validation(#[from] rtracker_core::ValidationError),
    #[error(transparent)]
    SampleLoad(#[from] SampleLoadError),
    #[error("event {0} references voice '{1}' which is not in the palette")]
    MissingVoice(usize, String),
}

/// Render with samples resolved relative to the current working directory.
pub fn render(piece: &Piece) -> Result<Vec<f32>, RenderError> {
    render_with_dir(piece, Path::new("."))
}

/// Render, resolving any sample paths relative to `base_dir`.
pub fn render_with_dir(piece: &Piece, base_dir: &Path) -> Result<Vec<f32>, RenderError> {
    // Validate before touching the filesystem so a malformed piece fails fast
    // with a validation error rather than a sample-load error. `render_with_bank`
    // does not re-validate.
    piece.validate()?;
    let bank = SampleBank::load(piece, base_dir)?;
    render_with_bank_unchecked(piece, &bank)
}

pub fn render_with_bank(piece: &Piece, bank: &SampleBank) -> Result<Vec<f32>, RenderError> {
    piece.validate()?;
    render_with_bank_unchecked(piece, bank)
}

fn render_with_bank_unchecked(piece: &Piece, bank: &SampleBank) -> Result<Vec<f32>, RenderError> {
    let total = (piece.duration_samples as usize) * 2;
    let mut master = vec![0.0f32; total];
    let ctx = VoiceCtx { sample_rate: piece.sample_rate, bank };

    for (i, event) in piece.events.iter().enumerate() {
        let voice = piece
            .voices
            .get(&event.voice)
            .ok_or_else(|| RenderError::MissingVoice(i, event.voice.clone()))?;
        mix_event(voice, event, &ctx, &mut master);
    }

    for s in master.iter_mut() {
        *s = s.clamp(-1.0, 1.0);
    }
    Ok(master)
}

fn mix_event(voice: &VoiceDef, event: &Event, ctx: &VoiceCtx, master: &mut [f32]) {
    if event.dur == 0 {
        return;
    }
    let dur = event.dur as usize;
    let mut buf = vec![0.0f32; dur];

    render_voice(voice, event, ctx, &mut buf);
    apply_envelope(&event.envelope, &mut buf);
    for fx in &event.fx {
        apply_fx(fx, &mut buf, ctx.sample_rate);
    }
    for s in buf.iter_mut() {
        *s *= event.amp;
    }
    declick(&mut buf, ctx.sample_rate);

    let (lg, rg) = pan_gains(event.pan);
    let start = event.t as usize * 2;
    let end = (start + dur * 2).min(master.len());
    let mut bi = 0;
    let mut mi = start;
    while mi + 1 < end {
        master[mi] += buf[bi] * lg;
        master[mi + 1] += buf[bi] * rg;
        bi += 1;
        mi += 2;
    }
}

/// Short linear fade at both edges of an event's buffer. Without it, a one-shot
/// sample cut off mid-waveform (e.g. a `gate` Amen chop truncated to one row) or
/// a note that starts/ends off a zero crossing leaves a step discontinuity when
/// summed into the master bus — heard as a broadband click ("blip"). A ~2.5 ms
/// ramp is inaudible against the transient but removes the click.
fn declick(buf: &mut [f32], sample_rate: u32) {
    let n = buf.len();
    if n < 4 {
        return;
    }
    // ~2.5 ms, clamped so a very short event still fades cleanly.
    let fade = ((sample_rate as usize) / 400).clamp(1, n / 2);
    for i in 0..fade {
        let g = (i as f32 + 0.5) / fade as f32;
        buf[i] *= g;
        buf[n - 1 - i] *= g;
    }
}

/// Equal-power pan. pan = -1 → fully L; 0 → centre (≈0.707 each); +1 → fully R.
fn pan_gains(pan: f32) -> (f32, f32) {
    let pan = pan.clamp(-1.0, 1.0);
    let theta = (pan + 1.0) * 0.5 * FRAC_PI_2;
    (theta.cos(), theta.sin())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    use rtracker_core::{Envelope, PieceMetadata, ValidationError};

    #[test]
    fn pan_centre_equal_power() {
        let (l, r) = pan_gains(0.0);
        assert!((l - r).abs() < 1e-6);
        assert!((l * l + r * r - 1.0).abs() < 1e-6);
    }

    #[test]
    fn pan_extremes() {
        let (l, r) = pan_gains(-1.0);
        assert!((l - 1.0).abs() < 1e-6 && r.abs() < 1e-6);
        let (l, r) = pan_gains(1.0);
        assert!(l.abs() < 1e-6 && (r - 1.0).abs() < 1e-6);
    }

    #[test]
    fn declick_ramps_edges_to_near_zero() {
        let mut buf = vec![1.0f32; 4800]; // 100 ms @ 48k
        declick(&mut buf, 48000);
        // Edges are pulled down; the body is untouched.
        assert!(buf[0] < 0.05);
        assert!(buf[buf.len() - 1] < 0.05);
        assert!((buf[2400] - 1.0).abs() < 1e-6);
        // No step larger than one fade increment anywhere along the ramp.
        let max_jump = buf.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
        assert!(max_jump < 0.02, "max jump {max_jump}");
    }

    #[test]
    fn render_rejects_mismatched_sine_partials_before_mixing() {
        let mut voices = HashMap::new();
        voices.insert(
            "v".into(),
            VoiceDef::SinePartials {
                partials: vec![1.0, 2.0],
                amplitudes: vec![1.0],
                default_pan: 0.0,
            },
        );
        let piece = Piece {
            sample_rate: 48000,
            duration_samples: 100,
            voices,
            samples: HashMap::new(),
            events: vec![Event {
                t: 0,
                voice: "v".into(),
                freq: Some(440.0),
                dur: 100,
                amp: 0.5,
                pan: 0.0,
                envelope: Envelope::Gate,
                fx: vec![],
                pitch_ratio: None,
                pitch_env: None,
            }],
            metadata: PieceMetadata::default(),
        };

        let err = render_with_bank(&piece, &SampleBank::empty()).expect_err("render should fail");
        assert!(matches!(
            err,
            RenderError::Validation(ValidationError::BadSinePartialsLength { .. })
        ));
    }
}
