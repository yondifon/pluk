//! Window frame persistence and content-minimum clamping.
//!
//! The Swift app saves its frame via `NSWindow.setFrameAutosaveName` and
//! then clamps a restored frame that is smaller than `contentMinSize`
//! (720×520). We replicate that with a plain JSON file in the app config
//! directory because `tauri-plugin-window-state` is not yet adopted.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Default window rect matching `NSWindow(contentRect: 1040×660 …)`.
pub const DEFAULT_WIDTH: f64 = 1040.0;
pub const DEFAULT_HEIGHT: f64 = 660.0;

/// Content minimum matching `window.contentMinSize = 720×520`.
pub const CONTENT_MIN_WIDTH: f64 = 720.0;
pub const CONTENT_MIN_HEIGHT: f64 = 520.0;

/// Persisted frame. `x`/`y` may be absent on first launch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Frame {
    #[serde(default)]
    pub x: Option<f64>,
    #[serde(default)]
    pub y: Option<f64>,
    pub width: f64,
    pub height: f64,
}

impl Default for Frame {
    fn default() -> Self {
        Self { x: None, y: None, width: DEFAULT_WIDTH, height: DEFAULT_HEIGHT }
    }
}

impl Frame {
    pub fn new(width: f64, height: f64) -> Self {
        Self { x: None, y: None, width, height }
    }

    /// Clamp the frame up to the content minimum, matching Swift's
    /// post-restore fixup. A frame smaller than the minimum breaks the
    /// split-view layout.
    pub fn clamped(self) -> Self {
        Self {
            x: self.x,
            y: self.y,
            width: self.width.max(CONTENT_MIN_WIDTH),
            height: self.height.max(CONTENT_MIN_HEIGHT),
        }
    }

    /// Whether the frame needs clamping.
    pub fn needs_clamp(&self) -> bool {
        self.width < CONTENT_MIN_WIDTH || self.height < CONTENT_MIN_HEIGHT
    }
}

/// Resolve the file that holds the persisted frame.
pub fn default_file_path() -> PathBuf {
    pluk_core::platform::app_config_dir().join("window-frame.json")
}

/// Load the persisted frame, or the default when absent / corrupt.
pub fn load(path: &PathBuf) -> Frame {
    if let Ok(data) = std::fs::read_to_string(path) {
        if let Ok(frame) = serde_json::from_str::<Frame>(&data) {
            return frame.clamped();
        }
    }
    Frame::default()
}

/// Save the frame.
pub fn save(path: &PathBuf, frame: &Frame) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let data = serde_json::to_string_pretty(frame).expect("frame serializes");
    std::fs::write(path, data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_frame_is_1040x660() {
        let f = Frame::default();
        assert_eq!(f.width, 1040.0);
        assert_eq!(f.height, 660.0);
        assert!(!f.needs_clamp());
    }

    #[test]
    fn clamp_raises_small_frame_to_content_minimum() {
        let small = Frame { x: Some(0.0), y: Some(0.0), width: 400.0, height: 300.0 };
        assert!(small.needs_clamp());
        let clamped = small.clamped();
        assert_eq!(clamped.width, CONTENT_MIN_WIDTH);
        assert_eq!(clamped.height, CONTENT_MIN_HEIGHT);
        // Larger already is untouched.
        let big = Frame::new(900.0, 800.0).clamped();
        assert_eq!(big.width, 900.0);
        assert_eq!(big.height, 800.0);
    }

    #[test]
    fn only_one_dimension_small_still_clamps_that_dimension() {
        let f = Frame::new(800.0, 400.0).clamped();
        assert_eq!(f.width, 800.0);
        assert_eq!(f.height, CONTENT_MIN_HEIGHT);
    }

    #[test]
    fn round_trips_through_file_and_clamps_corrupt_restores() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("frame.json");
        let frame = Frame { x: Some(10.0), y: Some(20.0), width: 500.0, height: 500.0 };
        save(&path, &frame).unwrap();
        let loaded = load(&path);
        // Saved small frame is clamped on load.
        assert_eq!(loaded.width, CONTENT_MIN_WIDTH);
        assert_eq!(loaded.height, CONTENT_MIN_HEIGHT);
        // Valid frame round-trips clamped value.
        let big = Frame { x: Some(5.0), y: Some(5.0), width: 1200.0, height: 800.0 };
        save(&path, &big).unwrap();
        assert_eq!(load(&path), big);
    }

    #[test]
    fn missing_file_returns_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing.json");
        assert_eq!(load(&path), Frame::default());
    }

    #[test]
    fn corrupt_json_returns_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("frame.json");
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(load(&path), Frame::default());
    }

    #[test]
    fn serialization_is_stable() {
        let f = Frame { x: Some(1.0), y: Some(2.0), width: 1040.0, height: 660.0 };
        let json = serde_json::to_string(&f).unwrap();
        let back: Frame = serde_json::from_str(&json).unwrap();
        assert_eq!(f, back);
    }
}
