//! Loaded sample bank — reads every `SampleRef` referenced by a piece into
//! memory as mono f32 once, before rendering.
//!
//! Notes:
//! - Multi-channel WAVs are folded to mono by averaging channels.
//! - The sample's native sample rate is *not* resampled to the piece's rate.
//!   A 44.1 kHz sample played in a 48 kHz piece will be pitched up by ~8.8%
//!   (because we treat each file sample as one piece sample). This matches
//!   tracker convention; use `pitch_ratio` on the event to compensate.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use hound::SampleFormat;
use rtracker_core::Piece;
use thiserror::Error;

#[derive(Debug, Default)]
pub struct SampleBank {
    pub samples: HashMap<String, Arc<Vec<f32>>>,
}

#[derive(Debug, Error)]
pub enum SampleLoadError {
    #[error("sample '{id}' at {path}: {source}")]
    Read { id: String, path: PathBuf, #[source] source: hound::Error },
    #[error("sample '{id}' has unsupported bits_per_sample {bits}")]
    UnsupportedBits { id: String, bits: u16 },
    #[error("sample '{id}' slice [{start}..{end}] is out of range (length {len})")]
    SliceOutOfRange { id: String, start: u64, end: u64, len: u64 },
}

impl SampleBank {
    pub fn empty() -> Self { Self::default() }

    /// Load every sample referenced by the piece. Relative paths are resolved
    /// against `base_dir`.
    pub fn load(piece: &Piece, base_dir: &Path) -> Result<Self, SampleLoadError> {
        let mut samples = HashMap::new();
        for (id, sref) in &piece.samples {
            let full = if sref.path.is_absolute() {
                sref.path.clone()
            } else {
                base_dir.join(&sref.path)
            };
            let mono = read_wav_as_mono(id, &full)?;
            let start = sref.start_sample as usize;
            let end = if sref.end_sample == 0 {
                mono.len()
            } else {
                sref.end_sample as usize
            };
            if start > mono.len() || end > mono.len() || start > end {
                return Err(SampleLoadError::SliceOutOfRange {
                    id: id.clone(),
                    start: sref.start_sample,
                    end: sref.end_sample,
                    len: mono.len() as u64,
                });
            }
            let sliced = if start == 0 && sref.end_sample == 0 {
                // convention: end_sample=0 means "to end of file"
                mono
            } else {
                mono[start..end].to_vec()
            };
            samples.insert(id.clone(), Arc::new(sliced));
        }
        Ok(SampleBank { samples })
    }
}

fn read_wav_as_mono(id: &str, path: &Path) -> Result<Vec<f32>, SampleLoadError> {
    let reader = hound::WavReader::open(path)
        .map_err(|e| SampleLoadError::Read { id: id.into(), path: path.into(), source: e })?;
    let spec = reader.spec();
    let channels = spec.channels as usize;
    let interleaved: Vec<f32> = match spec.sample_format {
        SampleFormat::Float => reader
            .into_samples::<f32>()
            .filter_map(Result::ok)
            .collect(),
        SampleFormat::Int => match spec.bits_per_sample {
            16 => reader.into_samples::<i16>().filter_map(Result::ok)
                .map(|s| s as f32 / i16::MAX as f32).collect(),
            24 | 32 => {
                let max = (1i64 << (spec.bits_per_sample - 1)) as f32;
                reader.into_samples::<i32>().filter_map(Result::ok)
                    .map(|s| s as f32 / max).collect()
            }
            8 => reader.into_samples::<i16>().filter_map(Result::ok)
                .map(|s| s as f32 / i8::MAX as f32).collect(),
            bits => return Err(SampleLoadError::UnsupportedBits { id: id.into(), bits }),
        },
    };
    if channels <= 1 {
        return Ok(interleaved);
    }
    let frames = interleaved.len() / channels;
    let mut mono = Vec::with_capacity(frames);
    for f in 0..frames {
        let mut sum = 0.0f32;
        for c in 0..channels {
            sum += interleaved[f * channels + c];
        }
        mono.push(sum / channels as f32);
    }
    Ok(mono)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    use hound::{SampleFormat, WavSpec, WavWriter};
    use rtracker_core::{PieceMetadata, SampleRef};

    #[test]
    fn end_sample_zero_slices_from_start_to_end_of_file() {
        let dir = temp_test_dir();
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let wav = dir.join("slice.wav");
        write_test_wav(&wav, &[0, 1024, 2048, 4096]);

        let mut samples = HashMap::new();
        samples.insert(
            "s".into(),
            SampleRef {
                path: wav.file_name().unwrap().into(),
                start_sample: 2,
                end_sample: 0,
                label: None,
            },
        );
        let piece = Piece {
            sample_rate: 48000,
            duration_samples: 100,
            voices: HashMap::new(),
            samples,
            events: vec![],
            metadata: PieceMetadata::default(),
        };

        let bank = SampleBank::load(&piece, &dir).expect("load sample bank");
        let loaded = bank.samples.get("s").expect("sample exists");
        assert_eq!(loaded.len(), 2);
        assert!((loaded[0] - 2048.0 / i16::MAX as f32).abs() < 1e-6);
        assert!((loaded[1] - 4096.0 / i16::MAX as f32).abs() < 1e-6);

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn explicit_sample_end_still_slices_exclusively() {
        let dir = temp_test_dir();
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let wav = dir.join("slice.wav");
        write_test_wav(&wav, &[0, 1024, 2048, 4096]);

        let mut samples = HashMap::new();
        samples.insert(
            "s".into(),
            SampleRef {
                path: wav.file_name().unwrap().into(),
                start_sample: 1,
                end_sample: 3,
                label: None,
            },
        );
        let piece = Piece {
            sample_rate: 48000,
            duration_samples: 100,
            voices: HashMap::new(),
            samples,
            events: vec![],
            metadata: PieceMetadata::default(),
        };

        let bank = SampleBank::load(&piece, &dir).expect("load sample bank");
        let loaded = bank.samples.get("s").expect("sample exists");
        assert_eq!(loaded.len(), 2);
        assert!((loaded[0] - 1024.0 / i16::MAX as f32).abs() < 1e-6);
        assert!((loaded[1] - 2048.0 / i16::MAX as f32).abs() < 1e-6);

        let _ = std::fs::remove_dir_all(dir);
    }

    fn write_test_wav(path: &Path, samples: &[i16]) {
        let spec = WavSpec {
            channels: 1,
            sample_rate: 48000,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        };
        let mut writer = WavWriter::create(path, spec).expect("create wav");
        for sample in samples {
            writer.write_sample(*sample).expect("write sample");
        }
        writer.finalize().expect("finalize wav");
    }

    fn temp_test_dir() -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir().join(format!("rtracker_sample_bank_{}_{}", std::process::id(), nanos))
    }

}
