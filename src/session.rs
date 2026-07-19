//! The persistence port: where merecat's data lives and how sessions
//! save/load. Multi-session since rung 6's second half: each session owns
//! `sessions/<id>/` (graph.json, frame.json, workbench.json,
//! browser_nodes.json, windows.json, manifest.json); the manifest set is
//! session-runtime's `ManifestStore`, and the flat single-session layout
//! this port started on migrates in on first boot.

use std::path::{Path, PathBuf};

use frisket::{FrisketLayout, SessionId};
// The frame-sidecar store is frisket's own since meerkat's deletion (it moved
// out of session-runtime with the pane model).
use frisket::store as frisket_store;
use mere::kernel::graph::Graph;
use session_runtime::{GraphSessionManifest, ManifestStore, session_graph_store};

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

/// The sessions directory under the data root: one subdirectory per session,
/// named by its uuid (ManifestStore's own layout).
pub fn sessions_root(data_root: &Path) -> PathBuf {
    data_root.join("sessions")
}

/// One session's directory: where ALL its sidecars live.
pub fn session_dir(data_root: &Path, id: SessionId) -> PathBuf {
    sessions_root(data_root).join(id.as_uuid().to_string())
}

/// The current-session marker (`<root>/current_session`, the bare uuid): the
/// session a restart reopens. Best-effort, like every sidecar.
const CURRENT_SESSION_FILE: &str = "current_session";

pub fn record_current_session(data_root: &Path, id: SessionId) {
    let path = data_root.join(CURRENT_SESSION_FILE);
    if let Err(err) = std::fs::write(&path, id.as_uuid().to_string()) {
        tracing::warn!(%err, "failed to record the current session");
    }
}

/// The session a boot should open: the recorded current session when it
/// still exists, else the most recently updated manifest, else `None` (a
/// fresh install — the caller mints one).
pub fn pick_session(data_root: &Path, store: &ManifestStore) -> Option<SessionId> {
    let recorded = std::fs::read_to_string(data_root.join(CURRENT_SESSION_FILE))
        .ok()
        .and_then(|s| s.trim().parse::<uuid::Uuid>().ok())
        .map(SessionId::from_uuid)
        .filter(|id| store.get(*id).is_some());
    recorded.or_else(|| {
        store
            .iter()
            .max_by_key(|(_, m)| m.updated_at)
            .map(|(id, _)| id)
    })
}

/// Load the manifest set from `sessions/`. Failures are logged per directory
/// (the store's own report), never fatal.
pub fn load_manifests(data_root: &Path) -> ManifestStore {
    let mut store = ManifestStore::new();
    match store.load_from_disk(sessions_root(data_root)) {
        Ok(report) => {
            for failure in &report.failed {
                tracing::warn!(
                    dir = %failure.dir_name,
                    reason = %failure.reason,
                    "a session manifest failed to load"
                );
            }
        }
        Err(err) => tracing::warn!(%err, "failed to read the sessions directory"),
    }
    store
}

/// The sidecar files a session owns (the flat layout's file set, and each
/// session directory's).
const SESSION_FILES: [&str; 5] = [
    session_graph_store::GRAPH_FILE,
    frisket_store::FRAME_FILE,
    WORKBENCH_FILE,
    session_runtime::browser_node_state::BROWSER_NODES_FILE,
    frisket_store::WINDOWS_FILE,
];

