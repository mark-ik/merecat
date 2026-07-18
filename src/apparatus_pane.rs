//! The Apparatus pane: the graph-object facet analyzer (taxonomy revised at
//! the 2026-07-18 harvest) — the selected OBJECT's metadata, analyzed and
//! edited, retargeting with selection (never settings-as-nodes, the ruled-out
//! design). Its first editable rows are the object-scoped handling controls:
//! the viewer override below, which is also the deletion matrix's "change
//! engine/viewer settings and see them apply" row. Facet/classification
//! analysis (chartulary's atomic facets) is the pane's growth direction;
//! app-level settings are a DIFFERENT, later pane.
//!
//! The viewer control is cambium's `radio_group` over the registered engine
//! lanes (Auto / genet.web / genet.livery). A pick lowers
//! `Action::SetViewerOverride` through the spine: the override lands in the
//! browser-state sidecar (persisted at `browser_nodes.json`), and a node with
//! live content RESPAWNS through the routing policy's `pinned_engine`, so the
//! change is visibly applied — a different engine renders the page — not a
//! stored preference pretending to be one. The Inspector's Viewer/Engine rows
//! read the same truth back.
//!
//! The radio's selection mirrors the sidecar at sync; a click moves the
//! widget's own state and the shell lowers the diff — the same mirror-out
//! pattern the Roster's tab strip and the Workbench's strips use.

use std::cell::RefCell;
use std::rc::Rc;

use cambium::{
    AnyView, DomHandle, GenetAppRunner, GenetCtx, GenetElement, PointerClick, RadioGroup, el, lens,
    radio_group,
};
use genet_layout::{IncrementalLayout, ScrollOffsets};
use genet_scripted_dom::{NodeId, ScriptedDom};
use layout_dom_api::LayoutDom;

use crate::app::App;

/// The selectable viewer lanes, in radio order. `Auto` clears the override
/// (the routing policy decides); the named lanes pin an engine id.
pub const VIEWER_OPTIONS: [&str; 3] = ["Auto", "genet.web", "genet.livery"];

/// The viewer override a radio index maps to.
pub fn viewer_for_index(index: usize) -> Option<String> {
    match index {
        0 => None,
        i => VIEWER_OPTIONS.get(i).map(|s| s.to_string()),
    }
}

/// The radio index a sidecar override maps to (unknown overrides show Auto).
fn index_for_viewer(viewer: Option<&str>) -> usize {
    match viewer {
        Some(v) => VIEWER_OPTIONS.iter().position(|o| *o == v).unwrap_or(0),
        None => 0,
    }
}

/// What an Apparatus interaction produces: set the focused node's viewer
/// override (None = back to automatic routing).
pub enum ApparatusIntent {
    SetViewer(Option<String>),
}

struct ApparatusState {
    /// The focused node's caption, for the pane header ("Viewer — <node>").
    target: Option<String>,
    radio: RadioGroup,
    /// The synced sidecar index, for the click diff.
    synced: usize,
    viewport_w: f32,
    viewport_h: f32,
}

type ApparatusView = Box<dyn AnyView<ApparatusState, (), GenetCtx, GenetElement>>;
type ApparatusRunner =
    GenetAppRunner<ApparatusState, fn(&ApparatusState) -> ApparatusView, ApparatusView, ()>;

fn apparatus_view(state: &ApparatusState) -> ApparatusView {
    let header = el::<_, ApparatusState, ()>(
        "div",
        match &state.target {
            Some(caption) => format!("Viewer — {caption}"),
            None => "Viewer — no node focused".to_string(),
        },
    )
    .attr("class", "list-section-title");
    let radio = lens(
        |radio: &mut RadioGroup| radio_group(radio, &VIEWER_OPTIONS),
        |state: &mut ApparatusState| &mut state.radio,
    );
    // The honest footnote: what a pick DOES (respawn through the pinned
    // route), so the control describes its own effect.
    let note = el::<_, ApparatusState, ()>(
        "div",
        "applies on live content by respawning through the pinned engine",
    )
    .attr("class", "list-row muted");
    Box::new(
        el::<_, ApparatusState, ()>("div", (header, radio, note))
            .attr("class", "pane")
            .attr(
                "style",
                format!(
                    "width: {}px; height: {}px;",
                    state.viewport_w, state.viewport_h
                ),
            ),
    )
}

/// The Apparatus pane: a retained cambium runner over the focused node's
/// viewer setting. Held by the shell like the other panes.
pub struct ApparatusPane {
    dom: DomHandle,
    runner: ApparatusRunner,
}

