//! The cambium seam: a cambium view rendered into a merecat pane surface, and
//! the pane's pointer events routed back into it. Rung 5 slice D, the toolkit
//! adoption.
//!
//! cambium builds a `ScriptedDom` — the exact type merecat's chrome already lays
//! out through genet-layout into a paint list — so a cambium view drops into the
//! compose-at-surface-rect seam slice A built. A `GenetAppRunner` holds the view's
//! state and renders it into a `DomHandle`; merecat composites that DOM, and
//! routes a pane-local click back through genet-layout's hit test into
//! `dispatch_click`, which returns the Actions the view emitted. Those lower
//! through merecat's ordinary spine — the same path a keypress takes.
//!
//! That round trip (render -> composite -> hit-test -> dispatch -> Action) is the
//! general pane-event path every cambium pane reuses; the Roster's `data_grid` is
//! its first consumer. Merecat is cambium's first pixel host, so this integration
//! is new: the catalog example is a headless acceptance test.

use std::cell::RefCell;
use std::rc::Rc;

use cambium::{
    AnyView, DomHandle, GenetAppRunner, GenetCtx, GenetElement, GridColumn, GridSpec, PointerClick,
    data_grid, el, on_click,
};
use genet_layout::{IncrementalLayout, ScrollOffsets};
use genet_scripted_dom::{NodeId, ScriptedDom};

use crate::app::App;
use crate::roster_view::{RosterGridRow, roster_grid_rows};

/// The Roster grid's state: the flat node rows and the pane height the grid
/// virtualizes against.
struct RosterState {
    rows: Vec<RosterGridRow>,
    viewport_h: f32,
}

/// What a Roster grid interaction produces. The shell lowers `Navigate` as
/// `Action::OpenAddress`, so a grid click reaches the graph through the same
/// spine as a keypress.
pub enum RosterAction {
    Navigate(String),
}

// Opt `RosterAction` into cambium's bubbling-action marker, so an `on_click`
// handler may return it directly (the blanket `OptionalAction<A>` impl). Without
// the marker only `()` handlers compile — cambium seals this deliberately, so an
// app names the types it bubbles.
impl cambium::Action for RosterAction {}

type RosterView = Box<dyn AnyView<RosterState, RosterAction, GenetCtx, GenetElement>>;
type RosterRunner =
    GenetAppRunner<RosterState, fn(&RosterState) -> RosterView, RosterView, RosterAction>;

/// Two columns: the node's display name and its address.
fn roster_spec() -> GridSpec {
    GridSpec {
        columns: vec![
            GridColumn::new("Node", 240.0),
            GridColumn::new("Address", 360.0),
        ],
        row_height: 26.0,
        header_height: 28.0,
        overscan: 4,
    }
}

/// Build the Roster grid view from state. Cells own their text and their target
/// url (cambium's runner retains the view, so a cell cannot borrow `state`); a
/// click on either column emits `Navigate` for that row.
fn roster_grid(state: &RosterState) -> RosterView {
    let cell_rows = state.rows.clone();
    let class_rows = state.rows.clone();
    data_grid(
        &roster_spec(),
        state.rows.len(),
        state.viewport_h,
        0.0,
        move |row, col| {
            let (text, url) = match cell_rows.get(row) {
                Some(r) if col == 0 => (r.title.clone(), r.url.clone()),
                Some(r) => (r.url.clone(), r.url.clone()),
                None => (String::new(), String::new()),
            };
            Box::new(on_click(
                el::<_, RosterState, RosterAction>("span", text),
                move |_state: &mut RosterState, _click: PointerClick| {
                    RosterAction::Navigate(url.clone())
                },
            )) as RosterView
        },
        // Sort-by-column is caller state; a follow-on. Header clicks are inert.
        |_state: &mut RosterState, _col: usize| {},
        move |row| {
            class_rows
                .get(row)
                .filter(|r| r.selected)
                .map(|_| "grid-row-selected".to_string())
        },
    )
}

/// The pane-local y at the centre of grid row `idx` (below the sticky header).
/// The scenario's `click-row` aims here, so a receipt clicks the row the grid
/// actually drew rather than guessing at a list geometry the grid retired.
pub fn grid_row_center_y(idx: usize) -> f32 {
    let spec = roster_spec();
    spec.header_height + idx as f32 * spec.row_height + spec.row_height / 2.0
}

