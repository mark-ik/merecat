//! Swatch presets: the named projections a swatch pane can show, each a pure
//! `gather: fn(&App) -> SwatchModel` — the same currency as a section
//! provider, so a pane pulls presets and sections the same way.

use std::collections::HashMap;

use mere::canvas::{NodeState, palette};
use sprigging::ColorF;
use uuid::Uuid;

use crate::app::App;
use crate::content::NodeContent;
use crate::overmap;

use super::{SwatchActivate, SwatchModel, SwatchNode};

/// A named swatch projection: the pane config the gloss-composite design
/// calls a preset. `gather` is a pure `fn(&App) -> SwatchModel` — the same
/// currency as the section providers, so presets compose the same way.
#[derive(Clone, Copy)]
pub struct ProjectionPreset {
    /// Stable id (`"gloss.minimap"`, `"overmap.lineage"`).
    pub id: &'static str,
    /// The swatch's accessible label.
    pub label: &'static str,
    /// The `<custom-leaf>` registry key (stable per preset).
    pub leaf_key: u64,
    /// Whether node labels render as visible text.
    pub node_labels: bool,
    /// Whether the Expand chip renders.
    pub expand: bool,
    /// Gather the projection from app truth.
    pub gather: fn(&App) -> SwatchModel,
}

/// The Gloss minimap as a preset: the live canvas geometry, colored by
/// content state, a node click navigating to its url. Labels off (minimap
/// density); Expand on (the canvas IS the fuller view).
pub const GLOSS_MINIMAP: ProjectionPreset = ProjectionPreset {
    id: "gloss.minimap",
    label: "Graph minimap",
    leaf_key: 1,
    node_labels: false,
    expand: true,
    gather: gloss_gather,
};

/// The Overmap as a preset: sessions as container nodes with fork lineage,
/// laid out by generation, a node click switching sessions. Labels on
/// (session identity is the point); Expand on (leave the overmap for the
/// canvas).
pub const OVERMAP_LINEAGE: ProjectionPreset = ProjectionPreset {
    id: "overmap.lineage",
    label: "Session overmap",
    leaf_key: 2,
    node_labels: true,
    expand: true,
    gather: overmap_gather,
};

/// A node's palette state from the host's content lifecycle (the same data
/// the canvas colors by).
fn content_state(app: &App, id: Uuid) -> NodeState {
    match app.content.get(id) {
        Some(NodeContent::Live) | Some(NodeContent::Requested) => NodeState::Open,
        Some(NodeContent::Failed(_)) => NodeState::Closed,
        _ => NodeState::Idle,
    }
}

/// The minimap gather: `Canvas::minimap_geometry` normalized into `0..1`
/// (aspect preserved by the larger span), each node keyed + activated by its
/// url. (The GlossPane sync, now a pure function of app truth.)
fn gloss_gather(app: &App) -> SwatchModel {
    let (geo_nodes, geo_edges) = app.canvas.minimap_geometry();

    let (mut min_x, mut min_y, mut max_x, mut max_y) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for (_, (x, y), _, _) in &geo_nodes {
        min_x = min_x.min(*x);
        min_y = min_y.min(*y);
        max_x = max_x.max(*x);
        max_y = max_y.max(*y);
    }
    let span = (max_x - min_x).max(max_y - min_y).max(1e-3);
    let norm = |x: f32, y: f32| ((x - min_x) / span, (y - min_y) / span);

    let mut by_pos: HashMap<(u32, u32), Uuid> = HashMap::new();
    let mut model = SwatchModel::default();
    for &(id, (x, y), is_selected, _size) in &geo_nodes {
        by_pos.insert((x.to_bits(), y.to_bits()), id);
        if is_selected {
            model.selected = Some(id);
        }
        let (label, url) = app
            .canvas
            .graph()
            .get_node_by_id(id)
            .map(|(key, node)| {
                (
                    app.canvas.graph().node_display_label(key),
                    node.url().to_string(),
                )
            })
            .unwrap_or_default();
        model.nodes.push(SwatchNode {
            id,
            position: norm(x, y),
            state: content_state(app, id),
            label,
            // The url is the node's stable targeting key: two nodes can share
            // a display label, so `click-node` resolves on this, not the label.
            key: Some(url.clone()),
            activate: Some(SwatchActivate::Open(url)),
        });
    }
    // Edge endpoints come back as world points; matched back bit-exactly,
    // which holds because both come from the same positions pass.
    for &((ax, ay), (bx, by), _w) in &geo_edges {
        if let (Some(&from), Some(&to)) = (
            by_pos.get(&(ax.to_bits(), ay.to_bits())),
            by_pos.get(&(bx.to_bits(), by.to_bits())),
        ) {
            model.edges.push((from, to));
        }
    }
    model
}

