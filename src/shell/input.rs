//! Input routing: pointer, wheel, keys, and the drag gestures, shared by winit
//! and the scenario runner so one description drives two runners.
//!
//! Focus picks the lane, for keys as for the pointer: the omnibar when open,
//! the focused page when one holds focus, the canvas otherwise. Ephemeral
//! input (scroll, hover, blur) rides state directly per the gesture law;
//! durable intent becomes an `Action`.

use winit::event::MouseButton;
use winit::keyboard::{Key as WinitKey, NamedKey as WinitNamedKey};

use crate::panes::PaneContent;
use genet_probe::AutomatableExt as _;
use inker::{SessionClick, SessionScrollKey};
use mere::canvas::PointerButton;

use crate::action::{Action, CaretMove};
use crate::surface::Rect;

use super::{Shell, decode_sprite};

/// The canvas's `PointerButton` for a winit `MouseButton`, or `None` for
/// buttons the canvas does not handle.
pub(super) fn pointer_button(button: MouseButton) -> Option<PointerButton> {
    match button {
        MouseButton::Left => Some(PointerButton::Left),
        MouseButton::Middle => Some(PointerButton::Middle),
        MouseButton::Right => Some(PointerButton::Right),
        _ => None,
    }
}

impl Shell {
    pub(super) fn click_pane_row(&mut self, substr: &str) {
        // Both list panes resolve through the shared driver's `click`: a Trail
        // `list-row` or a grid `roster-cell` whose text contains `substr`, over
        // all surfaces at once (no per-pane dispatch). Short-circuit `||` means a
        // hit presses once; only a total miss is attributable.
        let hit = self.click(&genet_probe::Selector::class("roster-cell").containing(substr))
            || self.click(&genet_probe::Selector::class("list-row").containing(substr))
            // A settings option is a row for receipt purposes (the Apparatus
            // pane's radio options).
            || self.click(&genet_probe::Selector::class("radio").containing(substr))
            // A composed list section's row (the gloss-composite): the same
            // verb addresses it, wherever the section was composed.
            || self.click(&genet_probe::Selector::class("section-row").containing(substr));
        if !hit {
            self.app.note(crate::observe::AppEvent::InteractionMissed {
                what: "click-row",
                target: substr.to_string(),
            });
            tracing::warn!(%substr, "click-row: no list-pane row matched");
        }
    }

    /// Click the Roster's tab labelled `label` (the scenario's `click-tab`),
    /// through the shared driver: a `.tab` element whose text is `label`. The
    /// strip's geometry is the layout's to know; the host names the target and
    /// the resolver finds it — the same substrate every genet app shares.
    pub(super) fn click_pane_tab(&mut self, label: &str) {
        if !self.click(&genet_probe::Selector::class("tab").containing(label)) {
            self.app.note(crate::observe::AppEvent::InteractionMissed {
                what: "click-tab",
                target: label.to_string(),
            });
            tracing::warn!(%label, "click-tab: no Roster tab matched");
        }
    }

    /// Click the Gloss minimap's node matching `substr` (the scenario's
    /// `click-node`), through the shared driver. The node buttons carry their url
    /// as `data-key`, so the driver selects on it — unique where the display
    /// label (two "Example Domain" pages) is not.
    pub(super) fn click_pane_node(&mut self, substr: &str) {
        let sel =
            genet_probe::Selector::class("graph-canvas-swatch-node").with_attr("data-key", substr);
        if !self.click(&sel) {
            self.app.note(crate::observe::AppEvent::InteractionMissed {
                what: "click-node",
                target: substr.to_string(),
            });
            tracing::warn!(%substr, "click-node: no pane node matched");
        }
    }

    /// Route a wheel event to the surface under `(x, y)` (rung 5 slice B). The
    /// page scrolls when the pointer is on it, the canvas pans when it is not.
    /// Ephemeral, so it drives the session's semantic method directly (the
    /// gesture law), never an Action. Shared by winit and the scenario runner.
    pub(super) fn deliver_wheel(&mut self, x: f32, y: f32, dx: f32, dy: f32) {
        let plan = self.surface_plan();
        if let Some(hit) = crate::surface::hit_test(&plan, self.app.focus, x, y)
            && let crate::surface::SurfaceKind::Content(node) = hit.kind
            && let Some(session) = self.content_sessions.get_mut(&node)
        {
            if session.scroll_at(hit.local.0, hit.local.1, dx, dy) {
                self.request_redraw();
            }
            return;
        }
        if self.app.canvas.wheel(dx, dy) {
            self.request_redraw();
        }
    }

