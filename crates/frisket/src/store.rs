//! Pane-layout sidecar: the pane split tree ([`crate::FrisketLayout`])
//! persisted beside `graph.json`, so a window's pane arrangement (which panes
//! are open, their split ratios) survives a restart.
//!
//! ```text
//! <session_dir>/
//! ├── graph.json            ← session_graph_store
//! ├── settings.json         ← settings_store
//! └── frame.json            ← this module (the on-disk tag is still `frame`)
//! ```
//!
//! Moved here from `session_runtime::frisket_store` at meerkat's deletion
//! (2026-07-18) — the pane-coupled half of session-runtime, relocated with
//! the pane model exactly as the boundary-pass plan parked it. v0 stores the
//! single content layout as one file; per-window files arrive with
//! multi-window (the `FrisketId` would key a subdirectory the way `views/`
//! keys view-intent).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::FrisketLayout;

/// Filename for the pane layout's sidecar (beside `graph.json`).
///
/// The on-disk tag is still `frame`, deliberately. Renaming the file is a
/// format migration and it travels with the other one this rename left in
/// place, `PaneContent::Orrery` (a serde variant name, and so also on-disk).
/// Both are parked as a single vocabulary decision rather than two silent
/// breaks.
pub const FRAME_FILE: &str = "frame.json";

/// The frame sidecar path under `session_dir`.
pub fn frame_layout_path(session_dir: &Path) -> PathBuf {
    session_dir.join(FRAME_FILE)
}

/// Serialize `layout` to JSON and write it atomically (tmp + rename).
pub fn save_frisket_layout(session_dir: &Path, layout: &FrisketLayout) -> io::Result<()> {
    let target = frame_layout_path(session_dir);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(layout)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let tmp = target.with_extension("json.tmp");
    fs::write(&tmp, json)?;
    fs::rename(&tmp, &target)?;
    Ok(())
}

/// Read + parse the frame sidecar. `Ok(None)` when it doesn't exist (fresh
/// session — the host falls back to its default single-pane layout).
pub fn load_frisket_layout(session_dir: &Path) -> io::Result<Option<FrisketLayout>> {
    let path = frame_layout_path(session_dir);
    if !path.exists() {
        return Ok(None);
    }
    let text = fs::read_to_string(&path)?;
    let layout: FrisketLayout =
        serde_json::from_str(&text).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    Ok(Some(layout))
}

#[cfg(test)]
mod tests {
    use crate::{FrisketId, GraphId, InsertSide, PaneContent, PaneId, PaneNode};

    use super::*;

    fn temp_session_dir(label: &str) -> PathBuf {
        let pid = std::process::id();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir =
            std::env::temp_dir().join(format!("merecat-frame-store-test-{label}-{pid}-{nanos}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample_layout() -> FrisketLayout {
        let mut layout = FrisketLayout {
            id: FrisketId::new("content"),
            label: "content".into(),
            root: PaneNode::Leaf {
                pane_id: PaneId(0),
                content: PaneContent::Workbench,
                graph_id: GraphId::from_uuid(uuid::Uuid::from_u128(1)),
            },
        };
        layout.summon_leaf(
            &[],
            InsertSide::Right,
            PaneNode::Leaf {
                pane_id: PaneId(1),
                content: PaneContent::Roster,
                graph_id: GraphId::from_uuid(uuid::Uuid::from_u128(1)),
            },
        );
        layout.set_split_ratio(&[], 0.66);
        layout
    }

    #[test]
    fn save_then_load_round_trips_layout() {
        let dir = temp_session_dir("round-trip");
        let original = sample_layout();
        save_frisket_layout(&dir, &original).unwrap();
        let restored = load_frisket_layout(&dir)
            .unwrap()
            .expect("frame file present");
        assert_eq!(restored, original);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_returns_none_when_no_file() {
        let dir = temp_session_dir("no-file");
        assert!(load_frisket_layout(&dir).unwrap().is_none());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn malformed_json_is_invalid_data() {
        let dir = temp_session_dir("malformed");
        fs::write(frame_layout_path(&dir), "{ not json").unwrap();
        match load_frisket_layout(&dir) {
            Err(e) => assert_eq!(e.kind(), io::ErrorKind::InvalidData),
            Ok(_) => panic!("expected malformed JSON to fail"),
        }
        fs::remove_dir_all(&dir).ok();
    }
}
