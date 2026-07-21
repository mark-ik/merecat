//! The Gloss pane: the graph's minimap, on cambium's `graph_canvas_swatch` —
//! the first custom-paint leaf a merecat pane renders, which is why this pane
//! exists before its outline half does. The swatch view carries a
//! `<custom-leaf>`; painting it forces the host through the leaf-render
//! pipeline (`scene_from_dom_with_leaves`) every later leaf consumer (Steward's
//! Meter, the Workbench's node-body Swatch) reuses.
//!
//! Data comes from graph truth via `Canvas::minimap_geometry` — the method mere
//! grew for exactly this consumer ("the gloss pane draws its own swatch from
//! this rather than rendering a second canvas"). Node colour carries node
//! identity from mere's palette (`accent`), per the NODE_SHEET rule: a live
//! node is green, a failed one red, an idle one blue, the focused one the
//! selection orange — in the minimap exactly as on the canvas.
//!
//! Interaction follows the tab strip's mirror-then-drain shape, not the grid's
//! action bubbling: the swatch's handlers are `Fn(&mut State, Id)` mutators, so
//! a click records an intent in the pane's own state and [`GlossPane::click`]
//! drains it. The shell lowers `Navigate` through `Action::OpenAddress` — a
//! minimap node click and a Roster row click land on the same spine.

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
use crate::content::NodeContent;

/// The stable key the swatch's `<custom-leaf>` and the registry share.
const MINIMAP_LEAF: u64 = 1;

/// Inset (px) of the swatch from the pane edges.
const SWATCH_PAD: f32 = 12.0;

/// What a Gloss interaction asks of the shell. Recorded in pane state by the
/// swatch's mutator handlers and drained by [`GlossPane::click`] (the swatch
/// mutates state; it does not bubble actions).
#[derive(Clone, Debug, PartialEq)]
pub enum GlossIntent {
    /// Land on this node (the shell lowers `Action::OpenAddress`).
    Navigate(String),
    /// Jump to the full canvas (the swatch's Expand button).
    Expand,
}

/// The minimap's node identity: mere's node-state colour classes.
#[derive(Clone, Debug, PartialEq)]
struct GlossKind(NodeState);

struct GlossState {
    swatch: GraphCanvasSwatch<Uuid, GlossKind>,
    /// Each node's address, for lowering a click as a navigation.
    urls: HashMap<Uuid, String>,
    /// Intents the swatch's handlers recorded, awaiting the drain.
    pending: Vec<GlossIntent>,
    viewport_w: f32,
    viewport_h: f32,
}

type GlossView = Box<dyn AnyView<GlossState, (), GenetCtx, GenetElement>>;
type GlossRunner = GenetAppRunner<GlossState, fn(&GlossState) -> GlossView, GlossView, ()>;

