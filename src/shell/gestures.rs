//! Drag gestures and file drops: the multi-event interactions that resolve on
//! release rather than on press.
//!
//! A workbench tab dragged onto another cell stacks into it, dragged to an
//! edge splits beside it, dragged outside tears out; released where it began
//! it is a click. A dropped image textures the node under it, anything else
//! becomes a node.

use winit::event::MouseButton;

use crate::action::Action;
use crate::panes::PaneContent;

use super::{Shell, decode_sprite};

impl Shell {
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
