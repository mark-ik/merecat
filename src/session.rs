//! The persistence port: where merecat's data lives and how the session
//! graph saves/loads. Flat single-session `graph.json` today; multi-session
//! (`sessions/<id>/` + manifests), the browser-state sidecar, view intent,
//! and settings join at obviation rung 6.

use std::path::{Path, PathBuf};

use frisket::FrisketLayout;
use mere::kernel::graph::Graph;
use session_runtime::{frisket_store, session_graph_store};

/// The per-user data root (`<data_dir>/merecat`). A `MERECAT_ROOT` override
/// points the whole root at a scratch profile, so a headed-verification run
/// (or any throwaway session) isolates from the real per-user data dir (the
/// meerkat `MERE_ROOT` convention).
pub fn default_merecat_root() -> PathBuf {
    if let Some(root) = std::env::var_os("MERECAT_ROOT") {
        return PathBuf::from(root);
    }
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("merecat")
}

/// Restore the persisted session graph, if one exists. Logs and returns
/// `None` on a load failure (the host starts fresh rather than dying on a
/// corrupt file).
pub fn load_session_graph(data_root: &Path) -> Option<Graph> {
    let graph_file = data_root.join(session_graph_store::GRAPH_FILE);
    match session_graph_store::load(&graph_file) {
        Ok(Some(graph)) => {
            tracing::info!(path = ?graph_file, "session graph restored");
            Some(graph)
        }
        Ok(None) => None,
        Err(err) => {
            tracing::warn!(%err, path = ?graph_file, "failed to load the session graph; starting fresh");
            None
        }
    }
}

/// Persist the session graph at the flat `graph.json`. Best-effort: a write
/// failure is logged, not fatal. Run after each enrichment (so a crash loses
/// nothing) and on close.
pub fn save_session_graph(data_root: &Path, graph: &Graph) {
    let graph_file = data_root.join(session_graph_store::GRAPH_FILE);
    if let Err(err) = session_graph_store::save(&graph_file, graph) {
        tracing::warn!(%err, path = ?graph_file, "failed to persist the session graph");
    }
}

/// Restore the persisted pane layout (rung 5 slice C), if one exists. The sidecar
/// is `frame.json` beside `graph.json` (the on-disk tag stays `frame`, a parked
/// format decision). `None` starts on the default single-pane layout.
pub fn load_frisket_layout(data_root: &Path) -> Option<FrisketLayout> {
    match frisket_store::load_frisket_layout(data_root) {
        Ok(layout) => layout,
        Err(err) => {
            tracing::warn!(%err, "failed to load the pane layout; starting on the default");
            None
        }
    }
}

/// Persist the pane layout at `frame.json`. Best-effort, like the graph.
pub fn save_frisket_layout(data_root: &Path, layout: &FrisketLayout) {
    if let Err(err) = frisket_store::save_frisket_layout(data_root, layout) {
        tracing::warn!(%err, "failed to persist the pane layout");
    }
}
