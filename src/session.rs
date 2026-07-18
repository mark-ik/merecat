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

/// The workbench tiling sidecar, beside `graph.json` (the meerkat convention:
/// the tiling is the graph's, so it persists with the session).
const WORKBENCH_FILE: &str = "workbench.json";

/// Persist the workbench tiling as platen's canonical `(Arrangement, geometry)`
/// pair (the live `Pane` tree is a derived cache, never serde;
/// `to_persisted_json` debug-asserts `canonical_roundtrips` — platen's
/// persistence discipline, preserved verbatim). Best-effort, like the rest.
pub fn save_workbench(data_root: &Path, workbench: &mere::platen::Workbench) {
    match workbench.to_persisted_json() {
        Ok(json) => {
            let path = data_root.join(WORKBENCH_FILE);
            if let Err(err) = std::fs::write(&path, json) {
                tracing::warn!(%err, path = ?path, "failed to persist the workbench tiling");
            }
        }
        Err(err) => tracing::warn!(%err, "failed to serialize the workbench tiling"),
    }
}

/// Persist the browser-state sidecar (`browser_nodes.json` beside
/// `graph.json`): per-node browser handling — viewer override, compat mode,
/// and whether live content was ON, so a restart respawns it (rung 6).
/// Best-effort, like the rest.
pub fn save_browser_nodes(
    data_root: &Path,
    states: &session_runtime::browser_node_state::BrowserNodeStates,
) {
    if let Err(err) =
        session_runtime::browser_node_state::save_browser_node_states(data_root, states)
    {
        tracing::warn!(%err, "failed to persist the browser-state sidecar");
    }
}

/// Restore the browser-state sidecar. A missing or corrupt sidecar starts
/// empty (the graph stays correct without it, by the sidecar's own charter).
pub fn load_browser_nodes(
    data_root: &Path,
) -> session_runtime::browser_node_state::BrowserNodeStates {
    match session_runtime::browser_node_state::load_browser_node_states(data_root) {
        Ok(Some(states)) => states,
        Ok(None) => session_runtime::browser_node_state::BrowserNodeStates::new(),
        Err(err) => {
            tracing::warn!(%err, "failed to load the browser-state sidecar; starting empty");
            session_runtime::browser_node_state::BrowserNodeStates::new()
        }
    }
}

/// Restore the workbench tiling, pruned to `present` (the live graph's
/// members, so a tile whose node vanished between sessions collapses away).
/// A missing or corrupt sidecar starts on an empty workbench.
pub fn load_workbench(
    data_root: &Path,
    present: &std::collections::HashSet<uuid::Uuid>,
) -> mere::platen::Workbench {
    let path = data_root.join(WORKBENCH_FILE);
    let Ok(json) = std::fs::read_to_string(&path) else {
        return mere::platen::Workbench::new();
    };
    match mere::platen::Workbench::from_persisted_json(&json, present) {
        Some(wb) => {
            tracing::info!(path = ?path, tiles = wb.tile_count(), "workbench tiling restored");
            wb
        }
        None => {
            tracing::warn!(path = ?path, "failed to parse the workbench sidecar; starting empty");
            mere::platen::Workbench::new()
        }
    }
}
