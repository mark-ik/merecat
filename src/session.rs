//! The persistence port: where turnstone's data lives and how sessions
//! save/load. Multi-session since rung 6's second half: each session owns
//! `sessions/<id>/` (graph.json, frame.json, workbench.json,
//! browser_nodes.json, windows.json, manifest.json); the manifest set is
//! session-runtime's `ManifestStore`, and the flat single-session layout
//! this port started on migrates in on first boot.

use std::path::{Path, PathBuf};

use crate::panes::{FrisketLayout, SessionId};
use crate::place::{PlaceBindingError, PlaceBindingV1};
use image::ImageEncoder;
// The frame-sidecar store is frisket's own since meerkat's deletion (it moved
// out of session-runtime with the pane model).
use crate::panes::store as frisket_store;
use mere::kernel::graph::Graph;
use sceno::Score;
use session_runtime::{GraphSessionManifest, ManifestStore, session_graph_store};

/// The per-user data root (`<data_dir>/turnstone`). A `TURNSTONE_ROOT` override
/// points the whole root at a scratch profile, so a headed-verification run
/// (or any throwaway session) isolates from the real per-user data dir (the
/// meerkat `MERE_ROOT` convention).
pub fn default_turnstone_root() -> PathBuf {
    if let Some(root) = std::env::var_os("TURNSTONE_ROOT") {
        return PathBuf::from(root);
    }
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("turnstone")
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
const PROJECTION_SCORE_FILE: &str = "projection-score.json";
pub const PLACE_FILE: &str = "place.json";

const SESSION_FILES: [&str; 7] = [
    session_graph_store::GRAPH_FILE,
    frisket_store::FRAME_FILE,
    WORKBENCH_FILE,
    session_runtime::browser_node_state::BROWSER_NODES_FILE,
    frisket_store::WINDOWS_FILE,
    PROJECTION_SCORE_FILE,
    PLACE_FILE,
];

/// A strict `place.json` read or write failure. Unlike optional view sidecars,
/// an invalid binding must remain visible because silently treating it as a
/// personal session would change the session's authority model.
#[derive(Debug)]
pub enum PlaceSidecarError {
    Io(std::io::Error),
    Json(serde_json::Error),
    Binding(PlaceBindingError),
}

impl std::fmt::Display for PlaceSidecarError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "place sidecar I/O: {error}"),
            Self::Json(error) => write!(formatter, "place sidecar JSON: {error}"),
            Self::Binding(error) => write!(formatter, "place sidecar binding: {error}"),
        }
    }
}

impl std::error::Error for PlaceSidecarError {}

impl From<std::io::Error> for PlaceSidecarError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for PlaceSidecarError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl From<PlaceBindingError> for PlaceSidecarError {
    fn from(error: PlaceBindingError) -> Self {
        Self::Binding(error)
    }
}

pub fn place_binding_path(session_dir: &Path) -> PathBuf {
    session_dir.join(PLACE_FILE)
}

/// Persist the public binding through an adjacent temporary file. Group
/// secrets, welcome material, and live rendezvous state have no representation
/// in this sidecar.
pub fn save_place_binding(
    session_dir: &Path,
    binding: &PlaceBindingV1,
) -> Result<(), PlaceSidecarError> {
    binding.validate()?;
    std::fs::create_dir_all(session_dir)?;
    let target = place_binding_path(session_dir);
    let temporary = target.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(binding)?;
    if let Err(error) = (|| -> std::io::Result<()> {
        std::fs::write(&temporary, bytes)?;
        if target.exists() {
            std::fs::remove_file(&target)?;
        }
        std::fs::rename(&temporary, &target)
    })() {
        let _ = std::fs::remove_file(&temporary);
        return Err(error.into());
    }
    Ok(())
}

/// Load and validate this session's public shared-place binding. Absence means
/// a personal session; malformed or unsupported content is an explicit error.
pub fn load_place_binding(
    session_dir: &Path,
) -> Result<Option<PlaceBindingV1>, PlaceSidecarError> {
    let path = place_binding_path(session_dir);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let binding: PlaceBindingV1 = serde_json::from_slice(&bytes)?;
    binding.validate()?;
    Ok(Some(binding))
}

