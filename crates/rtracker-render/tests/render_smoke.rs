use std::path::PathBuf;

use rtracker_core::Piece;

fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is crates/rtracker-render.
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    crate_dir.parent().unwrap().parent().unwrap().to_path_buf()
}

#[test]
fn renders_handmade_smoke_to_wav() {
    let root = workspace_root();
    let piece_path = root.join("examples").join("handmade_smoke.json");
    let text = std::fs::read_to_string(&piece_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", piece_path.display()));
    let piece: Piece = serde_json::from_str(&text).expect("parse piece");
    piece.validate().expect("validate piece");

    let buf = rtracker_render::render(&piece).expect("render");
    assert_eq!(buf.len() as u64, piece.duration_samples * 2);
    let nonzero = buf.iter().filter(|s| s.abs() > 1e-6).count();
    assert!(nonzero > buf.len() / 10, "expected audible signal, got {nonzero} non-zero samples");
    assert!(buf.iter().all(|s| (-1.0..=1.0).contains(s)), "samples must be clamped to [-1, 1]");

    let out = std::env::temp_dir().join("rtracker_smoke_test.wav");
    rtracker_render::write_stereo_f32(&out, piece.sample_rate, &buf).expect("write wav");
    let meta = std::fs::metadata(&out).expect("wav exists");
    assert!(meta.len() > 1024, "wav file too small: {} bytes", meta.len());
}
