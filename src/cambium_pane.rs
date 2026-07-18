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
    TabStrip, data_grid, el, lens, on_click, tab_strip,
};
use genet_layout::{IncrementalLayout, ScrollOffsets};
use layout_dom_api::LayoutDom;
use genet_scripted_dom::{NodeId, ScriptedDom};

use crate::app::App;
use crate::roster_view::{RosterGridRow, roster_grid_rows};

/// The Roster's tabs — `mere::roster`'s four data views over one grid. Only
/// Nodes has a gatherer today; the rest render an honest empty state until
/// theirs land (Links needs the edge families walked into `build_link_rows`).
const ROSTER_TABS: [&str; 4] = ["Nodes", "Links", "Graphlets", "Fields"];

/// The Roster pane's state: the tab strip, the flat node rows, and the pane
/// size — the height the grid virtualizes against, and both dimensions for the
/// pane's own backdrop.
struct RosterState {
    tabs: TabStrip,
    rows: Vec<RosterGridRow>,
    viewport_w: f32,
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

/// The Roster pane: a tab strip over the active tab's view. The strip is
/// cambium's (consumer-pulled by this pane), lensed onto our `tabs` field; the
/// Nodes tab is the node grid, the others an honest empty state until their
/// gatherers land.
fn roster_pane(state: &RosterState) -> RosterView {
    let strip = lens(
        |tabs: &mut TabStrip| tab_strip::<RosterAction>(tabs, &ROSTER_TABS),
        |state: &mut RosterState| &mut state.tabs,
    );
    let body: RosterView = match state.tabs.selected {
        0 => roster_grid(state),
        _ => Box::new(
            el::<_, RosterState, RosterAction>(
                "div",
                format!(
                    "{} has no gatherer yet",
                    ROSTER_TABS[state.tabs.selected.min(ROSTER_TABS.len() - 1)]
                ),
            )
            .attr("class", "pane-empty"),
        ),
    };
    // The pane paints its own backdrop at the pane's full size. A tab whose body
    // is text-sized (an empty state) covers only a few rows of pixels, and what
    // sits under a pane is not the pane's colour — so without this the bare gap
    // shows through. Geometry inline, per the sheet contract; the colour is the
    // host sheet's `.pane`, the same one the hand-DOM panes wear.
    Box::new(
        el::<_, RosterState, RosterAction>("div", (strip, body))
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

/// The Nodes tab: the graph's node manifest as a grid. Cells own their text and
/// their target url (cambium's runner retains the view, so a cell cannot borrow
/// `state`); a click on either column emits `Navigate` for that row.
fn roster_grid(state: &RosterState) -> RosterView {
    let cell_rows = state.rows.clone();
    let class_rows = state.rows.clone();
    data_grid(
        &roster_spec(),
        state.rows.len(),
        (state.viewport_h - crate::ui::TABLIST_HEIGHT).max(0.0),
        0.0,
        move |row, col| {
            let (text, url) = match cell_rows.get(row) {
                Some(r) if col == 0 => (r.title.clone(), r.url.clone()),
                Some(r) => (r.url.clone(), r.url.clone()),
                None => (String::new(), String::new()),
            };
            // `roster-cell` is the genet-probe hit target: a stable class whose
            // direct child text is the cell's, so `click-row` resolves a grid
            // row the same way it resolves a Trail `list-row`. A block `div`
            // (not an inline `span`) so it has a box `absolute_rect` resolves AND
            // fills the cell width, so the resolved centre lands on the
            // clickable rather than in empty space past short text. No sheet rule
            // — `.grid-cell` already styles it; this class is targeting only.
            Box::new(on_click(
                el::<_, RosterState, RosterAction>("div", text).attr("class", "roster-cell"),
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

/// The pane-local y at the centre of grid row `idx` — below the tab strip and
/// the grid's sticky header. A test helper now that `click-row` resolves off the
/// DOM through genet-probe (`grid_dom_is_hit_testable` uses it to aim a probe
/// click at a known row); kept because it pins the grid's row math to the layout.
pub fn grid_row_center_y(idx: usize) -> f32 {
    let spec = roster_spec();
    crate::ui::TABLIST_HEIGHT + spec.header_height + idx as f32 * spec.row_height
        + spec.row_height / 2.0
}

/// The label of Roster tab `idx`, clamped. The observation rendering of
/// `App::roster_tab`, which is an index because the strip's selection is.
pub fn tab_label(idx: usize) -> &'static str {
    ROSTER_TABS[idx.min(ROSTER_TABS.len() - 1)]
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
            tabs: TabStrip::new(0).with_label("Roster"),
            rows: Vec::new(),
            viewport_w: 0.0,
            viewport_h: 0.0,
        };
        let runner = RosterRunner::new(
            dom.clone(),
            roster_pane as fn(&RosterState) -> RosterView,
            state,
        );
        Self { dom, runner }
    }

    /// Refresh the grid from graph truth and the pane's size. Rebuilds the view
    /// (and its DOM) through the runner. The size is the whole pane; the grid
    /// virtualizes against what the tab strip leaves it.
    pub fn sync(&mut self, app: &App, pane_w: f32, pane_h: f32) {
        let rows = roster_grid_rows(app);
        self.runner.update(|state| {
            state.rows = rows;
            state.viewport_w = pane_w;
            state.viewport_h = pane_h;
        });
    }

    /// Which tab is active, and its label.
    pub fn selected_tab(&self) -> (usize, &'static str) {
        let i = self.runner.state().tabs.selected;
        (i, ROSTER_TABS[i.min(ROSTER_TABS.len() - 1)])
    }

    /// Resolve a selector to a point within this pane's DOM at window rect
    /// `rect` (`[x, y, w, h]`), via the shared genet-probe resolver — the strip's
    /// bespoke `tab_center` collapsed onto the generic path. Returns the
    /// window-space centre of the first match, or `None` (not drawn). This is
    /// the "extraction simplifies the consumer" claim in the small: the pane no
    /// longer owns tab geometry, it forwards its DOM to the shared resolver.
    pub fn resolve(&self, sel: &genet_probe::Selector, rect: [f32; 4]) -> Option<(f32, f32)> {
        let dom = self.dom.borrow();
        let surfaces = [genet_probe::ProbeSurface {
            name: "roster",
            dom: &dom,
            rect,
            sheet: crate::ui::CAMBIUM_SHEET,
        }];
        genet_probe::resolve(&surfaces, sel).map(|h| h.point)
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

    fn grid_with_rows() -> RosterGrid {
        let mut g = RosterGrid::new();
        g.runner.update(|state| {
            state.rows = vec![row("alpha"), row("beta"), row("gamma")];
            state.viewport_w = 512.0;
            state.viewport_h = 600.0;
        });
        g
    }

    /// The pane-event round trip, headless: lay the grid's DOM out at a pane size
    /// and hit-test the points a click would land on. Pins the integration this
    /// seam depends on — a cambium view's DOM must be hit-testable through
    /// genet-layout, or no cambium pane can take a click.
    #[test]
    fn grid_dom_is_hit_testable() {
        let g = grid_with_rows();
        let d = g.dom.borrow();
        let layout = IncrementalLayout::new(&*d, &[crate::ui::CAMBIUM_SHEET], 512.0, 600.0);
        let scroll = ScrollOffsets::<NodeId>::default();
        assert!(
            layout
                .hit_test(&*d, 20.0, grid_row_center_y(0), &scroll)
                .is_some(),
            "a point inside the grid's first row must hit a DOM node"
        );
    }

    /// Isolates the click-row path: a `roster-cell` must resolve through
    /// genet-probe to a point, the way the shell's `click-row` drives it.
    #[test]
    fn a_roster_cell_resolves_by_text() {
        let g = grid_with_rows();
        let hit = g.resolve(
            &genet_probe::Selector::class("roster-cell").containing("alpha"),
            [0.0, 0.0, 512.0, 600.0],
        );
        // Diagnostic: how many roster-cell elements exist at all.
        let d = g.dom.borrow();
        let cells = d.all_with_class(d.document(), "roster-cell");
        assert!(
            hit.is_some(),
            "roster-cell 'alpha' must resolve; {} roster-cell elements in the DOM",
            cells.len()
        );
    }

    /// `TABLIST_HEIGHT` is a host-side constant standing in for what the host
    /// sheet lays the strip out at — the Roster subtracts it from the grid's
    /// viewport and adds it to a row's y, so if the sheet and the constant drift
    /// apart every click lands on the wrong row. Hold them together.
    #[test]
    fn tablist_height_matches_the_sheet() {
        let g = grid_with_rows();
        let d = g.dom.borrow();
        let layout = IncrementalLayout::new(&*d, &[crate::ui::CAMBIUM_SHEET], 512.0, 600.0);
        let scroll = ScrollOffsets::<NodeId>::default();
        // The strip occupies its declared height: a point just inside its bottom
        // edge is still a tab, and one just below it is not.
        let inside = layout.hit_test(&*d, 20.0, crate::ui::TABLIST_HEIGHT - 2.0, &scroll);
        let below = layout.hit_test(&*d, 20.0, crate::ui::TABLIST_HEIGHT + 2.0, &scroll);
        assert!(inside.is_some(), "the strip must fill its declared height");
        assert_ne!(
            inside, below,
            "the strip must END at its declared height: {}px hits the same node as \
             the row below it, so the sheet and TABLIST_HEIGHT have drifted",
            crate::ui::TABLIST_HEIGHT
        );
    }

    /// The tabs are live end-to-end: a click at a tab's centre reaches the
    /// strip's handler and switches the pane's body. This is the whole point of
    /// the strip — that cambium's catalog widget takes a real pane click through
    /// merecat's hit-test path, not just that it renders.
    #[test]
    fn clicking_a_tab_switches_the_body() {
        let mut g = grid_with_rows();
        assert_eq!(g.selected_tab(), (0, "Nodes"));
        let (x, y) = g
            .resolve(
                &genet_probe::Selector::class("tab").containing("Links"),
                [0.0, 0.0, 512.0, 600.0],
            )
            .expect("the strip must draw a Links tab");
        g.click(x, y, 512, 600);
        assert_eq!(
            g.selected_tab(),
            (1, "Links"),
            "clicking the Links tab must select it"
        );
        // The body followed the tab: the grid is gone, replaced by the empty
        // state. (The pane's backdrop still covers those pixels — that is the
        // point of it — so ask the DOM what is there, not whether anything is.)
        let d = g.dom.borrow();
        assert!(
            d.all_with_class(d.document(), "grid").is_empty(),
            "the Links tab has no gatherer, so no grid may remain under it"
        );
        assert!(
            !d.all_with_class(d.document(), "pane-empty").is_empty(),
            "a gatherer-less tab must say so rather than draw an empty grid"
        );
    }

    /// The shared resolver must agree with the strip the sheet lays out: every
    /// declared tab is drawn, in order, inside the strip's height. The scenario's
    /// `click-tab` aims through the same resolver, so a drift means receipts click
    /// the wrong tab.
    #[test]
    fn every_tab_is_drawn_in_the_strip() {
        let g = grid_with_rows();
        let mut last_x = 0.0;
        for label in ROSTER_TABS {
            let (x, y) = g
                .resolve(
                    &genet_probe::Selector::class("tab").containing(label),
                    [0.0, 0.0, 512.0, 600.0],
                )
                .unwrap_or_else(|| panic!("the strip must draw a {label} tab"));
            assert!(x > last_x, "tabs must run left to right: {label} at {x}");
            assert!(
                y > 0.0 && y < crate::ui::TABLIST_HEIGHT,
                "{label}'s centre ({y}) must sit inside the strip"
            );
            last_x = x;
        }
    }
}
