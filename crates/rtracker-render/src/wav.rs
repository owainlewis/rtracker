use std::path::Path;

use hound::{SampleFormat, WavSpec, WavWriter};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum WavError {
    #[error(transparent)]
    Hound(#[from] hound::Error),
}

/// A WAV whose integer sample width the decoder doesn't handle.
#[derive(Debug, Error)]
#[error("unsupported bits_per_sample {0}")]
pub struct UnsupportedBits(pub u16);

/// Decode an opened WAV reader to mono f32 plus its native sample rate.
///
/// Integer PCM is scaled to [-1, 1]; 8-bit is read as `i8` (hound applies the
/// unsigned 128 bias for us). Multi-channel audio is folded to mono by averaging
/// channels. The native rate is returned untouched — callers that don't resample
/// can ignore it. Shared by the sample bank and the CLI slicer.
pub fn decode_wav_mono<R: std::io::Read>(
    reader: hound::WavReader<R>,
) -> Result<(Vec<f32>, u32), UnsupportedBits> {
    let spec = reader.spec();
    let channels = spec.channels.max(1) as usize;
    let interleaved: Vec<f32> = match spec.sample_format {
        SampleFormat::Float => reader.into_samples::<f32>().filter_map(Result::ok).collect(),
        SampleFormat::Int => match spec.bits_per_sample {
            16 => reader.into_samples::<i16>().filter_map(Result::ok)
                .map(|s| s as f32 / i16::MAX as f32).collect(),
            24 | 32 => {
                let max = (1i64 << (spec.bits_per_sample - 1)) as f32;
                reader.into_samples::<i32>().filter_map(Result::ok)
                    .map(|s| s as f32 / max).collect()
            }
            8 => reader.into_samples::<i8>().filter_map(Result::ok)
                .map(|s| s as f32 / i8::MAX as f32).collect(),
            bits => return Err(UnsupportedBits(bits)),
        },
    };
    if channels <= 1 {
        return Ok((interleaved, spec.sample_rate));
    }
    let frames = interleaved.len() / channels;
    let mut mono = Vec::with_capacity(frames);
    for f in 0..frames {
        let sum: f32 = (0..channels).map(|c| interleaved[f * channels + c]).sum();
        mono.push(sum / channels as f32);
    }
    Ok((mono, spec.sample_rate))
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
