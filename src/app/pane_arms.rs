//! Pane-ring arms: arrangement edits.
//!
//! Every one lands on a leaf in whichever space holds it, primary or lens, so
//! the same op works in any window and persists with that space.

use uuid::Uuid;

use crate::action::{Action, Effect, PaneKind, SpaceRef};
use crate::observe::AppEvent;
use crate::panes::{GraphId, InsertSide, PaneContent, PaneId, PaneNode};
use super::pane_content;

use super::App;

impl App {
    pub(super) fn tear_out_active_pane(&mut self) -> Vec<Effect> {
            let Some(active) = self.active_pane else {
                return vec![Effect::Redraw];
            };
            // The pane leaves whichever window's tree holds it (a lens
            // pane tears out onward, not just primary panes out).
            let Some(source) = self.space_of(active) else {
                return vec![Effect::Redraw];
            };
            // Read the leaf wholesale (id + content + graph binding), then
            // remove it from its source tree.
            let Some(layout) = self.space_mut(source) else {
                return vec![Effect::Redraw];
            };
            let Some((pane_id, content, graph_id)) = layout
                .iter_leaves()
                .find(|(id, _, _)| *id == active)
                .map(|(id, c, g)| (id, c.clone(), g))
            else {
                return vec![Effect::Redraw];
            };
            let Some(path) = crate::pane::path_of(layout, active) else {
                return vec![Effect::Redraw];
            };
            if !layout.close_leaf(&path) {
                return vec![Effect::Redraw];
            }
            if self.maximized == Some(active) {
                self.maximized = None;
            }
            let mut effects = self.land_leaf_in_lens(
                PaneNode::Leaf {
                    pane_id,
                    content: content.clone(),
                    graph_id,
                },
                Some(source),
            );
            // The moved pane STAYS active: it kept living (same runner,
            // same id), so pane-anchored ops now follow it to its new
            // window — summon-beside lands there, the divider op reweights
            // there (the lens-frisket-ops receipt's hinge).
            self.active_pane = Some(pane_id);
            self.events
                .push(AppEvent::PaneTornOut(content.tag().to_string()));
            // The move is durable structure in TWO trees; persist it (the
            // lens-window sidecar is what makes the window survive a
            // restart).
            effects.push(Effect::SaveSession);
            effects.push(Effect::Redraw);
            effects
    }

    pub(super) fn summon_pane(&mut self, kind: PaneKind) -> Vec<Effect> {
            let content = pane_content(kind);
            let id = PaneId(self.next_pane_id);
            // Anchor on the active pane IN ITS OWN SPACE (a pane torn out
            // to a lens summons its neighbors there — the window as pane
            // host), else the primary Orrery (graph) leaf — meerkat's
            // fixed Right-split off the graph pane, generalized.
            let (space, anchor) = match self
                .active_pane
                .and_then(|a| self.space_of(a).map(|s| (s, a)))
            {
                Some((s, a)) => (s, Some(a)),
                None => (
                    SpaceRef::Primary,
                    self.frisket
                        .iter_leaves()
                        .find(|(_, c, _)| matches!(c, PaneContent::Orrery))
                        .map(|(id, _, _)| id),
                ),
            };
            let Some(layout) = self.space_mut(space) else {
                return vec![Effect::Redraw];
            };
            let anchor_path = anchor
                .and_then(|a| crate::pane::path_of(layout, a))
                .unwrap_or_default();
            let new_leaf = PaneNode::Leaf {
                pane_id: id,
                content,
                graph_id: GraphId::nil(),
            };
            if layout.summon_leaf(&anchor_path, InsertSide::Right, new_leaf) {
                self.next_pane_id += 1;
                self.active_pane = Some(id);
                self.events.push(AppEvent::PaneSummoned(kind.label()));
                vec![Effect::SaveSession, Effect::Redraw]
            } else {
                vec![Effect::Redraw]
            }
    }

    pub(super) fn move_pane_section(&mut self, pane: PaneId, section: String, delta: i32) -> Vec<Effect> {
            // Order IS the config's order, so a move is the same leaf edit
            // as add/remove. Clamped at the ends: a stack has a top and a
            // bottom, and silently wrapping would be a surprise.
            let Some(space) = self.space_of(pane) else {
                return vec![Effect::Redraw];
            };
            let Some(layout) = self.space_mut(space) else {
                return vec![Effect::Redraw];
            };
            let mut moved = false;
            if let Some(cfg) = layout.content_mut(pane).and_then(|c| c.composition_mut())
                && let Some(from) = cfg.sections.iter().position(|s| s == &section)
            {
                let to = (from as i32 + delta).clamp(0, cfg.sections.len() as i32 - 1)
                    as usize;
                if to != from {
                    let id = cfg.sections.remove(from);
                    cfg.sections.insert(to, id);
                    moved = true;
                }
            }
            if moved {
                self.events.push(AppEvent::PaneSectionMoved(section));
                vec![Effect::SaveSession, Effect::Redraw]
            } else {
                vec![Effect::Redraw]
            }
    }