/// One-time migration from the flat single-session layout: when a flat
/// `graph.json` sits at the root and no session holds anything yet, mint a
/// session, MOVE the flat sidecars into its directory, and write its
/// manifest. Returns the minted id when a migration ran. Best-effort per
/// file (a copy that fails logs and stays put — the graph file moving is
/// what the migration is judged by).
pub fn migrate_flat_layout(data_root: &Path, store: &mut ManifestStore) -> Option<SessionId> {
    if !store.is_empty() {
        return None;
    }
    let flat_graph = data_root.join(session_graph_store::GRAPH_FILE);
    if !flat_graph.exists() {
        return None;
    }
    let id = SessionId::new();
    let dir = session_dir(data_root, id);
    if let Err(err) = std::fs::create_dir_all(&dir) {
        tracing::warn!(%err, "flat-layout migration could not create the session dir");
        return None;
    }
    for file in SESSION_FILES {
        let from = data_root.join(file);
        if !from.exists() {
            continue;
        }
        if let Err(err) = std::fs::rename(&from, dir.join(file)) {
            tracing::warn!(%err, %file, "flat-layout migration failed to move a sidecar");
            if file == session_graph_store::GRAPH_FILE {
                return None;
            }
        }
    }
    let mut manifest = GraphSessionManifest::new(id, frisket::GraphId::nil());
    manifest.storage_path = Some(dir);
    store.insert(manifest);
    if let Err(err) = store.flush_dirty() {
        tracing::warn!(%err, "flat-layout migration failed to write the manifest");
    }
    tracing::info!(session = %id.as_uuid(), "flat single-session layout migrated to sessions/");
    Some(id)
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

/// Persist the lens-window spaces at `windows.json` (rung 7 depth: windows
/// are pane hosts, so torn-out panes survive a restart AS windows). Closed
/// slots persist as `null`, keeping ordinals stable. Best-effort, like the
/// rest.
pub fn save_lens_spaces(data_root: &Path, lenses: &[Option<FrisketLayout>]) {
    if let Err(err) = frisket_store::save_lens_spaces(data_root, lenses) {
        tracing::warn!(%err, "failed to persist the lens windows");
    }
}

/// Restore the lens-window spaces. Missing or corrupt starts with none (the
/// primary window alone — the panes those windows held are gone with them,
/// honestly, not silently folded into the primary).
pub fn load_lens_spaces(data_root: &Path) -> Vec<Option<FrisketLayout>> {
    match frisket_store::load_lens_spaces(data_root) {
        Ok(Some(lenses)) => lenses,
        Ok(None) => Vec::new(),
        Err(err) => {
            tracing::warn!(%err, "failed to load the lens-window sidecar; starting with none");
            Vec::new()
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "merecat-session-test-{label}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The flat single-session layout migrates into `sessions/<id>/` exactly
    /// once: the sidecars MOVE (the flat graph is gone), the manifest is
    /// written, and `pick_session` finds the minted session. A second boot
    /// with a populated store migrates nothing.
    #[test]
    fn flat_layout_migrates_into_sessions_once() {
        let root = temp_root("migrate");
        std::fs::write(root.join(session_graph_store::GRAPH_FILE), b"{}").unwrap();
        std::fs::write(root.join(frisket_store::FRAME_FILE), b"{}").unwrap();

        let mut store = load_manifests(&root);
        let id = migrate_flat_layout(&root, &mut store).expect("the flat layout migrates");
        let dir = session_dir(&root, id);
        assert!(dir.join(session_graph_store::GRAPH_FILE).exists());
        assert!(dir.join(frisket_store::FRAME_FILE).exists());
        assert!(dir.join("manifest.json").exists());
        assert!(
            !root.join(session_graph_store::GRAPH_FILE).exists(),
            "the flat graph MOVED, not copied"
        );
        assert_eq!(store.len(), 1);
        assert_eq!(pick_session(&root, &store), Some(id));

        // Reload from disk (a second boot): nothing migrates again.
        let mut store2 = load_manifests(&root);
        assert_eq!(store2.len(), 1);
        assert_eq!(migrate_flat_layout(&root, &mut store2), None);
    }

    /// The current-session marker round-trips, and a stale marker (a session
    /// that no longer exists) falls back to the most recent manifest.
    #[test]
    fn current_session_marker_round_trips_and_falls_back() {
        let root = temp_root("current");
        let mut store = ManifestStore::with_root(sessions_root(&root));
        let a = SessionId::new();
        let b = SessionId::new();
        let mut ma = GraphSessionManifest::new(a, frisket::GraphId::nil());
        // `b` is newer (created after), so the fallback picks it.
        std::thread::sleep(std::time::Duration::from_millis(10));
        let mb = GraphSessionManifest::new(b, frisket::GraphId::nil());
        ma.touch();
        store.insert(ma);
        store.insert(mb);

        record_current_session(&root, a);
        assert_eq!(pick_session(&root, &store), Some(a));
        record_current_session(&root, SessionId::new());
        // The recorded id is unknown; the newest manifest wins. `a` was
        // touched later than `b` was created, so updated_at prefers `a`.
        assert_eq!(pick_session(&root, &store), Some(a));
    }
}