/// The overmap gather: the derived session graph laid out by lineage
/// generation (left → right, siblings stacked), each node keyed by its
/// session id and activated as a switch. (The OvermapPane sync, now a pure
/// function of app truth.)
fn overmap_gather(app: &App) -> SwatchModel {
    let graph = overmap::overmap_graph(&app.sessions);

    // Depth by lineage: the CopiedFrom edge points child -> parent.
    let mut parent_of: HashMap<Uuid, Uuid> = HashMap::new();
    let mut by_key: HashMap<mere::kernel::graph::NodeKey, Uuid> = HashMap::new();
    for (key, node) in graph.nodes() {
        by_key.insert(key, node.id);
    }
    for rel in graph.relations() {
        if let (Some(&child), Some(&parent)) = (by_key.get(&rel.from), by_key.get(&rel.to)) {
            parent_of.entry(child).or_insert(parent);
        }
    }
    let depth_of = |mut id: Uuid| -> usize {
        let mut depth = 0usize;
        let mut hops = 0usize;
        while let Some(&parent) = parent_of.get(&id) {
            depth += 1;
            id = parent;
            hops += 1;
            if hops > 64 {
                break; // cycle guard; lineage is a tree in honest data
            }
        }
        depth
    };

    let mut row_at_depth: HashMap<usize, usize> = HashMap::new();
    let mut placed: Vec<(Uuid, usize, usize)> = Vec::new();
    let (mut max_depth, mut max_row) = (0usize, 0usize);
    for (_, node) in graph.nodes() {
        let depth = depth_of(node.id);
        let row = *row_at_depth.entry(depth).and_modify(|r| *r += 1).or_insert(0);
        max_depth = max_depth.max(depth);
        max_row = max_row.max(row);
        placed.push((node.id, depth, row));
    }
    // Padded band, centering degenerate axes at 0.5 so a small overmap reads
    // as a composed diagram rather than dots pinned to the pane corners. The
    // right pad is deeper than the left: node labels render rightward, and a
    // last-generation label must not clip at the swatch edge.
    let band = |t: f32| 0.12 + t * 0.62;
    let axis = |value: usize, max: usize| {
        if max == 0 { 0.5 } else { band(value as f32 / max as f32) }
    };

    let current_container = app.container_id();
    let mut model = SwatchModel::default();
    for &(id, depth, row) in &placed {
        let (key, node) = graph.get_node_by_id(id).expect("placed from this graph");
        let session = overmap::session_of_url(node.url());
        let is_current = current_container == Some(id);
        if is_current {
            model.selected = Some(id);
        }
        model.nodes.push(SwatchNode {
            id,
            position: (axis(depth, max_depth), axis(row, max_row)),
            state: if is_current { NodeState::Open } else { NodeState::Idle },
            label: graph.node_display_label(key),
            key: session.map(|s| s.0.to_string()),
            activate: session.map(SwatchActivate::Switch),
        });
    }
    for rel in graph.relations() {
        if let (Some(&from), Some(&to)) = (by_key.get(&rel.from), by_key.get(&rel.to)) {
            model.edges.push((from, to));
        }
    }
    model
}

/// The swatch's node identity color, from mere's palette — node color carries
/// node identity everywhere, so no swatch may invent its own.
pub(super) fn state_color(state: &NodeState) -> ColorF {
    let [r, g, b] = palette::unit(palette::accent(false, *state).bg);
    ColorF { r, g, b, a: 1.0 }
}