    pub(super) fn tear_out_tile(&mut self, member: Uuid) -> Vec<Effect> {
            if !self.workbench.close_tile(member) {
                return vec![Effect::Redraw];
            }
            let pane_id = PaneId(self.next_pane_id);
            self.next_pane_id += 1;
            let mut effects = self.land_leaf_in_lens(
                PaneNode::Leaf {
                    pane_id,
                    content: PaneContent::Tile(member),
                    graph_id: GraphId::nil(),
                },
                None,
            );
            self.active_pane = Some(pane_id);
            let label = self
                .canvas
                .graph()
                .nodes()
                .find(|(_, n)| n.id == member)
                .map(|(_, n)| n.url().to_string())
                .unwrap_or_default();
            self.events.push(AppEvent::TileTornOut(label));
            effects.push(Effect::SaveSession);
            effects.push(Effect::Redraw);
            effects
    }

    pub(super) fn open_in_workbench(&mut self) -> Vec<Effect> {
            let Some(target) = self
                .canvas
                .focused_member()
                .zip(self.canvas.focused_url().map(str::to_string))
            else {
                return Vec::new();
            };
            let (member, url) = target;
            self.workbench.ensure_tiled();
            self.workbench.open_tile(member);
            self.events.push(AppEvent::WorkbenchTileOpened(url.clone()));
            let mut effects = Vec::new();
            let has_pane = self
                .frisket
                .iter_leaves()
                .any(|(_, c, _)| matches!(c, PaneContent::Workbench));
            if !has_pane {
                effects.extend(self.update(Action::SummonPane(PaneKind::Workbench)));
            }
            // A tile wants live content; spawn it unless it already has
            // some (live or in flight). Failure surfaces as ever.
            if self.content.flip_spawns(member) {
                self.content.note_requested(member);
                self.events.push(AppEvent::ContentState {
                    node: member,
                    state: "requested".to_string(),
                });
                effects.push(Effect::SpawnContent { node: member, url });
            }
            effects.push(Effect::SaveSession);
            effects.push(Effect::Redraw);
            effects
    }

    pub(super) fn toggle_pane_section(&mut self, pane: PaneId, section: String) -> Vec<Effect> {
            // Mutate the pane's OWN leaf, in whichever space holds it, so
            // the composition persists with frame.json and travels with a
            // tear-out. Unknown pane / non-composable content: honest no-op.
            let Some(space) = self.space_of(pane) else {
                return vec![Effect::Redraw];
            };
            let Some(layout) = self.space_mut(space) else {
                return vec![Effect::Redraw];
            };
            let mut changed = None;
            if let Some(cfg) = layout.content_mut(pane).and_then(|c| c.composition_mut()) {
                if let Some(pos) = cfg.sections.iter().position(|s| s == &section) {
                    cfg.sections.remove(pos);
                    changed = Some(false);
                } else {
                    cfg.sections.push(section.clone());
                    changed = Some(true);
                }
            }
            match changed {
                Some(added) => {
                    self.events
                        .push(AppEvent::PaneSectionToggled { section, added });
                    vec![Effect::SaveSession, Effect::Redraw]
                }
                None => vec![Effect::Redraw],
            }
    }

    pub(super) fn close_active_pane(&mut self) -> Vec<Effect> {
            // The canvas (no active pane) has nothing to close. The op
            // lands in whichever window's tree holds the pane.
            let Some((active, space)) = self
                .active_pane
                .and_then(|a| self.space_of(a).map(|s| (a, s)))
            else {
                return vec![Effect::Redraw];
            };
            let Some(layout) = self.space_mut(space) else {
                return vec![Effect::Redraw];
            };
            let Some(path) = crate::pane::path_of(layout, active) else {
                return vec![Effect::Redraw];
            };
            if layout.close_leaf(&path) {
                if self.maximized == Some(active) {
                    self.maximized = None;
                }
                self.active_pane = None;
                self.events.push(AppEvent::PaneClosed);
                vec![Effect::SaveSession, Effect::Redraw]
            } else {
                vec![Effect::Redraw]
            }
    }
}
