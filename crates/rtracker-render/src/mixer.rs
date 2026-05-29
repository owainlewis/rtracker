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
    piece.validate()?;
    let bank = SampleBank::load(piece, base_dir)?;
    render_with_bank(piece, &bank)
}

pub fn render_with_bank(piece: &Piece, bank: &SampleBank) -> Result<Vec<f32>, RenderError> {
    piece.validate()?;
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
