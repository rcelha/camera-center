use std::path::PathBuf;

use anyhow::{Result, bail};
use cc_core::SegmentationModel;

/// Environment variable overriding the models directory.
pub const MODELS_DIR_ENV: &str = "CAMERA_CENTER_MODELS_DIR";

/// Locates the models directory. Checked in order:
/// 1. `$CAMERA_CENTER_MODELS_DIR`
/// 2. `<exe dir>/models`
/// 3. `<exe dir>/../Resources/models` (macOS app bundle)
/// 4. `<exe dir>/../share/camera-center/models` (Linux `just install-linux`)
/// 5. the workspace `models/` directory (development builds)
pub fn models_dir() -> Result<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(dir) = std::env::var_os(MODELS_DIR_ENV) {
        candidates.push(PathBuf::from(dir));
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        candidates.push(dir.join("models"));
        candidates.push(dir.join("../Resources/models"));
        candidates.push(dir.join("../share/camera-center/models"));
    }
    candidates.push(PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../models"
    )));

    for dir in &candidates {
        if dir.join("models.toml").is_file() {
            return Ok(dir.canonicalize().unwrap_or_else(|_| dir.clone()));
        }
    }
    bail!(
        "models directory not found (looked in {candidates:?}); set {MODELS_DIR_ENV} or run `git lfs pull`"
    )
}

/// Path of the ONNX file for `model`.
pub fn model_path(model: SegmentationModel) -> Result<PathBuf> {
    let path = models_dir()?.join(model.file_name());
    if !path.is_file() {
        bail!("model file {} is missing", path.display());
    }
    // A Git LFS pointer is a ~130 byte text file starting with this line. The
    // smallest real model (selfie_segmenter) is ~450 KB, so check the content
    // rather than the size.
    let mut head = [0u8; 24];
    let n = std::io::Read::read(&mut std::fs::File::open(&path)?, &mut head)?;
    if head[..n].starts_with(b"version https://git-lfs") {
        bail!(
            "{} is a Git LFS pointer, run `git lfs pull`",
            path.display()
        );
    }
    Ok(path)
}
