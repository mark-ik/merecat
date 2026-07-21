//! The Overmap pane (O1): the switcher as a GRAPH VIEW — the overmap graph
//! (sessions as container nodes, fork lineage as edges) rendered on the same
//! `graph_canvas_swatch` custom-paint leaf the Gloss minimap proved, with a
//! session-node click lowering the ordinary `Action::SwitchSession` through
//! the spine. Navigating to a container IS the switch (the overmap ruling's
//! v0); the list switcher (omnibar `>`) stays until this earns its keep.
//!
//! Layout: lineage generations, left to right — a root session sits in
//! column 0, its forks in column 1, theirs in column 2 — so the fork
//! ancestry reads as a little tree. Analytic and derived per sync; the
//! overmap graph carries no positions (positions are never graph truth).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use cambium::{
    AnyView, DomHandle, GenetAppRunner, GenetCtx, GenetElement, GraphCanvasEdge, GraphCanvasNode,
    GraphCanvasSubgraph, GraphCanvasSwatch, graph_canvas_swatch,
};
use genet_layout::{IncrementalLayout, ScrollOffsets};
use genet_scripted_dom::{NodeId, ScriptedDom};
use mere::canvas::NodeState;
use mere::canvas::palette;
use sprigging::{ColorF, LeafRegistry, RenderedLeaves};
use uuid::Uuid;

use crate::app::App;
use crate::overmap;

/// The stable key the swatch's `<custom-leaf>` and the registry share.
const OVERMAP_LEAF: u64 = 2;

/// Inset (px) of the swatch from the pane edges.
const SWATCH_PAD: f32 = 12.0;

/// What an Overmap interaction asks of the shell.
#[derive(Clone, Debug, PartialEq)]
pub enum OvermapIntent {
    /// Adopt this session (the shell lowers `Action::SwitchSession`).
    Switch(frisket::SessionId),
    /// Jump back to the canvas (the swatch's Expand chip — leave the overmap).
    Expand,
}

/// The overmap's node identity colour: the current session reads as the open
/// one, every other as idle — the same palette classes the canvas uses, so a
/// container node carries node identity like any node.
#[derive(Clone, Debug, PartialEq)]
struct OvermapKind(NodeState);

struct OvermapState {
    swatch: GraphCanvasSwatch<Uuid, OvermapKind>,
    /// Each container node's session, for lowering a click as a switch.
    session_of: HashMap<Uuid, frisket::SessionId>,
    pending: Vec<OvermapIntent>,
    viewport_w: f32,
    viewport_h: f32,
}

type OvermapView = Box<dyn AnyView<OvermapState, (), GenetCtx, GenetElement>>;
type OvermapRunner = GenetAppRunner<OvermapState, fn(&OvermapState) -> OvermapView, OvermapView, ()>;

fn overmap_view(state: &OvermapState) -> OvermapView {
    let swatch = graph_canvas_swatch(
        &state.swatch,
        |state: &mut OvermapState, id: Uuid| {
            if let Some(session) = state.session_of.get(&id) {
                state.pending.push(OvermapIntent::Switch(*session));
            }
        },
        // Pointer-move routing is live (deliver_hover): the handler writes the
        // hover emphasis, and the next sync's paint-leaf rebuild draws it.
        |state: &mut OvermapState, id: Option<Uuid>| state.swatch.hovered = id,
        // Expand = leave the overmap for the canvas (the Gloss semantics: the
        // full view of the CURRENT session is the canvas).
        |state: &mut OvermapState| state.pending.push(OvermapIntent::Expand),
    );
    Box::new(
        cambium::el::<_, OvermapState, ()>(
            "div",
            cambium::el::<_, OvermapState, ()>("div", swatch).attr(
                "style",
                format!("position: absolute; left: {SWATCH_PAD}px; top: {SWATCH_PAD}px;"),
            ),
        )
        .attr("class", "pane")
        .attr(
            "style",
            format!(
                "position: relative; width: {}px; height: {}px;",
                state.viewport_w, state.viewport_h
            ),
        ),
    )
}

