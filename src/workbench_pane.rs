//! The Workbench pane's view half (rung 5 slice E, the cambium path): each
//! cell of the walked tiling drawn as a placed box wearing cambium's
//! `tab_strip`, with a body hint underneath. The tile CONTENT does not render
//! here — a live tile's document composites as its own surface at the cell's
//! body rect (the shell's surface plan), exactly like the rung-4 content
//! inset; this DOM is the workbench's furniture (tab bars, cell backdrops,
//! and the honest hint when a tile has no live content to show).
//!
//! `workbench_tiling` is the geometry half (platen's `TreeGeometry` walked
//! into rects); this holds the retained cambium runner over it. Tab clicks
//! land in each cell's `TabStrip` (the strip owns its selection); the shell
//! reads the diff back as `WorkbenchActivate` — the same mirror-out pattern
//! the Roster's strip uses, generalized to N strips.

use std::cell::RefCell;
use std::rc::Rc;

use cambium::{
    AnyView, DomHandle, GenetAppRunner, GenetCtx, GenetElement, PointerClick, TabStrip,
    arrangement, el, lens, placed_with, tab_strip,
};
use sprigging::Placement;
use genet_layout::{IncrementalLayout, ScrollOffsets};
use genet_scripted_dom::{NodeId, ScriptedDom};
use layout_dom_api::LayoutDom;
use uuid::Uuid;

use crate::app::App;
use crate::content::NodeContent;
use crate::surface::Rect;
use crate::workbench_tiling::{WorkbenchTiling, place_workbench};

/// One cell as the view holds it: the walked placement's members and rect,
/// the tab labels, the strip state (selection mirrors the model's active
/// tab at sync; a click moves it and the shell lowers the diff), and the
/// body hint.
struct CellView {
    members: Vec<Uuid>,
    rect: Rect,
    labels: Vec<String>,
    strip: TabStrip,
    /// The synced model-active index, for the click diff.
    synced_active: usize,
    /// The body hint ("content off", "failed: ..."), empty when live content
    /// composites over the body.
    hint: String,
}

struct WorkbenchState {
    cells: Vec<CellView>,
    viewport_w: f32,
    viewport_h: f32,
}

type WbView = Box<dyn AnyView<WorkbenchState, (), GenetCtx, GenetElement>>;
type WbRunner =
    GenetAppRunner<WorkbenchState, fn(&WorkbenchState) -> WbView, WbView, ()>;

fn workbench_view(state: &WorkbenchState) -> WbView {
    let mut children: Vec<WbView> = Vec::new();
    for (i, cell) in state.cells.iter().enumerate() {
        let labels = cell.labels.clone();
        let strip = lens(
            move |tabs: &mut TabStrip| {
                let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
                tab_strip::<()>(tabs, &refs)
            },
            move |state: &mut WorkbenchState| &mut state.cells[i].strip,
        );
        let body_h = (cell.rect.h - crate::ui::TABLIST_HEIGHT).max(0.0);
        let body = el::<_, WorkbenchState, ()>("div", cell.hint.clone())
            .attr("class", "wb-body")
            .attr("style", format!("height: {body_h}px;"));
        children.push(Box::new(placed_with(
            Placement::new(cell.rect.x, cell.rect.y),
            format!(
                "width: {}px; height: {}px;",
                cell.rect.w, cell.rect.h
            ),
            (strip, body),
        )));
    }
    Box::new(
        el::<_, WorkbenchState, ()>(
            "div",
            arrangement(state.viewport_w, state.viewport_h, children),
        )
        .attr("class", "pane"),
    )
}

/// An activation the pane reports back after a click: make this member's tab
/// the active one (the shell lowers `Action::WorkbenchActivate`).
pub struct WbActivate(pub Uuid);

/// The Workbench pane: a retained cambium runner over the walked tiling.
/// Held by the shell like the other panes.
pub struct WorkbenchPane {
    dom: DomHandle,
    runner: WbRunner,
    /// The last synced walk, pane-local — the shell's input routing asks it
    /// for cells and divider bands (the same walk the frame drew).
    tiling: WorkbenchTiling,
}

