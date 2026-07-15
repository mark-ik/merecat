//! The cambium seam: a cambium view rendered into a merecat pane surface. Rung 5
//! slice D, the toolkit adoption.
//!
//! cambium (the xilem fork) builds a `ScriptedDom` — the exact type merecat's
//! chrome already lays out through genet-layout into a paint list — so a cambium
//! view drops into the compose-at-surface-rect seam slice A built. A
//! `GenetAppRunner` holds the view's state and renders it into a `DomHandle`;
//! merecat takes that DOM and composites it, and (next) feeds pointer/keyboard
//! events into the runner, which returns Actions merecat lowers through its spine.
//!
//! First view: the Roster as a `data_grid` — the honest form of the data pane
//! (virtualized rows, a sticky header, ARIA grid roles), replacing the hand-DOM
//! list. Merecat is cambium's first pixel host, so this render path (runner DOM
//! -> `scene_from_dom` under the cambium sheet) is new; custom-paint leaves (the
//! Gloss swatch) inject through the leaf registry, a follow-on.

use std::cell::RefCell;
use std::rc::Rc;

use cambium::{
    AnyView, DomHandle, GenetAppRunner, GenetCtx, GenetElement, GridColumn, GridSpec, data_grid, el,
};
use genet_scripted_dom::ScriptedDom;

use crate::app::App;
use crate::roster_view::{RosterGridRow, roster_grid_rows};

/// The Roster grid's state: the flat node rows and the pane height the grid
/// virtualizes against.
struct RosterState {
    rows: Vec<RosterGridRow>,
    viewport_h: f32,
}

/// What a Roster grid interaction produces. `Navigate` carries a node's url; the
/// shell lowers it as `Action::OpenAddress`.
#[allow(dead_code)]
enum RosterAction {
    Navigate(String),
}

type RosterView = Box<dyn AnyView<RosterState, RosterAction, GenetCtx, GenetElement>>;
type RosterRunner = GenetAppRunner<RosterState, fn(&RosterState) -> RosterView, RosterView, RosterAction>;

/// Two columns: the node's display name and its address.
fn roster_spec() -> GridSpec {
    GridSpec {
        columns: vec![GridColumn::new("Node", 240.0), GridColumn::new("Address", 360.0)],
        row_height: 26.0,
        header_height: 28.0,
        overscan: 4,
    }
}

/// Build the Roster grid view from state. The cell closure owns a copy of the
/// rows (cambium's runner retains the view, so a cell cannot borrow `state`).
fn roster_grid(state: &RosterState) -> RosterView {
    let cell_rows = state.rows.clone();
    let class_rows = state.rows.clone();
    data_grid(
        &roster_spec(),
        state.rows.len(),
        state.viewport_h,
        0.0,
        move |row, col| {
            let text = match cell_rows.get(row) {
                Some(r) if col == 0 => r.title.clone(),
                Some(r) => r.url.clone(),
                None => String::new(),
            };
            Box::new(el::<_, RosterState, RosterAction>("span", text)) as RosterView
        },
        // Sort-by-column is caller state; a follow-on (this render proof doesn't
        // reorder). Header clicks are inert for now.
        |_state: &mut RosterState, _col: usize| {},
        move |row| class_rows.get(row).filter(|r| r.selected).map(|_| "grid-row-sel".to_string()),
    )
}

/// Render the Roster as a cambium `data_grid` at `w`x`h`, off the app's graph.
/// Stateless per call for this render proof: a fresh runner builds the grid DOM
/// from the current rows, and merecat composites it. Scroll persistence and event
/// dispatch (the held-runner form) are the next step.
pub fn roster_grid_scene(app: &App, w: u32, h: u32) -> netrender::Scene {
    let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
    let state = RosterState {
        rows: roster_grid_rows(app),
        viewport_h: h as f32,
    };
    // `new` renders the initial surface into the DOM.
    let _runner = RosterRunner::new(dom.clone(), roster_grid as fn(&RosterState) -> RosterView, state);
    let dom_ref = dom.borrow();
    crate::ui::scene_from_dom(&dom_ref, crate::ui::CAMBIUM_SHEET, w, h)
}
