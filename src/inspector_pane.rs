//! The Inspector pane on cambium's `detail_panel` — the catalog entry the
//! surfaces-in-cambium mapping named for it (key/value sections, all inert).
//!
//! `inspector_view` is the data half (app truth -> sections); this is the
//! view half: sections handed to the panel, composited at the pane's rect
//! like every other cambium pane. Purely informational — a press on the pane
//! activates it (the shell's generic pane path) and nothing inside takes a
//! click, which is the panel's own contract.

use std::cell::RefCell;
use std::rc::Rc;

use cambium::{AnyView, DetailRow, DetailSection, DomHandle, GenetCtx, GenetElement, detail_panel, el};
use genet_scripted_dom::ScriptedDom;

use crate::app::App;
use crate::inspector_view::{InspectorSection, inspector_sections};

/// The pane emits no actions; the unit type keeps the runner honest about it.
struct InspectorState {
    sections: Vec<InspectorSection>,
    viewport_w: f32,
    viewport_h: f32,
}

type InspectorView = Box<dyn AnyView<InspectorState, (), GenetCtx, GenetElement>>;
type InspectorRunner = cambium::GenetAppRunner<
    InspectorState,
    fn(&InspectorState) -> InspectorView,
    InspectorView,
    (),
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
    Box::new(
        el::<_, InspectorState, ()>("div", detail_panel(&sections))
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
    pub fn sync(&mut self, app: &App, pane_w: f32, pane_h: f32) {
        let sections = inspector_sections(app);
        self.runner.update(|state| {
            state.sections = sections;
            state.viewport_w = pane_w;
            state.viewport_h = pane_h;
        });
    }

    /// The pane's scene at its size, under the host's cambium sheet.
    pub fn scene(&self, w: u32, h: u32) -> netrender::Scene {
        crate::ui::scene_from_dom(&self.dom.borrow(), crate::ui::CAMBIUM_SHEET, w, h)
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
}
