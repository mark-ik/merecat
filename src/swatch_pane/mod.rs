//! The swatch as a customizable projection (design_docs/
//! 2026-07-20_gloss_composite_pane.md).
//!
//! [`SwatchPane`] is the ONE retained pane over any [`ProjectionPreset`]; the
//! Gloss minimap and the Overmap are two presets of it. A preset carries what
//! is pane-specific as DATA plus one gather fn, including what a node click
//! MEANS ([`SwatchActivate`] per node), so a new swatch consumer needs no
//! handler code. Composed sections come from the pane's own leaf, not the
//! preset: the swatch is the preset, the sections are per-pane config.

mod presets;
use presets::state_color;
pub use presets::{GLOSS_MINIMAP, OVERMAP_LINEAGE, ProjectionPreset};

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
use crate::overmap;

/// Inset (px) of the swatch from the pane edges.
const SWATCH_PAD: f32 = 12.0;
/// When a swatch preset composes sections, the swatch takes this fraction of
/// the pane height and the sections stack below (the gloss-composite split).
const SWATCH_FRACTION: f32 = 0.55;

/// What activating (clicking) a swatch node means — data on the node, so the
/// one pane needs no per-preset handler code and the shell lowers each variant
/// through the ordinary spine.
#[derive(Clone, Debug, PartialEq)]
pub enum SwatchActivate {
    /// Land on this address (`Action::OpenAddress`).
    Open(String),
    /// Adopt this session (`Action::SwitchSession`).
    Switch(crate::panes::SessionId),
    /// Recover a removed node by its ORIGINAL id
    /// (`Action::RecoverDeletedNode`) — a composed Removed row's click.
    Recover(uuid::Uuid),
}

/// What a swatch-pane interaction asks of the shell.
#[derive(Clone, Debug, PartialEq)]
pub enum SwatchIntent {
    /// A node was activated; its meaning rode the node as data.
    Activate(SwatchActivate),
    /// The Expand chip: jump to the app's full view (the canvas).
    Expand,
}

/// One gathered node: identity, normalized position, palette state, label,
/// probe key, and what activating it means.
#[derive(Clone, Debug, PartialEq)]
pub struct SwatchNode {
    pub id: Uuid,
    /// Normalized `0..1` scene position.
    pub position: (f32, f32),
    /// The palette state (node color carries node identity everywhere).
    pub state: NodeState,
    pub label: String,
    /// The stable `data-key` a probe / `click-node` targets by.
    pub key: Option<String>,
    pub activate: Option<SwatchActivate>,
}

/// The gathered projection: what the swatch renders this frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SwatchModel {
    pub nodes: Vec<SwatchNode>,
    pub edges: Vec<(Uuid, Uuid)>,
    pub selected: Option<Uuid>,
}