    /// Route a pointer press to the surface under `(x, y)` and capture it until
    /// release (rung 5 slice B). A press on content focuses it and delivers the
    /// click: a link resolves to a durable navigation and goes through
    /// `Action::OpenAddress`, growing the graph; a press on the canvas begins a
    /// canvas gesture. Shared by winit and the scenario runner.
    pub(super) fn deliver_press(&mut self, x: f32, y: f32, button: MouseButton) {
        // A press while the omnibar is open dismisses it and is swallowed, so
        // the surface beneath never also reacts to the same press.
        if self.app.omnibar.open {
            // A press on a suggestion row COMMITS it (the retained chrome's
            // row handlers); anywhere else is the click-away dismiss.
            let intents = self
                .chrome
                .click(0, x, y, self.width.max(1), self.height.max(1));
            if let Some(crate::chrome_view::ChromeIntent::CommitRow(index)) =
                intents.into_iter().next()
            {
                self.act(Action::OmnibarCommitRow(index));
            } else {
                self.act(Action::OmnibarClose);
            }
            self.pointer_capture = None;
            return;
        }
        let plan = self.surface_plan();
        let hit = crate::surface::hit_test(&plan, self.app.focus, x, y);
        // Right-click is the context menu the palette registry names: open the
        // command palette (the `>` actions lane), selecting the graph node
        // under the pointer first so node-scoped actions apply to it. Panes and
        // content keep their own right-click behavior (none yet); this handles
        // the canvas, which is where the node-scoped actions live.
        if button == MouseButton::Right {
            if let Some(hit) = hit
                && matches!(hit.kind, crate::surface::SurfaceKind::Canvas)
                && let Some(member) = self.app.canvas.node_at_screen(hit.local.0, hit.local.1)
            {
                self.app.canvas.select_member(member);
            }
            self.act(Action::OmnibarOpen { command: true });
            self.pointer_capture = None;
            return;
        }
        self.pointer_capture = hit.map(|h| h.kind);
        if let Some(hit) = hit {
            match hit.kind {
                crate::surface::SurfaceKind::Content(node) => {
                    self.app.focus = crate::surface::FocusTarget::Content(node);
                    if button == MouseButton::Left
                        && let Some(session) = self.content_sessions.get_mut(&node)
                        && let SessionClick::Navigate(url) =
                            session.click_at(hit.local.0, hit.local.1)
                    {
                        self.act(Action::OpenAddress(url));
                    }
                    self.request_redraw();
                    return;
                }
                // A press on a pane makes it the active pane (the anchor for
                // close/maximize/divider). A Trail pane also routes the click to
                // its row (slice D): a navigable row lowers Action::OpenAddress
                // through the same spine as a keypress. Other panes are still
                // placeholders (slice C), so the press is otherwise swallowed.
                crate::surface::SurfaceKind::Pane(id) => {
                    self.app.active_pane = Some(id);
                    if button == MouseButton::Left {
                        match self.pane_content(id) {
                            Some(PaneContent::Trail) => {
                                // The same cambium round trip as the Roster.
                                let dims = plan.iter().find(|s| s.id == hit.id).map(|s| {
                                    (
                                        s.rect.w.round().max(1.0) as u32,
                                        s.rect.h.round().max(1.0) as u32,
                                    )
                                });
                                let actions = match (dims, self.trail_pane.as_mut()) {
                                    (Some((rw, rh)), Some(pane)) => {
                                        pane.click(hit.local.0, hit.local.1, rw, rh)
                                    }
                                    _ => Vec::new(),
                                };
                                for action in actions {
                                    match action {
                                        crate::trail_pane::TrailPaneAction::Navigate(url) => {
                                            self.act(Action::OpenAddress(url))
                                        }
                                        crate::trail_pane::TrailPaneAction::RecoverSession(id) => {
                                            // A Removed-sessions row: restore the
                                            // trashed session and switch (O3).
                                            if let Ok(id) = id.parse::<uuid::Uuid>() {
                                                self.act(Action::RecoverSession(
                                                    crate::panes::SessionId::from_uuid(id),
                                                ));
                                            }
                                        }
                                        crate::trail_pane::TrailPaneAction::Recover(id) => {
                                            // The Removed row carries the staged
                                            // node's ORIGINAL uuid; recovery
                                            // restores that identity.
                                            match id.parse::<uuid::Uuid>() {
                                                Ok(id) => self.act(Action::RecoverDeletedNode(id)),
                                                Err(_) => self.app.note(
                                                    crate::observe::AppEvent::InteractionMissed {
                                                        what: "recover",
                                                        target: id,
                                                    },
                                                ),
                                            }
                                        }
                                    }
                                }
                            }
                            Some(PaneContent::Roster) => {
                                // Route into the cambium grid: hit-test its DOM
                                // at the pane's size and dispatch, then lower
                                // whatever the view emitted through the spine —
                                // the same path a keypress takes. This is the
                                // general cambium pane-event round trip.
                                let dims = plan.iter().find(|s| s.id == hit.id).map(|s| {
                                    (
                                        s.rect.w.round().max(1.0) as u32,
                                        s.rect.h.round().max(1.0) as u32,
                                    )
                                });
                                let actions = match (dims, self.roster_grid.as_mut()) {
                                    (Some((rw, rh)), Some(grid)) => {
                                        let actions = grid.click(hit.local.0, hit.local.1, rw, rh);
                                        // The strip emits no action — switching a
                                        // tab is a state change in the widget's
                                        // own state. Mirror it out so the rest of
                                        // the host can see which tab is showing.
                                        self.app.roster_tab = grid.selected_tab().0;
                                        actions
                                    }
                                    _ => Vec::new(),
                                };
                                for action in actions {
                                    match action {
                                        crate::cambium_pane::RosterAction::Navigate(url) => {
                                            self.act(Action::OpenAddress(url))
                                        }
                                    }
                                }
                            }
                            Some(PaneContent::Gloss(_)) => {
                                // Same hit-test round trip; the outcome arrives
                                // as drained intents (the swatch mutates state
                                // rather than bubbling), lowered here.
                                let dims = plan.iter().find(|s| s.id == hit.id).map(|s| {
                                    (
                                        s.rect.w.round().max(1.0) as u32,
                                        s.rect.h.round().max(1.0) as u32,
                                    )
                                });
                                let intents = match (dims, self.gloss_pane.as_mut()) {
                                    (Some((rw, rh)), Some(pane)) => {
                                        pane.click(hit.local.0, hit.local.1, rw, rh)
                                    }
                                    _ => Vec::new(),
                                };
                                for intent in intents {
                                    match intent {
                                        crate::swatch_pane::SwatchIntent::Activate(
                                            crate::swatch_pane::SwatchActivate::Open(url),
                                        ) => self.act(Action::OpenAddress(url)),
                                        crate::swatch_pane::SwatchIntent::Activate(
                                            crate::swatch_pane::SwatchActivate::Switch(id),
                                        ) => self.act(Action::SwitchSession(id)),
                                        // A composed Removed row: recover the
                                        // node under its ORIGINAL id.
                                        crate::swatch_pane::SwatchIntent::Activate(
                                            crate::swatch_pane::SwatchActivate::Recover(id),
                                        ) => self.act(Action::RecoverDeletedNode(id)),
                                        crate::swatch_pane::SwatchIntent::Expand => {
                                            self.app.focus = crate::surface::FocusTarget::Canvas;
                                        }
                                    }
                                }
                            }
                            Some(PaneContent::Inspector) => {
                                let dims = plan.iter().find(|s| s.id == hit.id).map(|s| {
                                    (
                                        s.rect.w.round().max(1.0) as u32,
                                        s.rect.h.round().max(1.0) as u32,
                                    )
                                });
                                let clip = match (dims, self.inspector_pane.as_mut()) {
                                    (Some((rw, rh)), Some(pane)) => pane
                                        .click(hit.local.0, hit.local.1, rw, rh)
                                        .into_iter()
                                        .any(|intent| {
                                            matches!(
                                                intent,
                                                crate::inspector_pane::InspectorIntent::ClipToKnot
                                            )
                                        }),
                                    _ => false,
                                };
                                if clip {
                                    self.clip_focused_document_to_knot();
                                }
                            }
                            Some(PaneContent::Apparatus) => {
                                // The same cambium round trip: the radio's own
                                // selection moves, and the diff lowers as the
                                // typed viewer Action for the FOCUSED node.
                                let dims = plan.iter().find(|s| s.id == hit.id).map(|s| {
                                    (
                                        s.rect.w.round().max(1.0) as u32,
                                        s.rect.h.round().max(1.0) as u32,
                                    )
                                });
                                let intents = match (dims, self.apparatus_pane.as_mut()) {
                                    (Some((rw, rh)), Some(pane)) => {
                                        pane.click(hit.local.0, hit.local.1, rw, rh)
                                    }
                                    _ => Vec::new(),
                                };
                                for intent in intents {
                                    match intent {
                                        crate::apparatus_pane::ApparatusIntent::SetViewer(
                                            viewer,
                                        ) => {
                                            if let Some(member) = self.app.canvas.focused_member() {
                                                self.act(Action::SetViewerOverride {
                                                    member,
                                                    viewer,
                                                });
                                            }
                                        }
                                    }
                                }
                            }
                            Some(PaneContent::Overmap(_)) => {
                                // A session-node click adopts that session:
                                // navigating to a container IS the switch
                                // (overmap v0), through the ordinary spine.
                                let dims = plan.iter().find(|s| s.id == hit.id).map(|s| {
                                    (
                                        s.rect.w.round().max(1.0) as u32,
                                        s.rect.h.round().max(1.0) as u32,
                                    )
                                });
                                let intents = match (dims, self.overmap_pane.as_mut()) {
                                    (Some((rw, rh)), Some(pane)) => {
                                        pane.click(hit.local.0, hit.local.1, rw, rh)
                                    }
                                    _ => Vec::new(),
                                };
                                for intent in intents {
                                    match intent {
                                        crate::swatch_pane::SwatchIntent::Activate(
                                            crate::swatch_pane::SwatchActivate::Open(url),
                                        ) => self.act(Action::OpenAddress(url)),
                                        crate::swatch_pane::SwatchIntent::Activate(
                                            crate::swatch_pane::SwatchActivate::Switch(id),
                                        ) => self.act(Action::SwitchSession(id)),
                                        // A composed Removed row: recover the
                                        // node under its ORIGINAL id.
                                        crate::swatch_pane::SwatchIntent::Activate(
                                            crate::swatch_pane::SwatchActivate::Recover(id),
                                        ) => self.act(Action::RecoverDeletedNode(id)),
                                        crate::swatch_pane::SwatchIntent::Expand => {
                                            self.app.focus = crate::surface::FocusTarget::Canvas;
                                        }
                                    }
                                }
                            }
                            Some(PaneContent::Workbench) => {
                                // A press here begins a gesture, resolved on
                                // RELEASE (a tab click activates; a tab drag
                                // onto another cell stacks; a seam drag
                                // re-weights) — so record what was pressed and
                                // decide in deliver_release / deliver_move.
                                let dims = plan.iter().find(|s| s.id == hit.id).map(|s| {
                                    (
                                        s.rect,
                                        (
                                            s.rect.w.round().max(1.0) as u32,
                                            s.rect.h.round().max(1.0) as u32,
                                        ),
                                    )
                                });
                                if let (Some((rect, (rw, rh))), Some(pane)) =
                                    (dims, self.workbench_pane.as_mut())
                                {
                                    let (lx, ly) = hit.local;
                                    if let Some(div) = pane.tiling().divider_at(lx, ly).cloned() {
                                        self.wb_divider_drag = Some((div, (rect.x, rect.y)));
                                    } else if let Some(member) = pane.tab_at(lx, ly, rw, rh) {
                                        self.wb_tab_drag = Some(member);
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    self.request_redraw();
                    return;
                }
                crate::surface::SurfaceKind::Divider(index) => {
                    let area = Rect::full(self.width.max(1), self.height.max(1));
                    let tiling =
                        crate::pane::place_panes(&self.app.frisket, area, self.app.maximized);
                    self.divider_drag = tiling.dividers.into_iter().find(|d| d.index == index);
                    self.request_redraw();
                    return;
                }
                // The canvas (chrome is unreachable — an open omnibar was handled
                // above). Pressing it focuses it and begins the canvas gesture.
                crate::surface::SurfaceKind::Canvas | crate::surface::SurfaceKind::Chrome => {
                    self.app.focus = crate::surface::FocusTarget::Canvas;
                    if let Some(button) = pointer_button(button)
                        && self.app.canvas.pointer_down(button, x, y)
                    {
                        self.request_redraw();
                    }
                }
            }
        }
    }

    /// Route a pointer release to whatever the matching press captured (rung 5
    /// slice B). The canvas gets a release only if its own press began the
    /// gesture, so a content click never ends a canvas drag. Shared by winit
    /// and the scenario runner.

    /// Route a pointer move. Today only the divider drag consumes moves: while
    /// a seam is captured, each move becomes a ratio through cambium's
    /// `Split::ratio_at` over the split's own container rect, lowered as an
    /// ordinary Action — the same spine as everything else.
    /// Route a pointer move into the pane under it (pane pointer-move
    /// routing): the swatch panes get their Enter/Leave hover transitions, so
    /// the hover emphasis the component always supported finally lights up.
    /// A move off a hovering pane delivers its Leave. Ephemeral, so it drives
    /// the panes' semantic methods directly (the gesture law), never an Action.
    pub(super) fn deliver_hover(&mut self, x: f32, y: f32) {
        let plan = self.surface_plan();
        let hit = crate::surface::hit_test(&plan, self.app.focus, x, y);
        let pane_hit = match hit.as_ref().map(|h| h.kind) {
            Some(crate::surface::SurfaceKind::Pane(id)) => Some(id),
            _ => None,
        };
        let mut redraw = false;
        // Leaving the previously hovered pane clears its emphasis.
        if let Some(prev) = self.hovered_pane
            && pane_hit != Some(prev)
        {
            redraw |= match self.pane_content(prev) {
                Some(PaneContent::Gloss(_)) => {
                    self.gloss_pane.as_mut().is_some_and(|p| p.hover_leave())
                }
                Some(PaneContent::Overmap(_)) => {
                    self.overmap_pane.as_mut().is_some_and(|p| p.hover_leave())
                }
                _ => false,
            };
        }
        self.hovered_pane = pane_hit;
        if let (Some(hit), Some(id)) = (hit, pane_hit) {
            let dims = plan.iter().find(|s| s.id == hit.id).map(|s| {
                (
                    s.rect.w.round().max(1.0) as u32,
                    s.rect.h.round().max(1.0) as u32,
                )
            });
            if let Some((rw, rh)) = dims {
                redraw |= match self.pane_content(id) {
                    Some(PaneContent::Gloss(_)) => self
                        .gloss_pane
                        .as_mut()
                        .is_some_and(|p| p.hover(hit.local.0, hit.local.1, rw, rh)),
                    Some(PaneContent::Overmap(_)) => self
                        .overmap_pane
                        .as_mut()
                        .is_some_and(|p| p.hover(hit.local.0, hit.local.1, rw, rh)),
                    _ => false,
                };
            }
        }
        if redraw {
            self.request_redraw();
        }
    }

    pub(super) fn deliver_move(&mut self, x: f32, y: f32) {
        // A workbench divider drag: the band's pair re-weights toward the
        // pointer (host math over platen's N-ary fractions), lowered as an
        // ordinary Action. The walk is pane-local; the origin converts.
        if let Some((div, origin)) = self.wb_divider_drag.clone() {
            let fractions =
                crate::workbench_tiling::drag_fractions(&div, x - origin.0, y - origin.1);
            self.act(Action::WorkbenchSetFractions {
                path: div.path,
                fractions,
            });
            return;
        }
        let Some(drag) = self.divider_drag.clone() else {
            return;
        };
        let split = crate::pane::cambium_split(drag.axis, drag.ratio);
        let ratio = split.ratio_at(drag.area.w, drag.area.h, x - drag.area.x, y - drag.area.y);
        self.act(Action::SetSplitRatio {
            space: crate::action::SpaceRef::Primary,
            path: drag.path,
            ratio,
        });
    }

    pub(super) fn deliver_release(&mut self, x: f32, y: f32, button: MouseButton) {
        let to_canvas = matches!(
            self.pointer_capture,
            Some(crate::surface::SurfaceKind::Canvas)
        );
        self.pointer_capture = None;
        if self.wb_divider_drag.take().is_some() {
            // Like the frisket seam: moves rode Redraw; persist on release.
            self.act(Action::SaveSession);
            return;
        }
        if let Some(dragged) = self.wb_tab_drag.take() {
            self.finish_wb_tab_gesture(dragged, x, y);
            return;
        }
        if self.divider_drag.take().is_some() {
            // The drag's ratio moves rode Redraw only; the settled layout
            // persists once, on release.
            self.act(Action::SaveSession);
            return;
        }
        if to_canvas
            && let Some(button) = pointer_button(button)
            && self.app.canvas.pointer_up(button, x, y)
        {
            self.request_redraw();
        }
    }
}
