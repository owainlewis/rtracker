use std::path::Path;

use hound::{SampleFormat, WavSpec, WavWriter};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum WavError {
    #[error(transparent)]
    Hound(#[from] hound::Error),
}

/// Write an interleaved stereo f32 buffer to a 32-bit-float WAV.
pub fn write_stereo_f32<P: AsRef<Path>>(
    path: P,
    sample_rate: u32,
    interleaved: &[f32],
) -> Result<(), WavError> {
    let spec = WavSpec {
        channels: 2,
        sample_rate,
        bits_per_sample: 32,
        sample_format: SampleFormat::Float,
    };
    let mut w = WavWriter::create(path, spec)?;
    for s in interleaved {
        w.write_sample(*s)?;
    }
    w.finalize()?;
    Ok(())
}
