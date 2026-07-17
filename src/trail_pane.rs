//! The Trail pane on cambium's `sectioned_list` — the hand-DOM Trail retired.
//!
//! `trail_view` stays the data half (graph truth -> `TrailRow`s on mere's P8
//! vocabulary); this is the view half: rows regrouped into `ListSection`s, a
//! row activation bubbling `TrailPaneAction` out of the runner (the grid's
//! contract — a Trail row's activation IS a navigation), the shell lowering
//! Navigate through `Action::OpenAddress` and Recover awaiting the deletion
//! log.
//!
//! Row geometry moves from host arithmetic (`pane_rows`' fixed ROW_HEIGHT) to
//! the ask-the-layout rule: rows are normal-flow blocks whose heights the host
//! sheet decides, so `row_center` finds a row's rect from the laid-out DOM,
//! exactly like the Roster's `tab_center`.

use std::cell::RefCell;
use std::rc::Rc;

use cambium::{
    AnyView, DomHandle, GenetAppRunner, GenetCtx, GenetElement, ListRow, ListSection,
    PointerClick, el, sectioned_list,
};
use genet_layout::{IncrementalLayout, ScrollOffsets};
use genet_scripted_dom::{NodeId, ScriptedDom};
use layout_dom_api::LayoutDom;

use crate::app::App;
use crate::trail_view::{RowAction, TrailRow, trail_rows};

/// What a Trail row activation produces, bubbled out of the runner.
pub enum TrailPaneAction {
    /// Navigate to this url (the shell lowers `Action::OpenAddress`).
    Navigate(String),
    /// Recover this removed node (awaits the deletion log, rung 6).
    Recover(String),
}

impl cambium::Action for TrailPaneAction {}

/// The pane's state: the gathered rows regrouped into sections (kept with
/// their affordances, so the activation handler can look its row back up) and
/// the pane size for the backdrop.
struct TrailState {
    /// Section title + that section's rows (with affordances).
    sections: Vec<(String, Vec<TrailRow>)>,
    viewport_w: f32,
    viewport_h: f32,
}

type TrailView = Box<dyn AnyView<TrailState, TrailPaneAction, GenetCtx, GenetElement>>;
type TrailRunner =
    GenetAppRunner<TrailState, fn(&TrailState) -> TrailView, TrailView, TrailPaneAction>;

/// Regroup the flat `trail_rows` (headers inline) into sections.
fn sectioned(rows: Vec<TrailRow>) -> Vec<(String, Vec<TrailRow>)> {
    let mut out: Vec<(String, Vec<TrailRow>)> = Vec::new();
    for row in rows {
        match row.action {
            RowAction::Title => out.push((row.text, Vec::new())),
            _ => match out.last_mut() {
                Some((_, rows)) => rows.push(row),
                // A row before any header: give it an unnamed section rather
                // than dropping it silently.
                None => out.push((String::new(), vec![row])),
            },
        }
    }
    out
}