/// The Roster pane's cambium grid: a retained runner over the node rows. Held by
/// the shell (the runner is `!Send`, like the content sessions) so its state and
/// DOM persist between the frame that draws it and the click that hits it.
pub struct RosterGrid {
    dom: DomHandle,
    runner: RosterRunner,
}

impl RosterGrid {
    pub fn new() -> Self {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let state = RosterState {
            rows: Vec::new(),
            viewport_h: 0.0,
        };
        let runner = RosterRunner::new(
            dom.clone(),
            roster_grid as fn(&RosterState) -> RosterView,
            state,
        );
        Self { dom, runner }
    }

    /// Refresh the grid from graph truth and the pane's height. Rebuilds the
    /// view (and its DOM) through the runner.
    pub fn sync(&mut self, app: &App, viewport_h: f32) {
        let rows = roster_grid_rows(app);
        self.runner.update(|state| {
            state.rows = rows;
            state.viewport_h = viewport_h;
        });
    }

    /// The grid's scene at the pane's size, under the host's cambium sheet.
    pub fn scene(&self, w: u32, h: u32) -> netrender::Scene {
        crate::ui::scene_from_dom(&self.dom.borrow(), crate::ui::CAMBIUM_SHEET, w, h)
    }

    /// Route a click at pane-local `(x, y)` into the view: lay the DOM out at the
    /// pane's size, hit-test the point to a DOM node, and dispatch. Returns the
    /// Actions the view emitted (empty when the point hit nothing interactive).
    pub fn click(&mut self, x: f32, y: f32, w: u32, h: u32) -> Vec<RosterAction> {
        let hit = {
            let dom = self.dom.borrow();
            let layout = IncrementalLayout::new(&*dom, &[crate::ui::CAMBIUM_SHEET], w as f32, h as f32);
            let scroll = ScrollOffsets::<NodeId>::default();
            layout.hit_test(&*dom, x, y, &scroll)
        };
        // The borrow is dropped: dispatch rebuilds the view into the same DOM.
        let actions = match hit {
            Some(node) => self.runner.dispatch_click(node, PointerClick::at((x, y))),
            None => Vec::new(),
        };
        tracing::debug!(
            x,
            y,
            w,
            h,
            hit = ?hit.map(|n| n.raw()),
            rows = self.runner.state().rows.len(),
            viewport_h = self.runner.state().viewport_h,
            actions = actions.len(),
            "roster grid click"
        );
        actions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(title: &str) -> RosterGridRow {
        RosterGridRow {
            title: title.to_string(),
            kind: "text/html".to_string(),
            url: format!("https://{title}/"),
            selected: false,
        }
    }

    /// The pane-event round trip, headless: lay the grid's DOM out at a pane size
    /// and hit-test the points a click would land on. Pins the integration this
    /// seam depends on — a cambium view's DOM must be hit-testable through
    /// genet-layout, or no cambium pane can take a click.
    #[test]
    fn grid_dom_is_hit_testable() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let state = RosterState {
            rows: vec![row("alpha"), row("beta"), row("gamma")],
            viewport_h: 600.0,
        };
        let _runner = RosterRunner::new(
            dom.clone(),
            roster_grid as fn(&RosterState) -> RosterView,
            state,
        );
        let d = dom.borrow();
        let layout = IncrementalLayout::new(&*d, &[crate::ui::CAMBIUM_SHEET], 512.0, 600.0);
        let scroll = ScrollOffsets::<NodeId>::default();
        // Header (y<28), then rows at 28.. in 26px steps.
        for (x, y) in [(20.0, 10.0), (20.0, 40.0), (20.0, 47.0), (20.0, 70.0)] {
            let hit = layout.hit_test(&*d, x, y, &scroll);
            println!("({x}, {y}) -> {:?}", hit.map(|n| n.raw()));
        }
        // A click inside the first row must land on some node.
        assert!(
            layout.hit_test(&*d, 20.0, 40.0, &scroll).is_some(),
            "a point inside the grid's first row must hit a DOM node"
        );
    }
}
