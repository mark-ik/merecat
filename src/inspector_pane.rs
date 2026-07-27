//! The Inspector pane on cambium's `detail_panel` — the catalog entry the
//! surfaces-in-cambium mapping named for it (key/value sections plus declared
//! actions).
//!
//! `inspector_view` is the data half (app truth -> sections); this is the
//! view half: sections handed to the panel, composited at the pane's rect
//! like every other cambium pane. Purely informational — a press on the pane
//! activates it (the shell's generic pane path); the Knot clip button lowers a
//! typed intent through the configured endpoint handle.

use std::cell::RefCell;
use std::rc::Rc;

use cambium::{
    AnyView, DetailRow, DetailSection, DomHandle, GenetCtx, GenetElement, PointerClick, button,
    detail_panel, el,
};
use genet_layout::{IncrementalLayout, ScrollOffsets};
use genet_scripted_dom::{NodeId, ScriptedDom};

use crate::app::App;
use crate::inspector_view::{InspectorSection, inspector_sections};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InspectorIntent {
    ClipToKnot,
}

impl cambium::Action for InspectorIntent {}

struct InspectorState {
    sections: Vec<InspectorSection>,
    clip_target: Option<String>,
    clip_source_available: bool,
    clip_status: String,
    viewport_w: f32,
    viewport_h: f32,
}

type InspectorView = Box<dyn AnyView<InspectorState, InspectorIntent, GenetCtx, GenetElement>>;
type InspectorRunner = cambium::GenetAppRunner<
    InspectorState,
    fn(&InspectorState) -> InspectorView,
    InspectorView,
    InspectorIntent,
>;

fn inspector_pane_view(state: &InspectorState) -> InspectorView {
    let sections: Vec<DetailSection> = state
        .sections
        .iter()
        .map(|s| {
            DetailSection::new(
                s.title.clone(),
                s.rows
                    .iter()
                    .map(|(k, v)| DetailRow::new(k.clone(), v.clone()))
                    .collect(),
            )
        })
        .collect();
    let clip_label = match (&state.clip_target, state.clip_source_available) {
        (Some(target), true) => format!("Clip document to {target}"),
        (None, _) => "Set TURNSTONE_KNOT_CLIP_TARGET to enable clips".into(),
        (Some(_), false) => "Focused document cannot supply a clip".into(),
    };
    let clip_status = el::<_, InspectorState, InspectorIntent>(
        "div",
        format!("Knot clip: {}", state.clip_status),
    )
    .attr("class", "detail-value");
    let clip_button = button(
        clip_label,
        |state: &mut InspectorState, _click: PointerClick| {
            (state.clip_target.is_some() && state.clip_source_available)
                .then_some(InspectorIntent::ClipToKnot)
        },
    );
    Box::new(
        el::<_, InspectorState, InspectorIntent>(
            "div",
            (detail_panel(&sections), clip_status, clip_button),
        )
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

/// The Inspector pane: a retained cambium runner over the detail sections.
/// Held by the shell like the other panes.
pub struct InspectorPane {
    dom: DomHandle,
    runner: InspectorRunner,
}

impl InspectorPane {
    pub fn new() -> Self {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let state = InspectorState {
            sections: Vec::new(),
            clip_target: None,
            clip_source_available: false,
            clip_status: "unconfigured".into(),
            viewport_w: 0.0,
            viewport_h: 0.0,
        };
        let runner = InspectorRunner::new(
            dom.clone(),
            inspector_pane_view as fn(&InspectorState) -> InspectorView,
            state,
        );
        Self { dom, runner }
    }

    /// Refresh from app truth at the pane's size.
    pub fn sync(
        &mut self,
        app: &App,
        pane_w: f32,
        pane_h: f32,
        clip_target: Option<&str>,
        clip_source_available: bool,
        clip_status: &str,
    ) {
        let sections = inspector_sections(app);
        self.runner.update(|state| {
            state.sections = sections;
            state.clip_target = clip_target.map(str::to_string);
            state.clip_source_available = clip_source_available;
            state.clip_status = clip_status.to_string();
            state.viewport_w = pane_w;
            state.viewport_h = pane_h;
        });
    }

    /// The pane's scene at its size, under the host's cambium sheet.
    pub fn scene(&self, w: u32, h: u32) -> netrender::Scene {
        crate::ui::scene_from_dom(&self.dom.borrow(), crate::ui::CAMBIUM_SHEET, w, h)
    }

    pub fn click(&mut self, x: f32, y: f32, w: u32, h: u32) -> Vec<InspectorIntent> {
        let hit = {
            let dom = self.dom.borrow();
            let layout =
                IncrementalLayout::new(&*dom, &[crate::ui::CAMBIUM_SHEET], w as f32, h as f32);
            layout.hit_test(&*dom, x, y, &ScrollOffsets::<NodeId>::default())
        };
        hit.map(|node| self.runner.dispatch_click(node, PointerClick::at((x, y))))
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use layout_dom_api::LayoutDom;

    /// The pane draws the sections it was synced with: headers and key/value
    /// rows land in the DOM under the panel's classes.
    #[test]
    fn synced_sections_reach_the_dom() {
        let mut pane = InspectorPane::new();
        pane.runner.update(|state| {
            state.sections = vec![InspectorSection {
                title: "Node".to_string(),
                rows: vec![("URL".to_string(), "https://example.test/".to_string())],
            }];
            state.clip_target = Some("file:///notes/field.knot".into());
            state.clip_source_available = true;
            state.clip_status = "ready".into();
            state.viewport_w = 400.0;
            state.viewport_h = 600.0;
        });
        let dom = pane.dom.borrow();
        let rows = dom.all_with_class(dom.document(), "detail-row");
        assert_eq!(rows.len(), 1);
        let values = dom.all_with_class(dom.document(), "detail-value");
        let text: String = values
            .iter()
            .flat_map(|&n| dom.dom_children(n))
            .filter_map(|c| dom.text(c).map(str::to_string))
            .collect();
        assert_eq!(text, "https://example.test/");
    }

    #[test]
    fn clip_button_bubbles_a_typed_inspector_intent() {
        let mut pane = InspectorPane::new();
        pane.runner.update(|state| {
            state.clip_target = Some("file:///notes/field.knot".into());
            state.clip_source_available = true;
            state.viewport_w = 400.0;
            state.viewport_h = 600.0;
        });
        let (x, y) = {
            let dom = pane.dom.borrow();
            let layout = IncrementalLayout::new(&*dom, &[crate::ui::CAMBIUM_SHEET], 400.0, 600.0);
            let button = dom
                .dom_children(pane.runner.root())
                .find(|&node| {
                    dom.element_name(node)
                        .is_some_and(|name| name.local.as_ref() == "button")
                })
                .expect("the inspector action is a real button");
            let (x, y, w, h) = layout.absolute_rect(&*dom, button).unwrap();
            (x + w / 2.0, y + h / 2.0)
        };
        assert_eq!(
            pane.click(x, y, 400, 600),
            vec![InspectorIntent::ClipToKnot]
        );
    }
}