impl WorkbenchPane {
    pub fn new() -> Self {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let state = WorkbenchState {
            cells: Vec::new(),
            viewport_w: 0.0,
            viewport_h: 0.0,
        };
        let runner = WbRunner::new(
            dom.clone(),
            workbench_view as fn(&WorkbenchState) -> WbView,
            state,
        );
        Self {
            dom,
            runner,
            tiling: WorkbenchTiling::default(),
        }
    }

    /// Refresh from app truth at the pane's size: walk the model's geometry
    /// into pane-local rects, resolve tab labels off graph truth, mirror each
    /// cell's active tab into its strip.
    pub fn sync(&mut self, app: &App, pane_w: f32, pane_h: f32) {
        let geom = app.workbench.to_arrangement().1;
        self.tiling = place_workbench(geom.as_ref(), Rect::new(0.0, 0.0, pane_w, pane_h));
        let label_of = |member: Uuid| -> String {
            app.canvas
                .graph()
                .nodes()
                .find(|(_, n)| n.id == member)
                .map(|(_, n)| {
                    if n.title.trim().is_empty() {
                        n.url().to_string()
                    } else {
                        n.title.clone()
                    }
                })
                .unwrap_or_else(|| "gone".to_string())
        };
        let cells: Vec<CellView> = self
            .tiling
            .cells
            .iter()
            .map(|c| {
                let labels: Vec<String> = c.members.iter().map(|&m| label_of(m)).collect();
                // The honest body hint: what the ACTIVE tile's content is
                // doing, when there is nothing composited to show for it.
                let hint = match c.active_member().map(|m| app.content.get(m)) {
                    Some(Some(NodeContent::Live)) => String::new(),
                    Some(Some(NodeContent::Requested)) => "content loading".to_string(),
                    Some(Some(NodeContent::Failed(err))) => format!("failed: {err}"),
                    Some(None) => "content off — Toggle live content".to_string(),
                    None => String::new(),
                };
                CellView {
                    members: c.members.clone(),
                    rect: c.rect,
                    labels,
                    strip: TabStrip::new(c.active).with_label("Workbench cell"),
                    synced_active: c.active,
                    hint,
                }
            })
            .collect();
        self.runner.update(|state| {
            state.cells = cells;
            state.viewport_w = pane_w;
            state.viewport_h = pane_h;
        });
    }

    /// The last synced walk (pane-local rects). The shell's press routing
    /// reads cells and divider bands from here so input and paint agree.
    pub fn tiling(&self) -> &WorkbenchTiling {
        &self.tiling
    }

    /// The pane's scene at its size, under the host's cambium sheet.
    pub fn scene(&self, w: u32, h: u32) -> netrender::Scene {
        crate::ui::scene_from_dom(&self.dom.borrow(), crate::ui::CAMBIUM_SHEET, w, h)
    }

    /// The member whose TAB sits under pane-local `(x, y)`, from the laid-out
    /// DOM (the ask-the-layout rule: tab x-positions are flex + text
    /// measurement, so only the layout knows them). The tab-drag gesture's
    /// press resolution.
    pub fn tab_at(&self, x: f32, y: f32, w: u32, h: u32) -> Option<Uuid> {
        let dom = self.dom.borrow();
        let layout =
            IncrementalLayout::new(&*dom, &[crate::ui::CAMBIUM_SHEET], w as f32, h as f32);
        // Tabs appear in DOM order: cells in walk order, tabs in member order.
        let flat: Vec<Uuid> = self
            .runner
            .state()
            .cells
            .iter()
            .flat_map(|c| c.members.iter().copied())
            .collect();
        let tabs = dom.all_with_class(dom.document(), "tab");
        for (i, &tab) in tabs.iter().enumerate() {
            if let Some((tx, ty, tw, th)) = layout.absolute_rect(&*dom, tab)
                && x >= tx
                && x < tx + tw
                && y >= ty
                && y < ty + th
            {
                return flat.get(i).copied();
            }
        }
        None
    }

    /// Route a click at pane-local `(x, y)` into the view (a tab click moves
    /// its strip's selection); report each cell whose selection moved away
    /// from the synced model state as an activation for the shell to lower.
    pub fn click(&mut self, x: f32, y: f32, w: u32, h: u32) -> Vec<WbActivate> {
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
        self.runner
            .state()
            .cells
            .iter()
            .filter(|c| c.strip.selected != c.synced_active)
            .filter_map(|c| c.members.get(c.strip.selected).copied())
            .map(WbActivate)
            .collect()
    }