/// The pane view: the pane's backdrop (geometry inline, colour from the host
/// sheet's `.pane`, like every cambium pane) holding the swatch. The swatch's
/// own container is inline-positioned by the component; the backdrop insets it.
fn gloss_view(state: &GlossState) -> GlossView {
    let swatch = graph_canvas_swatch(
        &state.swatch,
        |state: &mut GlossState, id: Uuid| {
            if let Some(url) = state.urls.get(&id) {
                let url = url.clone();
                state.pending.push(GlossIntent::Navigate(url));
            }
        },
        // Pointer-move routing is live (deliver_hover): the handler writes the
        // hover emphasis, and the next sync's paint-leaf rebuild draws it.
        |state: &mut GlossState, id: Option<Uuid>| state.swatch.hovered = id,
        |state: &mut GlossState| state.pending.push(GlossIntent::Expand),
    );
    Box::new(
        cambium::el::<_, GlossState, ()>(
            "div",
            cambium::el::<_, GlossState, ()>("div", swatch).attr(
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

/// The minimap colour for a node's state, from mere's palette — node colour
/// carries node identity everywhere, so the minimap may not invent its own.
fn kind_color(kind: &GlossKind) -> ColorF {
    let [r, g, b] = palette::unit(palette::accent(false, kind.0).bg);
    ColorF { r, g, b, a: 1.0 }
}

/// A node's palette state from the host's content lifecycle (the same data the
/// canvas colours by).
fn node_state(app: &App, id: Uuid) -> NodeState {
    match app.content.get(id) {
        Some(NodeContent::Live) | Some(NodeContent::Requested) => NodeState::Open,
        Some(NodeContent::Failed(_)) => NodeState::Closed,
        _ => NodeState::Idle,
    }
}

/// The Gloss pane: a retained cambium runner (the swatch view) plus the leaf
/// registry and paint cache its `<custom-leaf>` renders through. Held by the
/// shell like the Roster's grid — `!Send`, persistent between the frame that
/// draws it and the click that hits it.
pub struct GlossPane {
    dom: DomHandle,
    runner: GlossRunner,
    registry: LeafRegistry<u64>,
    rendered: RenderedLeaves,
    /// The dom node the pointer last hovered, for Enter/Leave transitions
    /// (the hover contract is edge-triggered, like the browser's).
    last_hover: Option<NodeId>,
}

impl GlossPane {
    pub fn new() -> Self {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let state = GlossState {
            swatch: GraphCanvasSwatch::new(
                MINIMAP_LEAF,
                GraphCanvasSubgraph {
                    nodes: Vec::new(),
                    edges: Vec::new(),
                },
            )
            .with_label("Graph minimap"),
            urls: HashMap::new(),
            pending: Vec::new(),
            viewport_w: 0.0,
            viewport_h: 0.0,
        };
        let runner = GlossRunner::new(dom.clone(), gloss_view as fn(&GlossState) -> GlossView, state);
        Self {
            dom,
            runner,
            registry: LeafRegistry::new(),
            rendered: RenderedLeaves::new(),
            last_hover: None,
        }
    }

    /// Refresh the minimap from graph truth at the pane's size: gather
    /// `minimap_geometry`, normalize world positions into the swatch's `0..1`
    /// space, and re-register the paint leaf. Edge endpoints come back as world
    /// points, not ids; they are matched back to nodes bit-exactly, which holds
    /// because both come from the same positions pass.
    pub fn sync(&mut self, app: &App, pane_w: f32, pane_h: f32) {
        let (geo_nodes, geo_edges) = app.canvas.minimap_geometry();

        // World bbox -> 0..1, preserving aspect by the larger span so the graph
        // is not anamorphically stretched; the swatch's own viewport insets.
        let (mut min_x, mut min_y, mut max_x, mut max_y) =
            (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for (_, (x, y), _, _) in &geo_nodes {
            min_x = min_x.min(*x);
            min_y = min_y.min(*y);
            max_x = max_x.max(*x);
            max_y = max_y.max(*y);
        }
        let span = (max_x - min_x).max(max_y - min_y).max(1e-3);
        let norm = |x: f32, y: f32| ((x - min_x) / span, (y - min_y) / span);

        let mut by_pos: HashMap<(u32, u32), Uuid> = HashMap::new();
        let mut selected = None;
        let mut urls = HashMap::new();
        let nodes: Vec<GraphCanvasNode<Uuid, GlossKind>> = geo_nodes
            .iter()
            .map(|&(id, (x, y), is_selected, _size)| {
                by_pos.insert((x.to_bits(), y.to_bits()), id);
                if is_selected {
                    selected = Some(id);
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
                urls.insert(id, url.clone());
                GraphCanvasNode {
                    id,
                    kind: GlossKind(node_state(app, id)),
                    position: norm(x, y),
                    label,
                    // The url is the node's stable targeting key: two nodes can
                    // share a display label (two pages titled "Example Domain"),
                    // so `click-node` resolves on this `data-key`, not the label.
                    key: Some(url),
                }
            })
            .collect();
        let edges = geo_edges
            .iter()
            .filter_map(|&((ax, ay), (bx, by), _w)| {
                let from = *by_pos.get(&(ax.to_bits(), ay.to_bits()))?;
                let to = *by_pos.get(&(bx.to_bits(), by.to_bits()))?;
                Some(GraphCanvasEdge { from, to })
            })
            .collect();

        let sw = ((pane_w - 2.0 * SWATCH_PAD).max(32.0)) as u32;
        let sh = ((pane_h - 2.0 * SWATCH_PAD).max(32.0)) as u32;
        self.runner.update(|state| {
            state.swatch.graph = GraphCanvasSubgraph { nodes, edges };
            state.swatch.selected = selected;
            state.swatch.width = sw;
            state.swatch.height = sh;
            state.urls = urls;
            state.viewport_w = pane_w;
            state.viewport_h = pane_h;
        });
        self.registry.insert(
            MINIMAP_LEAF,
            Box::new(self.runner.state().swatch.paint_leaf(kind_color)),
        );
    }

    /// The pane's scene at its size: the swatch's leaf renders through the
    /// registry and splices at its `<custom-leaf>` box.
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

    /// Route a click at pane-local `(x, y)` into the view, then drain the
    /// intents the swatch's handlers recorded. The same hit-test path as the
    /// Roster's grid; the difference is where the outcome lives (state, not
    /// bubbled actions), because that is the swatch's contract.
    pub fn click(&mut self, x: f32, y: f32, w: u32, h: u32) -> Vec<GlossIntent> {
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

    /// Resolve a selector to a window point within this pane's DOM at window
    /// rect `rect`, via the shared genet-probe resolver. The minimap's node
    /// buttons carry their url as `data-key`, so `click-node` selects on that —
    /// unique where the display label is not. `node_center`'s bespoke
    /// projection-plus-leaf-box math collapsed here: the node button is a real
    /// positioned DOM element, so `absolute_rect` already knows where it is.
    pub fn resolve(&self, sel: &genet_probe::Selector, rect: [f32; 4]) -> Option<(f32, f32)> {
        let dom = self.dom.borrow();
        let surfaces = [genet_probe::ProbeSurface {
            name: "gloss",
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

    fn pane_on_sample_graph() -> (GlossPane, App) {
        let mut app = App::test_stub();
        app.canvas = mere::canvas::Canvas::with_sample_graph();
        let mut pane = GlossPane::new();
        pane.sync(&app, 480.0, 400.0);
        (pane, app)
    }

    /// The leaf pipeline end to end, headless: the swatch's `<custom-leaf>` is
    /// laid out, the registry paints it at that box, and the spliced scene is
    /// non-trivially larger than the leafless one. This is the pipeline every
    /// later leaf consumer (Meter, node-body Swatch) reuses — the pane exists to
    /// prove it.
    #[test]
    fn minimap_leaf_paints_through_the_pipeline() {
        let (mut pane, _app) = pane_on_sample_graph();
        assert!(
            !pane.runner.state().swatch.graph.nodes.is_empty(),
            "the sample graph must yield minimap nodes"
        );
        assert!(
            !pane.runner.state().swatch.graph.edges.is_empty(),
            "edge endpoints must match back to nodes bit-exactly"
        );
        let _scene = pane.scene(480, 400);
        assert!(
            pane.rendered.get(MINIMAP_LEAF).is_some_and(|c| !c.is_empty()),
            "the minimap leaf must render paint commands at its laid-out box"
        );
    }

    /// A click on a node's hit target records a Navigate intent for that node's
    /// url, drained by `click` — the mirror-then-drain contract. The node is
    /// resolved by its `data-key` (url) through genet-probe, the same path the
    /// shell's `click-node` drives.
    #[test]
    fn clicking_a_node_records_navigate() {
        let (mut pane, _app) = pane_on_sample_graph();
        let key = pane.runner.state().swatch.graph.nodes[0]
            .key
            .clone()
            .expect("every minimap node carries its url as a key");
        let (x, y) = pane
            .resolve(
                &genet_probe::Selector::class("graph-canvas-swatch-node")
                    .with_attr("data-key", &key),
                [0.0, 0.0, 480.0, 400.0],
            )
            .expect("the node must resolve by its data-key");
        let intents = pane.click(x, y, 480, 400);
        assert!(
            matches!(&intents[..], [GlossIntent::Navigate(url)] if *url == key),
            "a node click must drain its Navigate intent for that node's url, got {intents:?}"
        );
    }
}
