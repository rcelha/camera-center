//! Persisted user configuration.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Upper bound accepted for `blur_strength`.
pub const MAX_BLUR_STRENGTH: u8 = 100;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    /// Human readable name of the capture device. `None` = first available.
    pub input_device: Option<String>,
    /// Caps the processing (and virtual camera) resolution.
    pub resolution: ProcessingResolution,
    /// Segmentation model used by background effects and portrait mode.
    pub model: SegmentationModel,
    /// Image used by the "background image" effect.
    pub background_image: Option<PathBuf>,
    /// Background blur strength, 0..=100.
    pub blur_strength: u8,
    pub effects: EffectToggles,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            input_device: None,
            resolution: ProcessingResolution::P720,
            model: SegmentationModel::U2netHumanSeg,
            background_image: None,
            blur_strength: 50,
            effects: EffectToggles::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EffectToggles {
    pub blur_background: bool,
    pub background_image: bool,
    pub portrait: bool,
}

impl EffectToggles {
    pub fn any(&self) -> bool {
        self.blur_background || self.background_image || self.portrait
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessingResolution {
    /// Whatever the camera delivers.
    Native,
    #[serde(rename = "1080p")]
    P1080,
    #[serde(rename = "720p")]
    P720,
    #[serde(rename = "480p")]
    P480,
}

impl ProcessingResolution {
    pub const ALL: [ProcessingResolution; 4] = [Self::Native, Self::P1080, Self::P720, Self::P480];

    /// Maximum frame height, `None` for native.
    pub fn max_height(self) -> Option<u32> {
        match self {
            Self::Native => None,
            Self::P1080 => Some(1080),
            Self::P720 => Some(720),
            Self::P480 => Some(480),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Native => "Native (camera)",
            Self::P1080 => "1080p",
            Self::P720 => "720p",
            Self::P480 => "480p",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SegmentationModel {
    /// U²-Net trained for human segmentation (~176 MB, best quality).
    U2netHumanSeg,
    /// U²-Net small (~4.6 MB, fastest).
    U2netp,
}

impl SegmentationModel {
    pub const ALL: [SegmentationModel; 2] = [Self::U2netHumanSeg, Self::U2netp];

    /// File name inside the models directory (see `models/models.toml`).
    pub fn file_name(self) -> &'static str {
        match self {
            Self::U2netHumanSeg => "u2net_human_seg.onnx",
            Self::U2netp => "u2netp.onnx",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::U2netHumanSeg => "U²-Net human seg (quality)",
            Self::U2netp => "U²-Net-p (fast)",
        }
    }
}

impl AppConfig {
    /// Loads the config from `path`, returning defaults if it does not exist.
    pub fn load(path: &Path) -> Result<Self> {
        match fs::read_to_string(path) {
            Ok(text) => Self::from_toml(&text)
                .with_context(|| format!("failed to parse config {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("failed to read config {}", path.display())),
        }
    }

    pub fn from_toml(text: &str) -> Result<Self> {
        let cfg: AppConfig = toml::from_str(text)?;
        Ok(cfg.sanitized())
    }

    pub fn to_toml(&self) -> Result<String> {
        Ok(toml::to_string_pretty(self)?)
    }

    /// Writes the config atomically (write to a sibling file, then rename).
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)
                .with_context(|| format!("failed to create {}", dir.display()))?;
        }
        let tmp = path.with_extension("toml.partial");
        {
            let mut f = fs::File::create(&tmp)
                .with_context(|| format!("failed to create {}", tmp.display()))?;
            f.write_all(self.to_toml()?.as_bytes())?;
            f.sync_all()?;
        }
        fs::rename(&tmp, path).with_context(|| format!("failed to write {}", path.display()))
    }

    /// Clamps out-of-range values coming from hand-edited files.
    pub fn sanitized(mut self) -> Self {
        self.blur_strength = self.blur_strength.min(MAX_BLUR_STRENGTH);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let cfg = AppConfig {
            input_device: Some("FaceTime HD Camera".into()),
            resolution: ProcessingResolution::P1080,
            model: SegmentationModel::U2netp,
            background_image: Some(PathBuf::from("/tmp/bg.png")),
            blur_strength: 70,
            effects: EffectToggles {
                blur_background: true,
                background_image: true,
                portrait: false,
            },
        };
        let text = cfg.to_toml().unwrap();
        assert!(text.contains("resolution = \"1080p\""));
        assert!(text.contains("model = \"u2netp\""));
        assert_eq!(AppConfig::from_toml(&text).unwrap(), cfg);
    }

    #[test]
    fn missing_fields_use_defaults() {
        let cfg = AppConfig::from_toml("blur_strength = 10").unwrap();
        assert_eq!(cfg.blur_strength, 10);
        assert_eq!(cfg.model, SegmentationModel::U2netHumanSeg);
        assert_eq!(cfg.effects, EffectToggles::default());
    }

    #[test]
    fn clamps_blur() {
        let cfg = AppConfig::from_toml("blur_strength = 250").unwrap();
        assert_eq!(cfg.blur_strength, MAX_BLUR_STRENGTH);
    }

    #[test]
    fn save_and_load() {
        let dir = std::env::temp_dir().join(format!("cc-core-test-{}", std::process::id()));
        let path = dir.join("config.toml");
        let cfg = AppConfig {
            blur_strength: 33,
            ..Default::default()
        };
        cfg.save(&path).unwrap();
        assert_eq!(AppConfig::load(&path).unwrap(), cfg);
        fs::remove_dir_all(&dir).unwrap();
        assert_eq!(AppConfig::load(&path).unwrap(), AppConfig::default());
    }
}