fn kind_color(kind: &OvermapKind) -> ColorF {
    let [r, g, b] = palette::unit(palette::accent(false, kind.0).bg);
    ColorF { r, g, b, a: 1.0 }
}

/// The Overmap pane: a retained cambium runner over the swatch, like the
/// Gloss minimap — `!Send`, persistent between the frame that draws it and
/// the click that hits it.
pub struct OvermapPane {
    dom: DomHandle,
    runner: OvermapRunner,
    registry: LeafRegistry<u64>,
    rendered: RenderedLeaves,
    /// The dom node the pointer last hovered, for Enter/Leave transitions
    /// (the hover contract is edge-triggered, like the browser's).
    last_hover: Option<NodeId>,
}

impl OvermapPane {
    pub fn new() -> Self {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let state = OvermapState {
            swatch: GraphCanvasSwatch::new(
                OVERMAP_LEAF,
                GraphCanvasSubgraph {
                    nodes: Vec::new(),
                    edges: Vec::new(),
                },
            )
            .with_label("Session overmap")
            // Identity is the point of an overview of sessions: labels render.
            .with_node_labels(true),
            session_of: HashMap::new(),
            pending: Vec::new(),
            viewport_w: 0.0,
            viewport_h: 0.0,
        };
        let runner =
            OvermapRunner::new(dom.clone(), overmap_view as fn(&OvermapState) -> OvermapView, state);
        Self {
            dom,
            runner,
            registry: LeafRegistry::new(),
            rendered: RenderedLeaves::new(),
            last_hover: None,
        }
    }

    /// Refresh from the derived overmap graph at the pane's size: lineage
    /// generations left → right, siblings stacked top → bottom, normalized
    /// into the swatch's `0..1` space.
    pub fn sync(&mut self, app: &App, pane_w: f32, pane_h: f32) {
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

        // Column per depth, row per sibling index within the depth, in the
        // graph's (insertion) order so the layout is stable across syncs.
        let mut row_at_depth: HashMap<usize, usize> = HashMap::new();
        let mut placed: Vec<(Uuid, usize, usize)> = Vec::new();
        let mut max_depth = 0usize;
        let mut max_row = 0usize;
        for (_, node) in graph.nodes() {
            let depth = depth_of(node.id);
            let row = *row_at_depth
                .entry(depth)
                .and_modify(|r| *r += 1)
                .or_insert(0);
            max_depth = max_depth.max(depth);
            max_row = max_row.max(row);
            placed.push((node.id, depth, row));
        }
        // Normalize into a padded band, centering degenerate axes (one
        // generation, or one row) at 0.5 so a small overmap reads as a
        // composed diagram rather than dots pinned to the pane corners.
        let band = |t: f32| 0.15 + t * 0.7;
        let axis = |value: usize, max: usize| {
            if max == 0 {
                0.5
            } else {
                band(value as f32 / max as f32)
            }
        };
        let norm = |depth: usize, row: usize| (axis(depth, max_depth), axis(row, max_row));

        let current_container = app.container_id();
        let mut session_of = HashMap::new();
        let mut selected = None;
        let nodes: Vec<GraphCanvasNode<Uuid, OvermapKind>> = placed
            .iter()
            .map(|&(id, depth, row)| {
                let (key, node) = graph.get_node_by_id(id).expect("placed from this graph");
                let session = overmap::session_of_url(node.url());
                if let Some(session) = session {
                    session_of.insert(id, session);
                }
                let is_current = current_container == Some(id);
                if is_current {
                    selected = Some(id);
                }
                let state = if is_current { NodeState::Open } else { NodeState::Idle };
                GraphCanvasNode {
                    id,
                    kind: OvermapKind(state),
                    position: norm(depth, row),
                    label: graph.node_display_label(key),
                    // The session id is the stable targeting key (labels can
                    // repeat); `click-node` and the probe resolve on this.
                    key: session.map(|s| s.0.to_string()),
                }
            })
            .collect();
        let edges = graph
            .relations()
            .filter_map(|rel| {
                Some(GraphCanvasEdge {
                    from: *by_key.get(&rel.from)?,
                    to: *by_key.get(&rel.to)?,
                })
            })
            .collect();

        let sw = ((pane_w - 2.0 * SWATCH_PAD).max(32.0)) as u32;
        let sh = ((pane_h - 2.0 * SWATCH_PAD).max(32.0)) as u32;
        self.runner.update(|state| {
            state.swatch.graph = GraphCanvasSubgraph { nodes, edges };
            state.swatch.selected = selected;
            state.swatch.width = sw;
            state.swatch.height = sh;
            state.session_of = session_of;
            state.viewport_w = pane_w;
            state.viewport_h = pane_h;
        });
        self.registry.insert(
            OVERMAP_LEAF,
            Box::new(self.runner.state().swatch.paint_leaf(kind_color)),
        );
    }