struct SwatchState {
    swatch: GraphCanvasSwatch<Uuid, NodeState>,
    /// Each node's activation meaning, by id (data off the gather).
    activate_of: HashMap<Uuid, SwatchActivate>,
    pending: Vec<SwatchIntent>,
    viewport_w: f32,
    viewport_h: f32,
    /// The composed sections (title + rows) rendered below the swatch, gathered
    /// from the preset's providers. Empty for a fill-the-pane swatch.
    sections: Vec<(&'static str, Vec<crate::sections::SectionRow>)>,
    /// The swatch area's height (px): the whole pane when there are no
    /// sections, the top fraction when there are. The sections stack below it.
    swatch_h: f32,
}

type SwatchView = Box<dyn AnyView<SwatchState, (), GenetCtx, GenetElement>>;
type SwatchRunner = GenetAppRunner<SwatchState, fn(&SwatchState) -> SwatchView, SwatchView, ()>;

fn swatch_view(state: &SwatchState) -> SwatchView {
    let swatch = graph_canvas_swatch(
        &state.swatch,
        |state: &mut SwatchState, id: Uuid| {
            if let Some(activate) = state.activate_of.get(&id) {
                state.pending.push(SwatchIntent::Activate(activate.clone()));
            }
        },
        // Pointer-move routing writes the hover emphasis; the next sync's
        // paint-leaf rebuild draws it.
        |state: &mut SwatchState, id: Option<Uuid>| state.swatch.hovered = id,
        |state: &mut SwatchState| state.pending.push(SwatchIntent::Expand),
    );
    // The swatch fills the top (its own leaf is sized to `swatch_h` in sync).
    let mut children: Vec<SwatchView> = vec![Box::new(
        cambium::el::<_, SwatchState, ()>("div", swatch).attr(
            "style",
            format!("position: absolute; left: {SWATCH_PAD}px; top: {SWATCH_PAD}px;"),
        ),
    )];

    // Composed sections stack below the swatch in a scrollable column (the
    // gloss-composite: the minimap plus, say, the recycle bin's Removed rows).
    if !state.sections.is_empty() {
        let mut section_kids: Vec<SwatchView> = Vec::new();
        for (title, rows) in &state.sections {
            section_kids.push(Box::new(
                cambium::el::<_, SwatchState, ()>("div", title.to_string()).attr(
                    "style",
                    "color: #7d8590; padding: 6px 12px 2px; font-size: 11px;",
                ),
            ));
            if rows.is_empty() {
                section_kids.push(Box::new(
                    cambium::el::<_, SwatchState, ()>("div", "nothing here".to_string())
                        .attr("style", "color: #484f58; padding: 2px 12px; font-size: 12px;"),
                ));
            } else {
                for row in rows {
                    // The row's activation rides it as DATA (the swatch node's
                    // rule): the click handler pushes the intent the provider
                    // declared, so a new provider needs no handler code here.
                    // `section-row` is the probe class a receipt addresses.
                    let activate = row.activate.clone();
                    section_kids.push(Box::new(cambium::on_click(
                        cambium::el::<_, SwatchState, ()>("div", row.text.clone())
                            .attr("class", "section-row")
                            .attr(
                                "style",
                                "color: #c9d1d9; padding: 2px 12px; font-size: 12px;",
                            ),
                        move |state: &mut SwatchState, _click: cambium::PointerClick| {
                            match &activate {
                                Some(crate::sections::SectionActivate::Open(url)) => {
                                    state
                                        .pending
                                        .push(SwatchIntent::Activate(SwatchActivate::Open(
                                            url.clone(),
                                        )));
                                }
                                Some(crate::sections::SectionActivate::Recover(id)) => {
                                    state.pending.push(SwatchIntent::Activate(
                                        SwatchActivate::Recover(*id),
                                    ));
                                }
                                None => {}
                            }
                        },
                    )));
                }
            }
        }
        children.push(Box::new(
            cambium::el::<_, SwatchState, ()>("div", section_kids).attr(
                "style",
                format!(
                    "position: absolute; left: 0px; top: {}px; width: {}px; height: {}px; overflow-y: auto;",
                    state.swatch_h,
                    state.viewport_w,
                    (state.viewport_h - state.swatch_h).max(0.0),
                ),
            ),
        ));
    }

    Box::new(
        cambium::el::<_, SwatchState, ()>("div", children)
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

/// The one retained swatch pane, parameterized by its [`ProjectionPreset`]:
/// a cambium runner + the custom-paint leaf pipeline, `!Send`, persistent
/// between the frame that draws it and the click that hits it. Gloss and the
/// Overmap are two instances of this.
pub struct SwatchPane {
    preset: ProjectionPreset,
    /// The composed section providers, set by the host from THIS pane's leaf
    /// config each frame. Empty = the swatch fills the pane.
    sections: Vec<crate::sections::SectionProvider>,
    dom: DomHandle,
    runner: SwatchRunner,
    registry: LeafRegistry<u64>,
    rendered: RenderedLeaves,
    /// The dom node the pointer last hovered, for Enter/Leave transitions
    /// (the hover contract is edge-triggered, like the browser's).
    last_hover: Option<NodeId>,
}

impl SwatchPane {
    pub fn new(preset: ProjectionPreset) -> Self {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let state = SwatchState {
            swatch: GraphCanvasSwatch::new(
                preset.leaf_key,
                GraphCanvasSubgraph {
                    nodes: Vec::new(),
                    edges: Vec::new(),
                },
            )
            .with_label(preset.label)
            .with_node_labels(preset.node_labels)
            .with_expand(preset.expand),
            activate_of: HashMap::new(),
            pending: Vec::new(),
            viewport_w: 0.0,
            viewport_h: 0.0,
            sections: Vec::new(),
            swatch_h: 0.0,
        };
        let runner =
            SwatchRunner::new(dom.clone(), swatch_view as fn(&SwatchState) -> SwatchView, state);
        Self {
            preset,
            sections: Vec::new(),
            dom,
            runner,
            registry: LeafRegistry::new(),
            rendered: RenderedLeaves::new(),
            last_hover: None,
        }
    }

    /// Set the composed section providers for this pane (the host resolves
    /// them from the leaf's `PaneComposition` each frame). Cheap: providers are
    /// `Copy` descriptors, and the rows themselves are gathered in `sync`.
    pub fn set_sections(&mut self, sections: Vec<crate::sections::SectionProvider>) {
        self.sections = sections;
    }

    /// Refresh from app truth at the pane's size: run the preset's gather,
    /// project it into the swatch, re-register the paint leaf.
    pub fn sync(&mut self, app: &App, pane_w: f32, pane_h: f32) {
        let model = (self.preset.gather)(app);
        let mut activate_of = HashMap::new();
        let nodes: Vec<GraphCanvasNode<Uuid, NodeState>> = model
            .nodes
            .iter()
            .map(|node| {
                if let Some(activate) = &node.activate {
                    activate_of.insert(node.id, activate.clone());
                }
                GraphCanvasNode {
                    id: node.id,
                    kind: node.state,
                    position: node.position,
                    label: node.label.clone(),
                    key: node.key.clone(),
                }
            })
            .collect();
        let edges = model
            .edges
            .iter()
            .map(|&(from, to)| GraphCanvasEdge { from, to })
            .collect();

        // Composed sections shrink the swatch to the top fraction; without
        // them it fills the pane (the Overmap's shape, unchanged).
        let sections: Vec<(&'static str, Vec<crate::sections::SectionRow>)> = self
            .sections
            .iter()
            .map(|p| (p.title, (p.gather)(app)))
            .collect();
        let swatch_h = if sections.is_empty() {
            pane_h
        } else {
            (pane_h * SWATCH_FRACTION).max(64.0)
        };
        let sw = ((pane_w - 2.0 * SWATCH_PAD).max(32.0)) as u32;
        let sh = ((swatch_h - 2.0 * SWATCH_PAD).max(32.0)) as u32;
        self.runner.update(|state| {
            state.swatch.graph = GraphCanvasSubgraph { nodes, edges };
            state.swatch.selected = model.selected;
            state.swatch.width = sw;
            state.swatch.height = sh;
            state.activate_of = activate_of;
            state.viewport_w = pane_w;
            state.viewport_h = pane_h;
            state.sections = sections;
            state.swatch_h = swatch_h;
        });
        self.registry.insert(
            self.preset.leaf_key,
            Box::new(self.runner.state().swatch.paint_leaf(state_color)),
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

    /// Route a pointer MOVE at pane-local `(x, y)`: Enter/Leave transitions;
    /// returns whether the hover target changed (the host redraws on true).
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

    /// The pointer left this pane: deliver the pending Leave so emphasis clears.
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
    pub fn click(&mut self, x: f32, y: f32, w: u32, h: u32) -> Vec<SwatchIntent> {
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

    /// Resolve a probe selector within this pane's DOM (nodes carry their
    /// stable identity as `data-key`).
    pub fn resolve(&self, sel: &genet_probe::Selector, rect: [f32; 4]) -> Option<(f32, f32)> {
        let dom = self.dom.borrow();
        let surfaces = [genet_probe::ProbeSurface {
            name: self.preset.id,
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
    use crate::action::Update;

    fn gloss_pane_on_sample_graph() -> (SwatchPane, App) {
        let mut app = App::test_stub();
        app.canvas = mere::canvas::Canvas::with_sample_graph();
        let mut pane = SwatchPane::new(GLOSS_MINIMAP);
        pane.sync(&app, 480.0, 400.0);
        (pane, app)
    }

    fn overmap_pane_on_fork_pair() -> (SwatchPane, App, crate::panes::SessionId) {
        let mut app = App::test_stub();
        let donor = app.session_id;
        let mut donor_m = session_runtime::GraphSessionManifest::new(
            donor,
            crate::panes::GraphId::from_uuid(uuid::Uuid::from_u128(0xd0)),
        );
        donor_m.display_name = Some("home".to_string());
        app.sessions.insert(donor_m);
        let fork = crate::panes::SessionId::new();
        let mut fork_m = session_runtime::GraphSessionManifest::new(
            fork,
            crate::panes::GraphId::from_uuid(uuid::Uuid::from_u128(0xf0)),
        );
        fork_m.parent_session = Some(donor);
        app.sessions.insert(fork_m);
        app.session_id = fork;
        let mut pane = SwatchPane::new(OVERMAP_LINEAGE);
        pane.sync(&app, 480.0, 400.0);
        (pane, app, donor)
    }

    /// The leaf pipeline end to end, headless, for the minimap preset — the
    /// pipeline every leaf consumer reuses (the old GlossPane's charter).
    #[test]
    fn minimap_preset_paints_through_the_pipeline() {
        let (mut pane, _app) = gloss_pane_on_sample_graph();
        assert!(!pane.runner.state().swatch.graph.nodes.is_empty());
        assert!(
            !pane.runner.state().swatch.graph.edges.is_empty(),
            "edge endpoints must match back to nodes bit-exactly"
        );
        assert!(!pane.runner.state().swatch.show_labels, "minimap stays bare");
        let _scene = pane.scene(480, 400);
        assert!(
            pane.rendered
                .get(GLOSS_MINIMAP.leaf_key)
                .is_some_and(|c| !c.is_empty()),
            "the minimap leaf must render paint commands at its laid-out box"
        );
    }

    /// A minimap node click drains the Open activation for that node's url —
    /// the intent riding the node as preset data.
    #[test]
    fn clicking_a_minimap_node_activates_open() {
        let (mut pane, _app) = gloss_pane_on_sample_graph();
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
            matches!(&intents[..], [SwatchIntent::Activate(SwatchActivate::Open(url))] if *url == key),
            "a node click drains Open for that node's url, got {intents:?}"
        );
    }

    /// The overmap preset renders both sessions + the lineage edge, labels
    /// on, and a session-node click drains the Switch activation.
    #[test]
    fn overmap_preset_renders_lineage_and_switches() {
        let (mut pane, _app, donor) = overmap_pane_on_fork_pair();
        assert_eq!(pane.runner.state().swatch.graph.nodes.len(), 2);
        assert_eq!(pane.runner.state().swatch.graph.edges.len(), 1);
        assert!(pane.runner.state().swatch.show_labels, "identity labels on");
        assert_eq!(
            pane.runner.state().swatch.selected,
            Some(uuid::Uuid::from_u128(0xf0)),
            "the current session's container is selected"
        );
        let _scene = pane.scene(480, 400);
        assert!(
            pane.rendered
                .get(OVERMAP_LINEAGE.leaf_key)
                .is_some_and(|c| !c.is_empty())
        );
        let (x, y) = pane
            .resolve(
                &genet_probe::Selector::class("graph-canvas-swatch-node")
                    .with_attr("data-key", &donor.0.to_string()),
                [0.0, 0.0, 480.0, 400.0],
            )
            .expect("the donor session node resolves by its data-key");
        let intents = pane.click(x, y, 480, 400);
        assert!(
            matches!(&intents[..], [SwatchIntent::Activate(SwatchActivate::Switch(id))] if *id == donor),
            "a session-node click drains Switch for that session, got {intents:?}"
        );
    }

    /// Hover transitions set and clear the emphasis, preset-agnostically.
    #[test]
    fn hovering_sets_and_clears_emphasis() {
        let (mut pane, _app, donor) = overmap_pane_on_fork_pair();
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
            Some(uuid::Uuid::from_u128(0xd0))
        );
        assert!(!pane.hover(x, y, 480, 400), "same target, no re-dispatch");
        assert!(pane.hover_leave(), "leaving the pane is a change");
        assert_eq!(pane.runner.state().swatch.hovered, None, "emphasis cleared");
    }

    /// A recovered content state colors the minimap node open — the preset
    /// gather reads the same lifecycle the canvas colors by.
    #[test]
    fn minimap_nodes_color_by_content_state() {
        let (mut pane, mut app) = gloss_pane_on_sample_graph();
        let id = app.canvas.graph().nodes().next().map(|(_, n)| n.id).unwrap();
        app.apply_update(Update::ContentSpawned { node: id, facts: None });
        pane.sync(&app, 480.0, 400.0);
        let node = pane
            .runner
            .state()
            .swatch
            .graph
            .nodes
            .iter()
            .find(|n| n.id == id)
            .unwrap();
        assert_eq!(node.kind, NodeState::Open, "live content reads open");
    }
}
