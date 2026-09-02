//! Zoom state — nine typography-only scale steps.
//!
//! The frontend applies the factor as a CSS variable / root font-size.
//! The host owns the value, the persistence, and the menu enablement.

use std::path::PathBuf;

/// Ordered scale factors, low to high.
pub const STEPS: [f64; 9] = [0.85, 0.90, 1.00, 1.10, 1.25, 1.40, 1.60, 1.80, 2.00];
const DEFAULT_INDEX: usize = 2;
const SETTINGS_KEY: &str = "ui_zoom_step";

/// Pure in-memory zoom state. Persistence is a separate concern so this
/// is fully testable without a display or file system.
#[derive(Debug, Clone, PartialEq)]
pub struct ZoomState {
    index: usize,
}

impl Default for ZoomState {
    fn default() -> Self {
        Self {
            index: DEFAULT_INDEX,
        }
    }
}

impl ZoomState {
    pub fn new(index: usize) -> Self {
        Self {
            index: index.clamp(0, STEPS.len() - 1),
        }
    }

    /// Current scale factor the frontend should apply.
    pub fn scale(&self) -> f64 {
        STEPS[self.index]
    }

    pub fn index(&self) -> usize {
        self.index
    }

    pub fn is_default(&self) -> bool {
        self.index == DEFAULT_INDEX
    }

    pub fn can_zoom_in(&self) -> bool {
        self.index < STEPS.len() - 1
    }

    pub fn can_zoom_out(&self) -> bool {
        self.index > 0
    }

    pub fn label(&self) -> String {
        format!("{}%", (self.scale() * 100.0).round() as i64)
    }

    /// "Actual Size" or "Actual Size (125%)".
    pub fn reset_title(&self) -> String {
        if self.is_default() {
            "Actual Size".to_string()
        } else {
            format!("Actual Size ({})", self.label())
        }
    }

    pub fn zoom_in(&mut self) {
        if self.can_zoom_in() {
            self.index += 1;
        }
    }

    pub fn zoom_out(&mut self) {
        if self.can_zoom_out() {
            self.index -= 1;
        }
    }

    pub fn reset(&mut self) {
        self.index = DEFAULT_INDEX;
    }
}


/// Persisted zoom backed by either `pluk_store::Store` (`settings` table)
/// or a plain file at `app_config_dir/zoom.json`. The file path is
/// preferred for isolation in tests; production code passes `None` for
/// `file_fallback` and uses the Store.
pub struct PersistedZoom {
    state: ZoomState,
    /// When `Some`, used for persistence. When `None`, caller provides a Store.
    file_path: Option<PathBuf>,
}

impl PersistedZoom {
    pub fn from_index(index: usize, file_path: Option<PathBuf>) -> Self {
        Self {
            state: ZoomState::new(index),
            file_path,
        }
    }

    pub fn state(&self) -> &ZoomState {
        &self.state
    }

    pub fn state_mut(&mut self) -> &mut ZoomState {
        &mut self.state
    }

    /// Load from `file_path` when given, otherwise return default.
    /// Callers that prefer Store should use `load_from_store`.
    pub fn load(file_path: Option<PathBuf>) -> Self {
        if let Some(ref path) = file_path
            && let Ok(data) = std::fs::read_to_string(path)
        {
            if let Ok(v) = data.trim().parse::<usize>()
                && v < STEPS.len()
            {
                return Self {
                    state: ZoomState::new(v),
                    file_path,
                };
            }
            // Also try JSON { "index": n }
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&data)
                && let Some(n) = json.get("index").and_then(|v| v.as_u64())
            {
                let idx = n as usize;
                if idx < STEPS.len() {
                    return Self {
                        state: ZoomState::new(idx),
                        file_path,
                    };
                }
            }
        }
        Self {
            state: ZoomState::default(),
            file_path,
        }
    }

    pub fn load_from_store(store: &pluk_store::Store) -> Self {
        let index = store
            .get_setting(SETTINGS_KEY)
            .ok()
            .flatten()
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|&i| i < STEPS.len())
            .unwrap_or(DEFAULT_INDEX);
        Self {
            state: ZoomState::new(index),
            file_path: None,
        }
    }

    pub fn save(&self, store: Option<&pluk_store::Store>) -> std::io::Result<()> {
        if let Some(store) = store {
            let _ = store.set_setting(SETTINGS_KEY, &self.state.index.to_string());
            return Ok(());
        }
        if let Some(ref path) = self.file_path {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(path, self.state.index.to_string())?;
        }
        Ok(())
    }
}

/// Resolve the default file path for zoom persistence.
pub fn default_file_path() -> PathBuf {
    pluk_core::platform::app_config_dir().join("zoom.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nine_steps_from_085_to_200_default_is_100() {
        assert_eq!(STEPS.len(), 9);
        assert!((STEPS[0] - 0.85).abs() < f64::EPSILON);
        assert!((STEPS[STEPS.len() - 1] - 2.0).abs() < f64::EPSILON);
        assert_eq!(ZoomState::default().scale(), 1.0);
        assert_eq!(ZoomState::default().index(), 2);
    }

    #[test]
    fn zoom_in_out_clamps_at_ends() {
        let mut z = ZoomState::new(0);
        assert!(!z.can_zoom_out());
        assert!(z.can_zoom_in());
        z.zoom_out();
        assert_eq!(z.index(), 0);
        z.zoom_in();
        assert_eq!(z.index(), 1);

        let mut z = ZoomState::new(STEPS.len() - 1);
        assert!(!z.can_zoom_in());
        z.zoom_in();
        assert_eq!(z.index(), STEPS.len() - 1);
        z.zoom_out();
        assert_eq!(z.index(), STEPS.len() - 2);
    }

    #[test]
    fn reset_returns_to_default_and_label() {
        let mut z = ZoomState::new(5);
        assert!(!z.is_default());
        assert_eq!(z.label(), "140%");
        assert_eq!(z.reset_title(), "Actual Size (140%)");
        z.reset();
        assert!(z.is_default());
        assert_eq!(z.reset_title(), "Actual Size");
    }

    #[test]
    fn new_clamps_out_of_range() {
        assert_eq!(ZoomState::new(999).index(), STEPS.len() - 1);
        assert_eq!(ZoomState::new(0).scale(), 0.85);
    }

    #[test]
    fn persisted_zoom_round_trips_via_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("zoom.json");
        let mut pz = PersistedZoom::load(Some(path.clone()));
        assert!(pz.state().is_default());
        pz.state_mut().zoom_in();
        pz.state_mut().zoom_in();
        pz.save(None).unwrap();
        let reloaded = PersistedZoom::load(Some(path));
        assert_eq!(reloaded.state().index(), DEFAULT_INDEX + 2);
    }

    #[test]
    fn persisted_zoom_via_store() {
        let dir = tempfile::tempdir().unwrap();
        let store = pluk_store::Store::open(&dir.path().join("pluk.db")).unwrap();
        let mut pz = PersistedZoom::load_from_store(&store);
        assert!(pz.state().is_default());
        pz.state_mut().zoom_out();
        pz.save(Some(&store)).unwrap();
        let reloaded = PersistedZoom::load_from_store(&store);
        assert_eq!(reloaded.state().index(), DEFAULT_INDEX - 1);
    }

    #[test]
    fn every_step_is_distinct_and_sorted() {
        for w in STEPS.windows(2) {
            assert!(w[0] < w[1]);
        }
    }
}
