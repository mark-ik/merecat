//! Input routing: pointer, wheel, keys, and the drag gestures, shared by winit
//! and the scenario runner so one description drives two runners.
//!
//! Focus picks the lane, for keys as for the pointer: the omnibar when open,
//! the focused page when one holds focus, the canvas otherwise. Ephemeral
//! input (scroll, hover, blur) rides state directly per the gesture law;
//! durable intent becomes an `Action`.

use winit::event::MouseButton;
use winit::keyboard::{Key as WinitKey, NamedKey as WinitNamedKey};

use frisket::PaneContent;
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

/// The scroll a focused content session should perform for a key, or `None`
/// when the key is not a content-scroll key. Space pages down, Shift+Space
/// pages up (the browser convention); the arrows line-scroll, and Home/End
/// jump to the ends.
fn content_scroll_key(key: &WinitKey, shift: bool) -> Option<SessionScrollKey> {
    Some(match key {
        WinitKey::Named(WinitNamedKey::ArrowDown) => SessionScrollKey::LineDown,
        WinitKey::Named(WinitNamedKey::ArrowUp) => SessionScrollKey::LineUp,
        WinitKey::Named(WinitNamedKey::PageDown) => SessionScrollKey::PageDown,
        WinitKey::Named(WinitNamedKey::PageUp) => SessionScrollKey::PageUp,
        WinitKey::Named(WinitNamedKey::Home) => SessionScrollKey::Home,
        WinitKey::Named(WinitNamedKey::End) => SessionScrollKey::End,
        WinitKey::Named(WinitNamedKey::Space) if shift => SessionScrollKey::PageUp,
        WinitKey::Named(WinitNamedKey::Space) => SessionScrollKey::PageDown,
        _ => return None,
    })
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
        let sel = genet_probe::Selector::class("graph-canvas-swatch-node")
            .with_attr("data-key", substr);
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

    /// Deliver an ephemeral key to the FOCUSED content session (the gesture
    /// law, exactly as the wheel does): scroll keys scroll the page, Escape
    /// blurs back to the canvas. Returns whether the key was consumed here, so
    /// the caller skips the Action path. Keys that are NOT ephemeral content
    /// keys (the durable node/nav chords) return `false` and fall through to
    /// become Actions. Unlike the wheel this is focus-routed, not
    /// position-routed: a page reader's keys go to the page they are reading.
    pub(super) fn deliver_content_key(&mut self, key: &WinitKey) -> bool {
        let crate::surface::FocusTarget::Content(node) = self.app.focus else {
            return false;
        };
        // Escape blurs back to the canvas. Focus is ephemeral UI state (the
        // press path sets it directly too), so this rides on state, not an
        // Action.
        if matches!(key, WinitKey::Named(WinitNamedKey::Escape)) {
            self.app.focus = crate::surface::FocusTarget::Canvas;
            self.request_redraw();
            return true;
        }
        let Some(scroll) = content_scroll_key(key, self.shift) else {
            return false;
        };
        let moved = self
            .content_sessions
            .get_mut(&node)
            .is_some_and(|session| session.scroll_for_key(scroll));
        // Record the outcome for the scenario probe, and repaint when the page
        // actually moved.
        self.content_scroll_moved = Some(moved);
        if moved {
            self.request_redraw();
        }
        // Consumed even when the page did not move (already at the end): the
        // key belonged to the focused page, so the canvas must not also act on
        // it.
        true
    }

    /// The whole pressed-key path, shared by winit and the scenario runner so
    /// one description drives two runners for keys as well as pointers. Focus
    /// decides the lane, exactly as it does for the pointer: the omnibar when
    /// it is open, the focused content when a page holds focus, the canvas
    /// otherwise. Ephemeral content keys (scroll, blur) are delivered inline
    /// and consumed; everything else lowers to an Action through the spine.
    pub(super) fn on_key(&mut self, key: &WinitKey) {
        // Content-focused ephemeral keys take priority and never become
        // Actions (the gesture law). When one is consumed, no Action is
        // computed — the canvas view hotkeys stay suspended while a page reads.
        if !self.app.omnibar.open && self.deliver_content_key(key) {
            return;
        }
        let action = if self.app.omnibar.open {
            // The omnibar has keyboard focus: edit keys route to it; canvas
            // hotkeys are suspended while it is open.
            match key {
                WinitKey::Named(WinitNamedKey::Escape) => Some(Action::OmnibarClose),
                WinitKey::Named(WinitNamedKey::Enter) => Some(Action::OmnibarCommit),
                WinitKey::Named(WinitNamedKey::Backspace) => Some(Action::OmnibarBackspace),
                WinitKey::Named(WinitNamedKey::ArrowUp) => Some(Action::OmnibarMove(-1)),
                WinitKey::Named(WinitNamedKey::ArrowDown) => Some(Action::OmnibarMove(1)),
                WinitKey::Named(WinitNamedKey::ArrowLeft) => {
                    Some(Action::OmnibarCaret(CaretMove::Left))
                }
                WinitKey::Named(WinitNamedKey::ArrowRight) => {
                    Some(Action::OmnibarCaret(CaretMove::Right))
                }
                WinitKey::Named(WinitNamedKey::Home) => Some(Action::OmnibarCaret(CaretMove::Home)),
                WinitKey::Named(WinitNamedKey::End) => Some(Action::OmnibarCaret(CaretMove::End)),
                WinitKey::Named(WinitNamedKey::Delete) => Some(Action::OmnibarDelete),
                WinitKey::Named(WinitNamedKey::Space) => Some(Action::OmnibarChar(' ')),
                WinitKey::Character(s) if !self.ctrl => s.chars().next().map(Action::OmnibarChar),
                _ => None,
            }
        } else if matches!(self.app.focus, crate::surface::FocusTarget::Content(_)) {
            // A page holds focus. Its scroll keys and Escape were already
            // consumed above; only the durable node/nav chords still apply
            // here. The canvas VIEW hotkeys (reseed, isometric, orbit) are
            // deliberately suspended: you are in the page, so a stray `space`
            // or `i` must not reshape the graph behind it.
            match key {
                WinitKey::Named(WinitNamedKey::Delete) => Some(Action::DeleteFocusedNode),
                WinitKey::Named(WinitNamedKey::ArrowLeft) if self.alt => Some(Action::NavBack),
                WinitKey::Named(WinitNamedKey::ArrowRight) if self.alt => Some(Action::NavForward),
                WinitKey::Character(s) if self.ctrl => match s.as_str() {
                    "l" => Some(Action::OmnibarOpen { command: false }),
                    "k" => Some(Action::OmnibarOpen { command: true }),
                    "r" => Some(Action::Reload),
                    _ => None,
                },
                _ => None,
            }
        } else {
            match key {
                WinitKey::Named(WinitNamedKey::Space) => Some(Action::ReseedLayout),
                // Delete forgets the focused node (recoverable from the Trail's
                // Removed section).
                WinitKey::Named(WinitNamedKey::Delete) => Some(Action::DeleteFocusedNode),
                // The browser nav chords (the r3-owed row).
                WinitKey::Named(WinitNamedKey::ArrowLeft) if self.alt => Some(Action::NavBack),
                WinitKey::Named(WinitNamedKey::ArrowRight) if self.alt => Some(Action::NavForward),
                WinitKey::Character(s) if self.ctrl => match s.as_str() {
                    // The summon chords: Ctrl+L address flavor, Ctrl+K command
                    // flavor (pre-seeded `>`).
                    "l" => Some(Action::OmnibarOpen { command: false }),
                    "k" => Some(Action::OmnibarOpen { command: true }),
                    "r" => Some(Action::Reload),
                    _ => None,
                },
                WinitKey::Character(s) => match s.as_str() {
                    // Plain-key summons beside the Ctrl chords: `/` (the
                    // quick-switcher convention) and `>` straight into the
                    // actions lane. Chord-free, so synthesized-input drivers
                    // can't lose the modifier race either.
                    "/" => Some(Action::OmnibarOpen { command: false }),
                    ">" => Some(Action::OmnibarOpen { command: true }),
                    "i" => Some(Action::ToggleIsometric),
                    "q" => Some(Action::OrbitBy(-0.15)),
                    "e" => Some(Action::OrbitBy(0.15)),
                    "[" => Some(Action::TiltBy(-0.05)),
                    "]" => Some(Action::TiltBy(0.05)),
                    "h" => Some(Action::ToggleHeightByDegree),
                    _ => None,
                },
                _ => None,
            }
        };
        if let Some(action) = action {
            self.act(action);
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
            let intents = self.chrome.click(0, x, y, self.width.max(1), self.height.max(1));
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
                                let dims = plan
                                    .iter()
                                    .find(|s| s.id == hit.id)
                                    .map(|s| (s.rect.w.round().max(1.0) as u32, s.rect.h.round().max(1.0) as u32));
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
                                                    frisket::SessionId::from_uuid(id),
                                                ));
                                            }
                                        }
                                        crate::trail_pane::TrailPaneAction::Recover(id) => {
                                            // The Removed row carries the staged
                                            // node's ORIGINAL uuid; recovery
                                            // restores that identity.
                                            match id.parse::<uuid::Uuid>() {
                                                Ok(id) => self.act(
                                                    Action::RecoverDeletedNode(id),
                                                ),
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
                                let dims = plan
                                    .iter()
                                    .find(|s| s.id == hit.id)
                                    .map(|s| (s.rect.w.round().max(1.0) as u32, s.rect.h.round().max(1.0) as u32));
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
                                let dims = plan
                                    .iter()
                                    .find(|s| s.id == hit.id)
                                    .map(|s| (s.rect.w.round().max(1.0) as u32, s.rect.h.round().max(1.0) as u32));
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
                                            self.app.focus =
                                                crate::surface::FocusTarget::Canvas;
                                        }
                                    }
                                }
                            }
                            Some(PaneContent::Apparatus) => {
                                // The same cambium round trip: the radio's own
                                // selection moves, and the diff lowers as the
                                // typed viewer Action for the FOCUSED node.
                                let dims = plan
                                    .iter()
                                    .find(|s| s.id == hit.id)
                                    .map(|s| (s.rect.w.round().max(1.0) as u32, s.rect.h.round().max(1.0) as u32));
                                let intents = match (dims, self.apparatus_pane.as_mut()) {
                                    (Some((rw, rh)), Some(pane)) => {
                                        pane.click(hit.local.0, hit.local.1, rw, rh)
                                    }
                                    _ => Vec::new(),
                                };
                                for intent in intents {
                                    match intent {
                                        crate::apparatus_pane::ApparatusIntent::SetViewer(viewer) => {
                                            if let Some(member) =
                                                self.app.canvas.focused_member()
                                            {
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
                                let dims = plan
                                    .iter()
                                    .find(|s| s.id == hit.id)
                                    .map(|s| (s.rect.w.round().max(1.0) as u32, s.rect.h.round().max(1.0) as u32));
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
                                            self.app.focus =
                                                crate::surface::FocusTarget::Canvas;
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
                                let dims = plan
                                    .iter()
                                    .find(|s| s.id == hit.id)
                                    .map(|s| (s.rect, (s.rect.w.round().max(1.0) as u32, s.rect.h.round().max(1.0) as u32)));
                                if let (Some((rect, (rw, rh))), Some(pane)) =
                                    (dims, self.workbench_pane.as_mut())
                                {
                                    let (lx, ly) = hit.local;
                                    if let Some(div) = pane.tiling().divider_at(lx, ly).cloned() {
                                        self.wb_divider_drag =
                                            Some((div, (rect.x, rect.y)));
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
                    self.divider_drag = tiling
                        .dividers
                        .into_iter()
                        .find(|d| d.index == index);
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
            let dims = plan
                .iter()
                .find(|s| s.id == hit.id)
                .map(|s| (s.rect.w.round().max(1.0) as u32, s.rect.h.round().max(1.0) as u32));
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
            let fractions = crate::workbench_tiling::drag_fractions(
                &div,
                x - origin.0,
                y - origin.1,
            );
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
        let ratio = split.ratio_at(
            drag.area.w,
            drag.area.h,
            x - drag.area.x,
            y - drag.area.y,
        );
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

    /// Resolve a workbench tab gesture at its release point: released over a
    /// DIFFERENT cell, the dragged tile stacks into it (platen's
    /// `move_to_slot_of`, lowered as an Action); released where it began, it
    /// is a click — routed into the pane's DOM so the strip's own selection
    /// answers, and the diff lowers as `WorkbenchActivate`.
    pub(super) fn finish_wb_tab_gesture(&mut self, dragged: uuid::Uuid, x: f32, y: f32) {
        let plan = self.surface_plan();
        let Some(surface) = plan.iter().find(|s| {
            matches!(s.kind, crate::surface::SurfaceKind::Pane(id)
                if self.pane_content(id) == Some(PaneContent::Workbench))
        }) else {
            return;
        };
        if !surface.rect.contains(x, y) {
            // Released OUTSIDE the workbench: Ctrl+Shift held is the FORK arm
            // (brief's gesture table — a new session snapshots the component);
            // otherwise the branch arm — the dragged tile tears out of the
            // tiling into a lens window as a pinned Tile pane. Both lower
            // through the same spine as every other op.
            if self.ctrl && self.shift {
                self.act(Action::ForkNode { member: dragged });
            } else {
                self.act(Action::TearOutTile { member: dragged });
            }
            self.request_redraw();
            return;
        }
        let (lx, ly) = surface.rect.to_local(x, y);
        let (rw, rh) = (
            surface.rect.w.round().max(1.0) as u32,
            surface.rect.h.round().max(1.0) as u32,
        );
        let Some(pane) = self.workbench_pane.as_mut() else {
            return;
        };
        let target_cell = pane.tiling().cell_at(lx, ly).cloned();
        match target_cell {
            Some(cell) => {
                // WHERE in the cell decides the gesture (meerkat's drop
                // resolution, re-derived): edge bands split (out of the own
                // cell, or beside another's); a different cell's tab bar or
                // centre stacks; anywhere else it is a click — the strip's
                // selection moves and the diff lowers through the spine.
                let target = cell.active_member().unwrap_or(dragged);
                match crate::workbench_tiling::wb_drop_action(dragged, target, &cell, lx, ly) {
                    Some(action) => self.act(action),
                    None => {
                        let activations = pane.click(lx, ly, rw, rh);
                        for a in activations {
                            self.act(Action::WorkbenchActivate(a.0));
                        }
                        self.request_redraw();
                    }
                }
            }
            None => {
                self.request_redraw();
            }
        }
    }

    /// Drive a workbench tab drag by LABEL (the scenario's `drag-tab`): both
    /// tab centres resolve through the pane's DOM (the shared prober), then
    /// the gesture runs through the same press/move/release path a pointer
    /// takes — one description, two runners.
    pub(super) fn drag_workbench_tab(&mut self, from: &str, onto: &str, edge: Option<&str>) {
        let plan = self.surface_plan();
        let found = plan.iter().find_map(|s| {
            let crate::surface::SurfaceKind::Pane(id) = s.kind else {
                return None;
            };
            if self.pane_content(id) != Some(PaneContent::Workbench) {
                return None;
            }
            let rect = [s.rect.x, s.rect.y, s.rect.w, s.rect.h];
            let pane = self.workbench_pane.as_ref()?;
            let a = pane.resolve(&genet_probe::Selector::class("tab").containing(from), rect)?;
            let b = pane.resolve(&genet_probe::Selector::class("tab").containing(onto), rect)?;
            // An edge release aims 10% into that band of the TARGET CELL's
            // body rather than at the tab (the split-beside zones).
            let release = match edge {
                None => b,
                Some(edge) => {
                    let local = (b.0 - s.rect.x, b.1 - s.rect.y);
                    let cell = pane.tiling().cell_at(local.0, local.1)?;
                    let body = cell.body();
                    let (px, py) = match edge {
                        "left" => (body.x + body.w * 0.1, body.y + body.h * 0.5),
                        "right" => (body.x + body.w * 0.9, body.y + body.h * 0.5),
                        "top" => (body.x + body.w * 0.5, body.y + body.h * 0.1),
                        _ => (body.x + body.w * 0.5, body.y + body.h * 0.9),
                    };
                    (s.rect.x + px, s.rect.y + py)
                }
            };
            Some((a, release))
        });
        let Some(((ax, ay), (bx, by))) = found else {
            self.app.note(crate::observe::AppEvent::InteractionMissed {
                what: "drag-tab",
                target: format!("{from} onto {onto}"),
            });
            tracing::warn!(%from, %onto, "drag-tab: no workbench tabs matched");
            return;
        };
        self.deliver_press(ax, ay, MouseButton::Left);
        self.deliver_move((ax + bx) / 2.0, (ay + by) / 2.0);
        self.deliver_move(bx, by);
        self.deliver_release(bx, by, MouseButton::Left);
    }

    /// Drive the tile TEAR-OUT drag by label (the scenario's `drag-tab <a>
    /// out`): the tab centre resolves through the pane's DOM and the release
    /// lands at the CANVAS pane's centre — outside the workbench, so the same
    /// press/move/release path a pointer takes resolves the branch arm.
    pub(super) fn drag_workbench_tab_out(&mut self, from: &str) {
        let plan = self.surface_plan();
        let start = plan.iter().find_map(|s| {
            let crate::surface::SurfaceKind::Pane(id) = s.kind else {
                return None;
            };
            if self.pane_content(id) != Some(PaneContent::Workbench) {
                return None;
            }
            let rect = [s.rect.x, s.rect.y, s.rect.w, s.rect.h];
            let pane = self.workbench_pane.as_ref()?;
            pane.resolve(&genet_probe::Selector::class("tab").containing(from), rect)
        });
        let release = plan
            .iter()
            .find(|s| matches!(s.kind, crate::surface::SurfaceKind::Canvas))
            .map(|s| (s.rect.x + s.rect.w / 2.0, s.rect.y + s.rect.h / 2.0));
        let (Some((ax, ay)), Some((bx, by))) = (start, release) else {
            self.app.note(crate::observe::AppEvent::InteractionMissed {
                what: "drag-tab",
                target: format!("{from} out"),
            });
            tracing::warn!(%from, "drag-tab out: no matching tab or no canvas pane");
            return;
        };
        self.deliver_press(ax, ay, MouseButton::Left);
        self.deliver_move((ax + bx) / 2.0, (ay + by) / 2.0);
        self.deliver_move(bx, by);
        self.deliver_release(bx, by, MouseButton::Left);
    }

    /// Handle a dropped file at window `(x, y)` (the unrunged deletion-matrix
    /// row): a decodable IMAGE over a canvas node textures that node's sprite
    /// face; anything else becomes a node (a `file://` address through the
    /// ordinary open path). Decode is port work (file IO), so it happens here
    /// and only the typed result lowers through the spine. Shared by winit's
    /// `DroppedFile` and the scenario's `drop-file` (one description, two
    /// runners).
    pub(super) fn drop_file(&mut self, x: f32, y: f32, path: &std::path::Path) {
        // The node under the drop, if the drop is over the canvas surface.
        let target = {
            let plan = self.surface_plan();
            plan.iter()
                .find(|s| matches!(s.kind, crate::surface::SurfaceKind::Canvas))
                .filter(|s| s.rect.contains(x, y))
                .and_then(|s| {
                    let (lx, ly) = s.rect.to_local(x, y);
                    self.app.canvas.node_at_screen(lx, ly)
                })
        };
        if let Some(member) = target
            && let Some((data_uri, hull)) = decode_sprite(path)
        {
            self.act(Action::SetNodeSprite { member, data_uri, hull });
            return;
        }
        // A dropped .lua (a control script) or .wasm (an `app-core` component)
        // is a pack: stage the denizen install and surface the VISIBLE grant
        // review with its ring profile (participant gate B1/B3). Nothing is
        // minted, and no grant exists, until the palette's Confirm commits.
        if path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("lua") || e.eq_ignore_ascii_case("wasm"))
        {
            self.act(Action::InstallDenizen {
                path: path.display().to_string(),
            });
            return;
        }
        // Not an image over a node: the file becomes a node. Forward slashes
        // so the address is stable across platforms.
        let url = format!("file:///{}", path.display().to_string().replace('\\', "/"));
        self.act(Action::OpenAddress(url));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The content scroll-key mapping: the page-scroll keys map, Space pages
    /// (down, or up with Shift — the browser convention), and a non-scroll key
    /// declines so the caller lets it fall through to the Action path.
    #[test]
    fn content_scroll_keys_map_page_navigation() {
        let named = |n| WinitKey::Named(n);
        assert_eq!(
            content_scroll_key(&named(WinitNamedKey::PageDown), false),
            Some(SessionScrollKey::PageDown)
        );
        assert_eq!(
            content_scroll_key(&named(WinitNamedKey::PageUp), false),
            Some(SessionScrollKey::PageUp)
        );
        assert_eq!(
            content_scroll_key(&named(WinitNamedKey::Home), false),
            Some(SessionScrollKey::Home)
        );
        assert_eq!(
            content_scroll_key(&named(WinitNamedKey::End), false),
            Some(SessionScrollKey::End)
        );
        assert_eq!(
            content_scroll_key(&named(WinitNamedKey::ArrowDown), false),
            Some(SessionScrollKey::LineDown)
        );
        // Space pages down; Shift+Space pages up.
        assert_eq!(
            content_scroll_key(&named(WinitNamedKey::Space), false),
            Some(SessionScrollKey::PageDown)
        );
        assert_eq!(
            content_scroll_key(&named(WinitNamedKey::Space), true),
            Some(SessionScrollKey::PageUp)
        );
        // A non-scroll key is not a content-scroll key: it must fall through to
        // become an Action (e.g. Delete forgets the node).
        assert_eq!(content_scroll_key(&named(WinitNamedKey::Delete), false), None);
        assert_eq!(
            content_scroll_key(&WinitKey::Character("i".into()), false),
            None
        );
    }
}
