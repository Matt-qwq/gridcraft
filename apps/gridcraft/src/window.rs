//! The window's restored size, kept in `window.json` next to `ui.json`.
//!
//! GridCraft always starts **maximized**. The only thing remembered is the inner size the window
//! has when restored, so the restore button returns to the size the user last chose instead of the
//! built-in 1440×900. The maximized inner rect is the monitor's, not the user's choice, so it is
//! never recorded; a saved window *position* is left out too, because it can land off-screen when
//! a monitor is unplugged.
//!
//! A missing, unreadable or hand-edited file never stops a start: it reads as the built-in size.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// The range of inner sizes a remembered window may have, in points. A corrupt file cannot ask for
/// a window too small to show the grid, or larger than any monitor.
const MIN_SIZE: f32 = 320.0;
const MAX_SIZE: f32 = 16_384.0;

/// What `window.json` holds.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WindowState {
    /// The inner size in points while the window was restored (not maximized).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<f32>,
}

impl WindowState {
    /// The file name inside the settings directory.
    pub const FILE: &'static str = "window.json";

    /// Read the state kept in `dir`; a missing, unreadable or malformed file is the default state.
    pub fn load(dir: Option<&Path>) -> Self {
        let Some(dir) = dir else { return Self::default() };
        let bytes = std::fs::read(dir.join(Self::FILE)).ok();
        bytes.and_then(|b| Self::parse(&b)).unwrap_or_default()
    }

    /// Parse `window.json`'s bytes; `None` when they are not the shape [`Self::save`] writes.
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        serde_json::from_slice(bytes).ok()
    }

    /// Write the state into `dir`, creating it first. Callers treat this as best-effort: a window
    /// that cannot save its size still works.
    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let bytes = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(dir.join(Self::FILE), bytes)
    }

    /// Record the size the window reports in a frame.
    ///
    /// Taken only while the window is restored: maximized, the inner rect is the monitor's, and
    /// remembering it would make the restore button return to screen size.
    pub fn observe(&mut self, maximized: Option<bool>, inner_size: Option<(f32, f32)>) {
        if maximized == Some(false)
            && let Some((width, height)) = inner_size
            && plausible(width, height)
        {
            self.width = Some(width);
            self.height = Some(height);
        }
    }

    /// Apply the remembered size to the window the app is about to create.
    ///
    /// The window is maximized one frame later, by a viewport command from the app (see
    /// `App::start_maximized` in `main.rs`), not here: on Windows a `with_maximized(true)` set at
    /// creation races egui's own `request_inner_size`, which lands afterwards and leaves the
    /// window with the maximize flag but the restored rectangle.
    pub fn apply(&self, viewport: egui::ViewportBuilder) -> egui::ViewportBuilder {
        match (self.width, self.height) {
            (Some(width), Some(height)) if plausible(width, height) => viewport.with_inner_size([width, height]),
            _ => viewport,
        }
    }
}

/// Whether a saved inner size is a window the app can show. `NaN` fails the range check too.
fn plausible(width: f32, height: f32) -> bool {
    (MIN_SIZE..=MAX_SIZE).contains(&width) && (MIN_SIZE..=MAX_SIZE).contains(&height)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// This test's own settings directory, so tests running in parallel never share a file.
    fn dir(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("gridcraft-window-{}-{tag}", std::process::id()))
    }

    /// The window the app creates when nothing was saved.
    fn default_viewport() -> egui::ViewportBuilder {
        egui::ViewportBuilder::default().with_inner_size([1440.0, 900.0])
    }

    /// The default: every start is maximized, saved state or not. The size is the one the window
    /// is created at (and where a restore returns to); the maximize itself is the app's first
    /// viewport command, because setting it at creation races egui's own size request on Windows.
    #[test]
    fn the_window_keeps_the_restored_size_for_a_maximized_start() {
        assert_eq!(WindowState::default().apply(default_viewport()).inner_size, Some(egui::vec2(1440.0, 900.0)));
        let dir = dir("maximized");
        let mut state = WindowState::default();
        state.observe(Some(false), Some((1200.0, 800.0)));
        assert!(state.save(&dir).is_ok());
        let back = WindowState::load(Some(&dir));
        // Restoring returns to the remembered size, not the built-in one.
        assert_eq!(back.apply(default_viewport()).inner_size, Some(egui::vec2(1200.0, 800.0)));
        assert_eq!(back.apply(default_viewport()).maximized, None, "the maximize is sent after the window exists");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The size is the restored one, and maximizing again does not overwrite it.
    #[test]
    fn the_restored_size_survives_maximizing() {
        let mut state = WindowState::default();
        state.observe(Some(false), Some((1200.0, 800.0)));
        state.observe(Some(true), Some((2560.0, 1440.0)));
        assert_eq!(state.apply(default_viewport()).inner_size, Some(egui::vec2(1200.0, 800.0)));
    }

    /// Without a reported state the saved size is kept as it is.
    #[test]
    fn an_unknown_state_keeps_the_saved_size() {
        let mut state = WindowState::default();
        state.observe(Some(false), Some((1200.0, 800.0)));
        state.observe(None, None);
        assert_eq!(state.width, Some(1200.0));
        state.observe(None, Some((1000.0, 700.0)));
        assert_eq!(state.width, Some(1200.0));
    }

    /// A missing or corrupt file never stops a start: the built-in size is used, maximized.
    #[test]
    fn a_missing_or_corrupt_file_is_ignored() {
        let nowhere = dir("missing");
        let _ = std::fs::remove_dir_all(&nowhere);
        assert_eq!(WindowState::load(Some(&nowhere)), WindowState::default());
        assert_eq!(WindowState::load(None), WindowState::default());
        assert_eq!(WindowState::parse(b"{\"width\": "), None);
        assert_eq!(WindowState::parse(b"not json"), None);
        assert_eq!(WindowState::parse(b"{\"width\": \"wide\"}"), None);
        // A file from the build that also saved `maximized` still reads: the extra key is ignored.
        let older = WindowState::parse(b"{\"maximized\": false, \"width\": 1200.0, \"height\": 800.0}");
        assert!(older.is_some_and(|s| s.width == Some(1200.0) && s.height == Some(800.0)));
        // A file written when there was no size keeps the default one.
        assert_eq!(WindowState::parse(b"{\"maximized\": true}"), Some(WindowState::default()));
    }

    /// A hand-edited size no monitor could show is ignored, not obeyed.
    #[test]
    fn an_implausible_size_is_ignored() {
        let huge = WindowState { width: Some(f32::INFINITY), height: Some(900.0) };
        assert_eq!(huge.apply(default_viewport()).inner_size, Some(egui::vec2(1440.0, 900.0)));
        let tiny = WindowState { width: Some(10.0), height: Some(900.0) };
        assert_eq!(tiny.apply(default_viewport()).inner_size, Some(egui::vec2(1440.0, 900.0)));
        let nan = WindowState { width: Some(f32::NAN), height: Some(f32::NAN) };
        assert_eq!(nan.apply(default_viewport()).inner_size, Some(egui::vec2(1440.0, 900.0)));
    }
}