    /// The pane's scene at its size (the shared cambium leaf pipeline).
    pub fn scene(&mut self, w: u32, h: u32) -> netrender::Scene {
        crate::ui::scene_from_dom_with_leaves(
            &self.dom.borrow(),
            crate::ui::CAMBIUM_SHEET,
            w,
            h,
            &mut self.registry,
            &mut self.rendered,
        )
    }

    /// Route a pointer MOVE at pane-local `(x, y)`: hit-test the dom and
    /// dispatch the Enter/Leave hover transitions; the view's handler writes
    /// `swatch.hovered` and the next sync repaints the emphasis. Returns
    /// whether the hover target changed (the host redraws on true).
    pub fn hover(&mut self, x: f32, y: f32, w: u32, h: u32) -> bool {
        let hit = {
            let dom = self.dom.borrow();
            let layout =
                IncrementalLayout::new(&*dom, &[crate::ui::CAMBIUM_SHEET], w as f32, h as f32);
            let scroll = ScrollOffsets::<NodeId>::default();
            layout.hit_test(&*dom, x, y, &scroll)
        };
        if hit == self.last_hover {
            return false;
        }
        if let Some(prev) = self.last_hover {
            let _ = self.runner.dispatch_hover(
                prev,
                cambium::HoverEvent::new(cambium::HoverPhase::Leave, (x, y), (x, y)),
            );
        }
        if let Some(node) = hit {
            let _ = self.runner.dispatch_hover(
                node,
                cambium::HoverEvent::new(cambium::HoverPhase::Enter, (x, y), (x, y)),
            );
        }
        self.last_hover = hit;
        true
    }

    /// The pointer left this pane: dispatch the pending Leave (if any) so the
    /// hover emphasis clears. Returns whether anything changed.
    pub fn hover_leave(&mut self) -> bool {
        let Some(prev) = self.last_hover.take() else {
            return false;
        };
        let _ = self.runner.dispatch_hover(
            prev,
            cambium::HoverEvent::new(cambium::HoverPhase::Leave, (0.0, 0.0), (0.0, 0.0)),
        );
        true
    }

    /// Route a click at pane-local `(x, y)`; drain the recorded intents.
    pub fn click(&mut self, x: f32, y: f32, w: u32, h: u32) -> Vec<OvermapIntent> {
        let hit = {
            let dom = self.dom.borrow();
            let layout =
                IncrementalLayout::new(&*dom, &[crate::ui::CAMBIUM_SHEET], w as f32, h as f32);
            let scroll = ScrollOffsets::<NodeId>::default();
            layout.hit_test(&*dom, x, y, &scroll)
        };
        if let Some(node) = hit {
            let _ = self
                .runner
                .dispatch_click(node, cambium::PointerClick::at((x, y)));
        }
        let mut drained = Vec::new();
        self.runner
            .update(|state| drained = std::mem::take(&mut state.pending));
        drained
    }

    /// Resolve a probe selector within this pane's DOM (the session-node
    /// buttons carry the session id as `data-key`).
    pub fn resolve(&self, sel: &genet_probe::Selector, rect: [f32; 4]) -> Option<(f32, f32)> {
        let dom = self.dom.borrow();
        let surfaces = [genet_probe::ProbeSurface {
            name: "overmap",
            dom: &dom,
            rect,
            sheet: crate::ui::CAMBIUM_SHEET,
        }];
        genet_probe::resolve(&surfaces, sel).map(|h| h.point)
    }