fn trail_pane_view(state: &TrailState) -> TrailView {
    let sections: Vec<ListSection> = state
        .sections
        .iter()
        .map(|(title, rows)| {
            ListSection::new(
                title.clone(),
                rows.iter()
                    .map(|r| match r.action {
                        RowAction::Muted | RowAction::Title => ListRow::muted(r.text.clone()),
                        RowAction::Navigate(_) => ListRow::plain(r.text.clone()),
                        RowAction::Recover(_) => ListRow::action(r.text.clone()),
                    })
                    .collect(),
            )
        })
        .collect();
    let list = sectioned_list(
        &sections,
        |state: &mut TrailState, si: usize, ri: usize| -> Option<TrailPaneAction> {
            let row = state.sections.get(si)?.1.get(ri)?;
            match &row.action {
                RowAction::Navigate(url) => Some(TrailPaneAction::Navigate(url.clone())),
                RowAction::Recover(id) => Some(TrailPaneAction::Recover(id.clone())),
                _ => None,
            }
        },
    );
    Box::new(
        el::<_, TrailState, TrailPaneAction>("div", list)
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

/// The Trail pane: a retained cambium runner over the trail rows. Held by the
/// shell like the Roster's grid.
pub struct TrailPane {
    dom: DomHandle,
    runner: TrailRunner,
}

impl TrailPane {
    pub fn new() -> Self {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let state = TrailState {
            sections: Vec::new(),
            viewport_w: 0.0,
            viewport_h: 0.0,
        };
        let runner = TrailRunner::new(
            dom.clone(),
            trail_pane_view as fn(&TrailState) -> TrailView,
            state,
        );
        Self { dom, runner }
    }

    /// Refresh from graph truth at the pane's size.
    pub fn sync(&mut self, app: &App, pane_w: f32, pane_h: f32) {
        let sections = sectioned(trail_rows(app));
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

    /// Route a click at pane-local `(x, y)`: hit-test the laid-out DOM and
    /// dispatch; the row's action bubbles back. The same round trip as the
    /// Roster's grid.
    pub fn click(&mut self, x: f32, y: f32, w: u32, h: u32) -> Vec<TrailPaneAction> {
        let hit = {
            let dom = self.dom.borrow();
            let layout =
                IncrementalLayout::new(&*dom, &[crate::ui::CAMBIUM_SHEET], w as f32, h as f32);
            let scroll = ScrollOffsets::<NodeId>::default();
            layout.hit_test(&*dom, x, y, &scroll)
        };
        let actions = match hit {
            Some(node) => self.runner.dispatch_click(node, PointerClick::at((x, y))),
            None => Vec::new(),
        };
        tracing::debug!(
            x,
            y,
            hit = ?hit.map(|n| n.raw()),
            actions = actions.len(),
            "trail pane click"
        );
        actions
    }

    /// The pane-local centre of the first row whose text contains `substr`,
    /// from the layout — rows are flow-laid-out under the host sheet, so only
    /// the layout knows their heights (the ask-the-layout rule; this retires
    /// `pane_rows`' fixed-height arithmetic for Trail).
    pub fn row_center(&self, substr: &str, w: u32, h: u32) -> Option<(f32, f32)> {
        let dom = self.dom.borrow();
        let layout =
            IncrementalLayout::new(&*dom, &[crate::ui::CAMBIUM_SHEET], w as f32, h as f32);
        let row = dom
            .all_with_class(dom.document(), "list-row")
            .into_iter()
            .find(|&n| {
                dom.dom_children(n)
                    .any(|c| dom.text(c).is_some_and(|t| t.contains(substr)))
            })?;
        let (x, y, rw, rh) = layout.absolute_rect(&*dom, row)?;
        Some((x + rw / 2.0, y + rh / 2.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane_with_rows() -> TrailPane {
        let mut pane = TrailPane::new();
        pane.runner.update(|state| {
            state.sections = vec![
                (
                    "Recent".to_string(),
                    vec![
                        TrailRow {
                            text: "example.com/".into(),
                            action: RowAction::Navigate("https://example.com/".into()),
                        },
                        TrailRow {
                            text: "no more".into(),
                            action: RowAction::Muted,
                        },
                    ],
                ),
                (
                    "Removed".to_string(),
                    vec![TrailRow {
                        text: "Recover beta".into(),
                        action: RowAction::Recover("beta-id".into()),
                    }],
                ),
            ];
            state.viewport_w = 400.0;
            state.viewport_h = 600.0;
        });
        pane
    }

    /// The migrated round trip: a click at a row's laid-out centre bubbles that
    /// row's own affordance — Navigate carries the url the row was built from.
    #[test]
    fn clicking_a_row_bubbles_its_affordance() {
        let mut pane = pane_with_rows();
        let (x, y) = pane
            .row_center("example.com", 400, 600)
            .expect("the Recent row must be drawn");
        let actions = pane.click(x, y, 400, 600);
        assert!(
            matches!(&actions[..], [TrailPaneAction::Navigate(url)] if url == "https://example.com/"),
            "the Recent row must bubble its url"
        );
        let (x, y) = pane
            .row_center("Recover beta", 400, 600)
            .expect("the Recover row must be drawn");
        let actions = pane.click(x, y, 400, 600);
        assert!(
            matches!(&actions[..], [TrailPaneAction::Recover(id)] if id == "beta-id"),
            "the Recover row must bubble its node id"
        );
    }

    /// A muted row is drawn but inert: no centre is reported for activation
    /// purposes... it IS findable (it's a list-row class) — but clicking it
    /// bubbles nothing.
    #[test]
    fn a_muted_row_is_inert() {
        let mut pane = pane_with_rows();
        let (x, y) = pane
            .row_center("no more", 400, 600)
            .expect("the muted row is drawn");
        assert!(
            pane.click(x, y, 400, 600).is_empty(),
            "a muted row click must bubble nothing"
        );
    }
}