    /// Resolve a selector to a point within this pane's DOM at window rect
    /// `rect`, via the shared genet-probe resolver (the scenario's
    /// `drag-tab` aims through this).
    pub fn resolve(&self, sel: &genet_probe::Selector, rect: [f32; 4]) -> Option<(f32, f32)> {
        let dom = self.dom.borrow();
        let surfaces = [genet_probe::ProbeSurface {
            name: "workbench",
            dom: &dom,
            rect,
            sheet: crate::ui::CAMBIUM_SHEET,
        }];
        genet_probe::resolve(&surfaces, sel).map(|hit| hit.point)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::Action;

    fn pane_over(app: &App, w: f32, h: f32) -> WorkbenchPane {
        let mut pane = WorkbenchPane::new();
        pane.sync(app, w, h);
        pane
    }

    fn app_with_two_tiles() -> (App, Uuid, Uuid) {
        let mut app = App::test_stub();
        app.update(Action::OpenAddress("mere://alpha".to_string()));
        let a = app.canvas.focused_member().unwrap();
        app.update(Action::OpenInWorkbench);
        app.update(Action::OpenAddress("mere://beta".to_string()));
        let b = app.canvas.focused_member().unwrap();
        app.update(Action::OpenInWorkbench);
        (app, a, b)
    }

    /// Two tiles draw as two cells, each wearing a real tab strip; the strips'
    /// tabs resolve by label through the shared prober.
    #[test]
    fn two_tiles_draw_two_strips() {
        let (app, _, _) = app_with_two_tiles();
        let pane = pane_over(&app, 800.0, 600.0);
        assert_eq!(pane.tiling().cells.len(), 2);
        let alpha = pane.resolve(
            &genet_probe::Selector::class("tab").containing("alpha"),
            [0.0, 0.0, 800.0, 600.0],
        );
        let beta = pane.resolve(
            &genet_probe::Selector::class("tab").containing("beta"),
            [0.0, 0.0, 800.0, 600.0],
        );
        let (ax, _) = alpha.expect("alpha's tab is drawn");
        let (bx, _) = beta.expect("beta's tab is drawn");
        assert!(bx > ax, "beta's cell sits to the right");
    }

    /// The press resolution: a point inside a drawn tab maps back to its
    /// member — the tab-drag gesture's anchor.
    #[test]
    fn a_tab_point_resolves_to_its_member() {
        let (app, a, b) = app_with_two_tiles();
        let pane = pane_over(&app, 800.0, 600.0);
        let (ax, ay) = pane
            .resolve(
                &genet_probe::Selector::class("tab").containing("alpha"),
                [0.0, 0.0, 800.0, 600.0],
            )
            .unwrap();
        assert_eq!(pane.tab_at(ax, ay, 800, 600), Some(a));
        let (bx, by) = pane
            .resolve(
                &genet_probe::Selector::class("tab").containing("beta"),
                [0.0, 0.0, 800.0, 600.0],
            )
            .unwrap();
        assert_eq!(pane.tab_at(bx, by, 800, 600), Some(b));
        assert_eq!(pane.tab_at(400.0, 300.0, 800, 600), None, "a body point is no tab");
    }

    /// A click on an inactive tab of a stacked cell reports the activation
    /// for the shell to lower — the strip's own selection is the signal.
    #[test]
    fn clicking_an_inactive_tab_reports_activation() {
        let (mut app, a, b) = app_with_two_tiles();
        app.update(Action::WorkbenchStackOnto { dragged: b, target: a });
        // b landed active; a's tab is the inactive one.
        let mut pane = pane_over(&app, 800.0, 600.0);
        assert_eq!(pane.tiling().cells.len(), 1);
        let (x, y) = pane
            .resolve(
                &genet_probe::Selector::class("tab").containing("alpha"),
                [0.0, 0.0, 800.0, 600.0],
            )
            .expect("alpha's tab is drawn in the stack");
        let acts = pane.click(x, y, 800, 600);
        assert_eq!(acts.len(), 1);
        assert_eq!(acts[0].0, a, "clicking alpha's tab activates alpha");
    }
}