    /// Borrow this pane's DOM for the shared driver's `with_surfaces`.
    pub fn dom_ref(&self) -> std::cell::Ref<'_, ScriptedDom> {
        self.dom.borrow()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An app with a donor + fork session pair (the fork wired the lineage),
    /// current = the fork.
    fn pane_on_fork_pair() -> (OvermapPane, App, frisket::SessionId) {
        let mut app = App::test_stub();
        let donor = app.session_id;
        let mut donor_m = session_runtime::GraphSessionManifest::new(
            donor,
            frisket::GraphId::from_uuid(uuid::Uuid::from_u128(0xd0)),
        );
        donor_m.display_name = Some("home".to_string());
        app.sessions.insert(donor_m);
        let fork = frisket::SessionId::new();
        let mut fork_m = session_runtime::GraphSessionManifest::new(
            fork,
            frisket::GraphId::from_uuid(uuid::Uuid::from_u128(0xf0)),
        );
        fork_m.parent_session = Some(donor);
        app.sessions.insert(fork_m);
        app.session_id = fork;
        let mut pane = OvermapPane::new();
        pane.sync(&app, 480.0, 400.0);
        (pane, app, donor)
    }

    /// The overmap renders both sessions and the lineage edge, and the pane's
    /// leaf paints through the shared pipeline.
    #[test]
    fn overmap_leaf_paints_sessions_and_lineage() {
        let (mut pane, _app, _donor) = pane_on_fork_pair();
        assert_eq!(pane.runner.state().swatch.graph.nodes.len(), 2);
        assert_eq!(
            pane.runner.state().swatch.graph.edges.len(),
            1,
            "the fork's lineage edge renders"
        );
        assert_eq!(
            pane.runner.state().swatch.selected,
            Some(uuid::Uuid::from_u128(0xf0)),
            "the current session's container is the selected node"
        );
        let _scene = pane.scene(480, 400);
        assert!(
            pane.rendered.get(OVERMAP_LEAF).is_some_and(|c| !c.is_empty()),
            "the overmap leaf must render paint commands at its laid-out box"
        );
    }

    /// Pointer-move routing: hovering a session node writes the hover
    /// emphasis; moving off the node (or off the pane) clears it.
    #[test]
    fn hovering_a_session_node_sets_and_clears_emphasis() {
        let (mut pane, _app, donor) = pane_on_fork_pair();
        let (x, y) = pane
            .resolve(
                &genet_probe::Selector::class("graph-canvas-swatch-node")
                    .with_attr("data-key", &donor.0.to_string()),
                [0.0, 0.0, 480.0, 400.0],
            )
            .expect("the donor session node resolves");
        assert!(pane.hover(x, y, 480, 400), "entering the node is a change");
        assert_eq!(
            pane.runner.state().swatch.hovered,
            Some(uuid::Uuid::from_u128(0xd0)),
            "the hover emphasis names the hovered container"
        );
        assert!(!pane.hover(x, y, 480, 400), "same target, no re-dispatch");
        assert!(pane.hover_leave(), "leaving the pane is a change");
        assert_eq!(pane.runner.state().swatch.hovered, None, "emphasis cleared");
    }

    /// Clicking a session node records the Switch intent for THAT session —
    /// resolved by its `data-key` (the session id), the same path the shell's
    /// `click-node` drives.
    #[test]
    fn clicking_a_session_node_records_switch() {
        let (mut pane, _app, donor) = pane_on_fork_pair();
        let (x, y) = pane
            .resolve(
                &genet_probe::Selector::class("graph-canvas-swatch-node")
                    .with_attr("data-key", &donor.0.to_string()),
                [0.0, 0.0, 480.0, 400.0],
            )
            .expect("the donor session node resolves by its data-key");
        let intents = pane.click(x, y, 480, 400);
        assert!(
            matches!(&intents[..], [OvermapIntent::Switch(id)] if *id == donor),
            "a session-node click drains Switch for that session, got {intents:?}"
        );
    }
}