/// The persisted product-free score for this session's active analytic view.
pub fn projection_score_path(session_dir: &Path) -> PathBuf {
    session_dir.join(PROJECTION_SCORE_FILE)
}

/// Persist a score atomically. It is view state: a missing or malformed score
/// must never prevent the graph session itself from opening.
pub fn save_projection_score(session_dir: &Path, score: &Score) {
    let target = projection_score_path(session_dir);
    let tmp = target.with_extension("json.tmp");
    let result = (|| -> std::io::Result<()> {
        std::fs::create_dir_all(session_dir)?;
        let bytes = serde_json::to_vec_pretty(score).map_err(std::io::Error::other)?;
        std::fs::write(&tmp, bytes)?;
        if target.exists() {
            std::fs::remove_file(&target)?;
        }
        std::fs::rename(&tmp, &target)
    })();
    if let Err(err) = result {
        tracing::warn!(%err, path = ?target, "failed to persist projection score");
        let _ = std::fs::remove_file(tmp);
    }
}

/// Restore the last valid score. A corrupt sidecar is diagnosed and ignored;
/// the canvas will recompute a fresh score from current graph truth.
pub fn load_projection_score(session_dir: &Path) -> Option<Score> {
    let path = projection_score_path(session_dir);
    match std::fs::read(&path) {
        Ok(bytes) => match serde_json::from_slice(&bytes) {
            Ok(score) => Some(score),
            Err(err) => {
                tracing::warn!(%err, path = ?path, "failed to parse projection score");
                None
            }
        },
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
        Err(err) => {
            tracing::warn!(%err, path = ?path, "failed to read projection score");
            None
        }
    }
}

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
    let mut manifest = GraphSessionManifest::new(id, crate::panes::GraphId::nil());
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
    match session_graph_store::load_snapshot(&graph_file) {
        Ok(Some(mut snapshot)) => {
            let legacy_node_facets = snapshot.legacy_node_facet_count();
            // Externalize any pre-phase-2 inline imagery BEFORE materializing:
            // conversion keeps references only, so pixels left here would be
            // dropped silently. One-time; a snapshot already externalized has
            // nothing to do.
            let migrated = externalize_legacy_images(&mut snapshot, data_root);
            let mut graph = mere::kernel::graph::Graph::from_snapshot(&snapshot);
            if legacy_node_facets > 0 {
                // Existing facets are canonical and therefore win over the
                // one-time import from legacy graph columns. Persist the merged
                // store first, then strip those columns from graph.json.
                graph.overlay_facets(load_node_facets(data_root).unwrap_or_default());
                save_node_facets(data_root, graph.facets());
                save_session_graph(data_root, &graph);
                tracing::info!(
                    legacy_node_facets,
                    "migrated legacy node metadata into facets"
                );
            } else if migrated > 0 {
                // Re-save immediately so the pixels leave `graph.json` even if
                // the session never saves again.
                save_session_graph(data_root, &graph);
            }
            if migrated > 0 {
                tracing::info!(migrated, "externalized legacy inline node imagery");
            }
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

/// Move a legacy snapshot's inline image bytes into the session blob
/// directory, leaving references on the nodes. Returns how many blobs were
/// written.
///
/// The file-sidecar counterpart of `session_runtime::image_store::
/// migrate_legacy_images`, which needs an eidetic `Store` this host does not
/// have. Same digest and same `<hex>` key, so the two agree.
///
/// The legacy favicon is raw RGBA with no container; encode it once here so
/// every durable image blob has the same PNG format.
fn externalize_legacy_images(
    snapshot: &mut mere::kernel::persistence::GraphSnapshot,
    data_root: &Path,
) -> usize {
    use mere::kernel::types::{ImageRef, ImageRole};

    let mut written = 0usize;
    for node in &mut snapshot.nodes {
        if let Some(png) = node.legacy_thumbnail_png.take() {
            let digest = *eidetic::Hash::of(&png).as_bytes();
            let image = ImageRef::new(
                digest,
                node.legacy_thumbnail_width,
                node.legacy_thumbnail_height,
            );
            save_image_blob(data_root, &image.hex(), &png);
            node.images.insert(ImageRole::Preview, image);
            node.legacy_thumbnail_width = 0;
            node.legacy_thumbnail_height = 0;
            written += 1;
        }
        if let Some(rgba) = node.legacy_favicon_rgba.take() {
            let Some(png) =
                encode_rgba_png(&rgba, node.legacy_favicon_width, node.legacy_favicon_height)
            else {
                node.legacy_favicon_rgba = Some(rgba);
                continue;
            };
            let digest = *eidetic::Hash::of(&png).as_bytes();
            let image = ImageRef::new(
                digest,
                node.legacy_favicon_width,
                node.legacy_favicon_height,
            );
            save_image_blob(data_root, &image.hex(), &png);
            node.images.insert(ImageRole::Favicon, image);
            node.legacy_favicon_width = 0;
            node.legacy_favicon_height = 0;
            written += 1;
        }
    }
    debug_assert_eq!(
        snapshot.legacy_image_count(),
        0,
        "every legacy image must be externalized before the snapshot materializes"
    );
    written
}

/// Encode decoded straight-alpha RGBA8 pixels into the one durable image
/// format used by the sidecar store.
pub(crate) fn encode_rgba_png(rgba: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
    let expected = (width as usize)
        .checked_mul(height as usize)?
        .checked_mul(4)?;
    if width == 0 || height == 0 || rgba.len() != expected {
        return None;
    }
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(rgba, width, height, image::ExtendedColorType::Rgba8)
        .ok()?;
    Some(png)
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

/// The session's image-blob directory: `<session>/images/<hex>`.
///
/// After the node-image externalization the graph carries ~40-byte references
/// and the pixels live out of line. Turnstone's session persistence is
/// file-sidecar shaped (`graph.json`, `facets.json`), so its blob store is a
/// directory of the same shape rather than the eidetic-backed
/// `session_runtime::image_store` — same content-addressed `<hex>` key, so the
/// two converge cleanly if this session ever gains a real store.
fn images_dir(data_root: &Path) -> std::path::PathBuf {
    data_root.join("images")
}

/// Persist one image blob under its digest hex. Best-effort, like the graph:
/// a lost favicon re-fetches, so a write failure is logged, not fatal.
pub fn save_image_blob(data_root: &Path, hex: &str, bytes: &[u8]) {
    let dir = images_dir(data_root);
    if let Err(err) = std::fs::create_dir_all(&dir) {
        tracing::warn!(%err, path = ?dir, "failed to create the image directory");
        return;
    }
    let path = dir.join(hex);
    // Content-addressed: identical bytes are the same file, so an existing
    // blob needs no rewrite.
    if path.exists() {
        return;
    }
    if let Err(err) = std::fs::write(&path, bytes) {
        tracing::warn!(%err, path = ?path, "failed to persist an image blob");
    }
}

/// Read one image blob back, or `None` when it is absent (swept, not yet
/// fetched, or a reference that outlived its blob).
pub fn load_image_blob(data_root: &Path, hex: &str) -> Option<Vec<u8>> {
    std::fs::read(images_dir(data_root).join(hex)).ok()
}

/// Mark/sweep the session's file-sidecar image store against every live graph
/// reference. Only 64-character hex filenames are store members; unrelated
/// files in the directory are left alone. Returns blobs actually removed.
pub fn gc_orphan_image_blobs(data_root: &Path, graph: &Graph) -> usize {
    let referenced: std::collections::HashSet<String> = graph
        .nodes()
        .flat_map(|(_, node)| node.images.values())
        .map(|image| image.hex())
        .collect();
    let Ok(entries) = std::fs::read_dir(images_dir(data_root)) else {
        return 0;
    };
    let mut dropped = 0;
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if !kind.is_file() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let is_digest = name.len() == 64 && name.bytes().all(|b| b.is_ascii_hexdigit());
        if is_digest && !referenced.contains(&name) && std::fs::remove_file(entry.path()).is_ok() {
            dropped += 1;
        }
    }
    dropped
}

/// Restore the persisted per-node facet store (`facets.json`), if one exists.
/// Carries the `arrangement.position` facets (the durable canvas layout — the
/// graph itself is position-free) plus any other namespace. Missing or corrupt
/// starts empty: the canvas keeps its origin park and settles fresh.
pub fn load_node_facets(data_root: &Path) -> Option<session_runtime::NodeFacetStore> {
    match session_runtime::load_node_facets(data_root) {
        Ok(facets) => facets,
        Err(err) => {
            tracing::warn!(%err, "failed to load the facet store; starting empty");
            None
        }
    }
}

/// Persist the per-node facet store at `facets.json`. Best-effort, like the graph.
pub fn save_node_facets(data_root: &Path, facets: &session_runtime::NodeFacetStore) {
    if let Err(err) = session_runtime::save_node_facets(data_root, facets) {
        tracing::warn!(%err, "failed to persist the facet store");
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

/// Repair pre-overmap manifests whose `root_graph_id` is nil: mint each a real
/// `GraphId` and flush. The root graph is the session's container node (the
/// one-node model) — its id keys the `scene.*` facets and is the session's
/// identity in the overmap, so nil ids would collide every pre-overmap session
/// onto one overmap node. Returns how many were healed; the caller migrates
/// each healed session's nil-keyed scene facets on adopt
/// (`App::adopt_session`). Idempotent: a healed store has nothing nil.
pub fn heal_nil_graph_ids(sessions: &mut ManifestStore) -> usize {
    let nil: Vec<SessionId> = sessions
        .iter()
        .filter(|(_, m)| m.root_graph_id == crate::panes::GraphId::nil())
        .map(|(id, _)| id)
        .collect();
    for id in &nil {
        sessions.update(*id, |m| m.root_graph_id = crate::panes::GraphId::new());
    }
    if !nil.is_empty() {
        if let Err(err) = sessions.flush_dirty() {
            tracing::warn!(%err, "failed to persist healed session graph ids");
        }
        tracing::info!(
            count = nil.len(),
            "healed nil root_graph_ids (overmap identity)"
        );
    }
    nil.len()
}

// `save_browser_nodes` / `load_browser_nodes` left with the web.* facet
// convergence (2026-07-20): browser state persists as web.* facets in
// facets.json (write_web_states in the save path, read_web_states on adopt).
// Only the legacy read below remains, for pre-convergence profiles.

/// Read a pre-convergence `browser_nodes.json`, if one exists — the one-time
/// legacy absorb on adopt (facet values win; this only seeds unseen nodes).
/// Missing or corrupt reads empty.
pub fn load_legacy_browser_nodes(
    data_root: &Path,
) -> session_runtime::browser_node_state::BrowserNodeStates {
    match session_runtime::browser_node_state::load_browser_node_states(data_root) {
        Ok(Some(states)) => states,
        Ok(None) => session_runtime::browser_node_state::BrowserNodeStates::new(),
        Err(err) => {
            tracing::warn!(%err, "failed to read the legacy browser-state sidecar; ignoring it");
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
    use sceno::{Arrangement, Spiral};

    fn temp_root(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "turnstone-session-test-{label}-{}",
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
        let mut ma = GraphSessionManifest::new(a, crate::panes::GraphId::nil());
        // `b` is newer (created after), so the fallback picks it.
        std::thread::sleep(std::time::Duration::from_millis(10));
        let mb = GraphSessionManifest::new(b, crate::panes::GraphId::nil());
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

    #[test]
    fn projection_score_sidecar_round_trips_without_becoming_graph_truth() {
        let root = temp_root("projection-score");
        let score = Score::new(Arrangement::Spiral(Spiral::default()));
        save_projection_score(&root, &score);
        assert_eq!(load_projection_score(&root), Some(score));
        assert!(projection_score_path(&root).exists());
        assert!(
            !root.join(session_graph_store::GRAPH_FILE).exists(),
            "the score remains a view sidecar"
        );
    }

    #[test]
    fn place_binding_round_trips_as_a_distinct_public_sidecar() {
        let root = temp_root("place-binding");
        let binding = PlaceBindingV1::new(
            crate::place::PlaceId([0x11; 32]),
            crate::place::SharedContainerId([0x22; 32]),
            crate::place::ChatSpaceId([0x33; 32]),
            "commons",
        )
        .unwrap();

        assert_eq!(load_place_binding(&root).unwrap(), None);
        save_place_binding(&root, &binding).unwrap();
        assert_eq!(load_place_binding(&root).unwrap(), Some(binding));
        assert!(
            !root.join(session_graph_store::GRAPH_FILE).exists(),
            "the place binding remains beside graph truth"
        );
    }

    #[test]
    fn unsupported_place_binding_stays_a_visible_error() {
        let root = temp_root("place-version");
        let binding = PlaceBindingV1::new(
            crate::place::PlaceId([0x11; 32]),
            crate::place::SharedContainerId([0x22; 32]),
            crate::place::ChatSpaceId([0x33; 32]),
            "commons",
        )
        .unwrap();
        let mut value = serde_json::to_value(binding).unwrap();
        value["version"] = serde_json::json!(99);
        std::fs::write(
            place_binding_path(&root),
            serde_json::to_vec_pretty(&value).unwrap(),
        )
        .unwrap();

        let error = load_place_binding(&root).unwrap_err();
        assert!(error.to_string().contains("unsupported place binding version 99"));
        assert!(
            place_binding_path(&root).exists(),
            "a failed load does not erase the evidence"
        );
    }

    #[test]
    fn legacy_node_columns_migrate_on_load_and_existing_facets_win() {
        let root = temp_root("node-facet-migration");
        let mut canvas = mere::canvas::Canvas::new();
        let key = canvas.visit("https://legacy.example/");
        let node_id = canvas.graph().get_node(key).unwrap().id;
        save_session_graph(&root, canvas.graph());

        let graph_file = root.join(session_graph_store::GRAPH_FILE);
        let mut snapshot = session_graph_store::load_snapshot(&graph_file)
            .unwrap()
            .unwrap();
        snapshot.nodes[0].is_pinned = true;
        std::fs::write(
            &graph_file,
            serde_json::to_string_pretty(&snapshot).unwrap(),
        )
        .unwrap();

        let mut facets = session_runtime::NodeFacetStore::new();
        facets
            .set(
                node_id,
                chartulary::FacetId::new(mere::kernel::graph::node_facets::ARRANGEMENT_PIN),
                serde_json::json!(false),
                &chartulary::AcceptAll,
            )
            .unwrap();
        save_node_facets(&root, &facets);

        let restored = load_session_graph(&root).expect("legacy graph");
        let restored_key = restored.get_node_key_by_id(node_id).unwrap();
        assert_eq!(
            restored.node_is_pinned(restored_key),
            Some(false),
            "the canonical sidecar overlays the imported legacy value"
        );

        let canonical = session_graph_store::load_snapshot(&graph_file)
            .unwrap()
            .unwrap();
        assert!(!canonical.nodes[0].is_pinned);
        assert_eq!(canonical.legacy_node_facet_count(), 0);
        let persisted = load_node_facets(&root).unwrap();
        assert_eq!(
            persisted.get(
                &node_id,
                &chartulary::FacetId::new(mere::kernel::graph::node_facets::ARRANGEMENT_PIN,),
            ),
            Some(&serde_json::json!(false))
        );
    }

    #[test]
    fn image_gc_keeps_live_blobs_and_drops_only_hash_named_orphans() {
        let root = temp_root("image-gc");
        let live = mere::kernel::types::ImageRef::new([1; 32], 1, 1);
        let orphan = mere::kernel::types::ImageRef::new([2; 32], 1, 1);
        save_image_blob(&root, &live.hex(), b"live");
        save_image_blob(&root, &orphan.hex(), b"orphan");
        std::fs::write(images_dir(&root).join("README"), b"not a blob").unwrap();

        let mut canvas = mere::canvas::Canvas::new();
        let key = canvas.visit("https://live.example/");
        let node = canvas.graph().get_node(key).unwrap().id;
        assert!(canvas.set_node_favicon_for(node, live));

        assert_eq!(gc_orphan_image_blobs(&root, canvas.graph()), 1);
        assert!(load_image_blob(&root, &live.hex()).is_some());
        assert!(load_image_blob(&root, &orphan.hex()).is_none());
        assert!(images_dir(&root).join("README").exists());
        assert_eq!(
            gc_orphan_image_blobs(&root, canvas.graph()),
            0,
            "re-running the sweep is a no-op"
        );
    }
}