impl ApparatusPane {
    pub fn new() -> Self {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let state = ApparatusState {
            target: None,
            radio: RadioGroup::new(0).with_label("Viewer"),
            synced: 0,
            viewport_w: 0.0,
            viewport_h: 0.0,
        };
        let runner = ApparatusRunner::new(
            dom.clone(),
            apparatus_view as fn(&ApparatusState) -> ApparatusView,
            state,
        );
        Self { dom, runner }
    }

    /// Refresh from app truth at the pane's size: the focused node's caption
    /// and its sidecar viewer override mirrored into the radio.
    pub fn sync(&mut self, app: &App, pane_w: f32, pane_h: f32) {
        let target = crate::app::focused_caption(&app.canvas);
        let synced = app
            .canvas
            .focused_member()
            .and_then(|m| app.browser.get(m))
            .map(|b| index_for_viewer(b.viewer_override.as_deref()))
            .unwrap_or(0);
        self.runner.update(|state| {
            state.target = target;
            state.radio.selected = synced;
            state.synced = synced;
            state.viewport_w = pane_w;
            state.viewport_h = pane_h;
        });
    }

    /// The pane's scene at its size, under the host's cambium sheet.
    pub fn scene(&self, w: u32, h: u32) -> netrender::Scene {
        crate::ui::scene_from_dom(&self.dom.borrow(), crate::ui::CAMBIUM_SHEET, w, h)
    }

    /// The retained DOM, for the shared probe harness.
    pub fn dom_ref(&self) -> std::cell::Ref<'_, ScriptedDom> {
        self.dom.borrow()
    }

    /// Route a click at pane-local `(x, y)` into the radio; report a moved
    /// selection as the intent the shell lowers.
    pub fn click(&mut self, x: f32, y: f32, w: u32, h: u32) -> Vec<ApparatusIntent> {
        let hit = {
            let dom = self.dom.borrow();
            let layout =
                IncrementalLayout::new(&*dom, &[crate::ui::CAMBIUM_SHEET], w as f32, h as f32);
            let scroll = ScrollOffsets::<NodeId>::default();
            layout.hit_test(&*dom, x, y, &scroll)
        };
        if let Some(node) = hit {
            let _: Vec<()> = self.runner.dispatch_click(node, PointerClick::at((x, y)));
        }
        let state = self.runner.state();
        if state.radio.selected != state.synced {
            vec![ApparatusIntent::SetViewer(viewer_for_index(
                state.radio.selected,
            ))]
        } else {
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::Action;

    /// A click on a radio option reports the viewer intent — and the sidecar
    /// mirror round-trips (sync shows what the app persisted).
    #[test]
    fn picking_a_viewer_reports_the_intent_and_mirrors_back() {
        let mut app = App::test_stub();
        app.update(Action::OpenAddress("https://example.com/x".to_string()));
        let mut pane = ApparatusPane::new();
        pane.sync(&app, 400.0, 600.0);
        // Resolve the livery option's centre off the laid-out DOM.
        let (x, y) = {
            let dom = pane.dom.borrow();
            let layout =
                IncrementalLayout::new(&*dom, &[crate::ui::CAMBIUM_SHEET], 400.0, 600.0);
            let radio = dom
                .all_with_class(dom.document(), "radio")
                .into_iter()
                .find(|&n| {
                    dom.dom_children(n)
                        .any(|c| dom.text(c).is_some_and(|t| t.contains("livery")))
                })
                .expect("the livery option is drawn");
            let (rx, ry, rw, rh) = layout.absolute_rect(&*dom, radio).unwrap();
            (rx + rw / 2.0, ry + rh / 2.0)
        };
        let intents = pane.click(x, y, 400, 600);
        assert!(
            matches!(&intents[..], [ApparatusIntent::SetViewer(Some(v))] if v == "genet.livery"),
            "picking livery reports the pinned viewer"
        );
        // The app applies it; a re-sync mirrors the persisted truth back.
        let member = app.canvas.focused_member().unwrap();
        app.update(Action::SetViewerOverride {
            member,
            viewer: Some("genet.livery".to_string()),
        });
        pane.sync(&app, 400.0, 600.0);
        assert_eq!(pane.runner.state().radio.selected, 2, "the sidecar mirrors back");
        assert!(pane.click(x, y, 400, 600).is_empty(), "re-picking the same is no intent");
    }

    #[test]
    fn index_mapping_round_trips() {
        assert_eq!(viewer_for_index(0), None);
        assert_eq!(viewer_for_index(1).as_deref(), Some("genet.web"));
        assert_eq!(viewer_for_index(2).as_deref(), Some("genet.livery"));
        assert_eq!(index_for_viewer(None), 0);
        assert_eq!(index_for_viewer(Some("genet.livery")), 2);
        assert_eq!(index_for_viewer(Some("unknown.lane")), 0, "unknown shows Auto");
    }
}
