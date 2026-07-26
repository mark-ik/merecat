//! Node and view arms: what a node IS and how it is shown.
//!
//! Deleting stages into the recycle bin and recovery restores the ORIGINAL
//! id, which is the whole identity contract the bin exists for.

use uuid::Uuid;

use crate::action::{Action, Effect};
use crate::observe::AppEvent;
use crate::panes::PaneContent;
use crate::surface::FocusTarget;
use crate::ui::{OmnibarState, Suggestion, normalize_address, recompute_suggestions};

use super::App;

impl App {
    pub(super) fn delete_focused_node(&mut self) -> Vec<Effect> {
            // Build the bin record off the LIVING node (identity, url,
            // title, tags — everything recovery restores), then drop the
            // node and reap what hung off it: the live content session
            // and any workbench tile. The record stages through the bin
            // port (Effect::RecordDeleted); the actor answers with the
            // refreshed list, so `removed` mirrors the store, never a
            // hand-kept copy.
            let record = self.canvas.focused_member().and_then(|m| {
                let graph = self.canvas.graph();
                let (key, node) = graph.get_node_by_id(m)?;
                let title = node.title.trim();
                // The node's whole character rides the tombstone: its
                // borne world (by id) and its facet bundle, so recovery
                // restores residency/arrangement/web state, not just
                // identity.
                let facets = self.facets.facets_of(&m).map(|f| {
                    serde_json::Value::Object(
                        f.iter()
                            .map(|(id, value)| (id.as_str().to_string(), value.clone()))
                            .collect(),
                    )
                });
                Some(crate::action::RemovedRecord {
                    node_id: node.id,
                    url: node.url().to_string(),
                    title: (!title.is_empty() && title != node.url())
                        .then(|| title.to_string()),
                    tags: graph
                        .node_tags(key)
                        .map(|t| t.iter().cloned().collect())
                        .unwrap_or_default(),
                    deleted_at_ms: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as u64)
                        .unwrap_or(0),
                    nested: node.nested.as_ref().map(|log| log.as_str().to_string()),
                    facets,
                })
            });
            let Some(record) = record else {
                return vec![Effect::Redraw];
            };
            // Archive-never-orphan: the world's file moves to the archive
            // slot BEFORE the bearing node leaves; a failed archive
            // aborts the delete (the node stays, nothing is lost).
            if let Some(log_id) = &record.nested
                && let Err(err) = crate::denizen::archive_world(&self.session_dir(), log_id)
            {
                tracing::warn!(%err, log_id, "world archive failed; delete aborted");
                return vec![Effect::Redraw];
            }
            let Some(member) = self.canvas.remove_focused() else {
                // The node did not leave after all: put the world back.
                if let Some(log_id) = &record.nested {
                    let _ = crate::denizen::unarchive_world(&self.session_dir(), log_id);
                }
                return vec![Effect::Redraw];
            };
            // The record is the archive now: the live facets go, and a
            // denizen's runtime entry goes with its node.
            self.facets.remove_node(&member);
            if self.denizens.residents.remove(&member).is_some() {
                let sdir = self.session_dir();
                self.denizens = crate::denizen::rebuild(
                    &self.facets,
                    self.canvas.graph(),
                    &sdir,
                    self.identity.as_ref(),
                );
            }
            self.workbench.close_tile(member);
            self.events.push(AppEvent::NodeRemoved(record.url.clone()));
            vec![
                Effect::RecordDeleted { record },
                Effect::CloseContent { node: member },
                Effect::SaveSession,
                Effect::Redraw,
            ]
    }

    pub(super) fn recover_deleted_node(&mut self, id: Uuid) -> Vec<Effect> {
            // Recover from the bin mirror BY IDENTITY: the node re-mints
            // under its ORIGINAL id with its recorded title/tags (the
            // canvas guards idempotency), gets selected + centered, joins
            // the visit history, and refetches. The bin record stays in
            // the store (append-only until athanor's pass); the Trail's
            // Removed section derives it away because the node is present
            // again.
            let Some(record) = self.removed.iter().find(|r| r.node_id == id).cloned() else {
                return vec![Effect::Redraw];
            };
            let member = self.canvas.recover_node(
                record.node_id,
                &record.url,
                record.title.as_deref(),
                &record.tags,
            );
            // Restore the node's character from the tombstone: the facet
            // bundle whole, then the borne world (file back to the live
            // slot, pointer re-borne through the spine), then the denizen
            // runtime so a recovered resident resides again.
            if let Some(serde_json::Value::Object(map)) = &record.facets {
                for (facet_id, value) in map {
                    let _ = self.facets.set(
                        member,
                        chartulary::FacetId::new(facet_id.as_str()),
                        value.clone(),
                        &chartulary::AcceptAll,
                    );
                }
            }
            if let Some(log_id) = &record.nested {
                let sdir = self.session_dir();
                if let Err(err) = crate::denizen::unarchive_world(&sdir, log_id) {
                    tracing::warn!(%err, log_id, "world unarchive failed; recovering empty");
                }
                let _ = self.canvas.set_node_nested_for(
                    member,
                    Some(mere::kernel::graph::LogId::new(log_id.clone())),
                );
                self.denizens =
                    crate::denizen::rebuild(
                        &self.facets,
                        self.canvas.graph(),
                        &sdir,
                        self.identity.as_ref(),
                    );
            }
            self.canvas.center_on_selected();
            self.history.visit(record.url.clone());
            self.events
                .push(AppEvent::NodeRecovered(record.url.clone()));
            let mut effects = vec![Effect::SaveSession, Effect::Redraw];
            if fetch::is_fetchable(&record.url) {
                effects.push(Effect::FetchPage {
                    node: member,
                    url: record.url.clone(),
                });
            }
            effects
    }

    pub(super) fn set_viewer_override(&mut self, member: Uuid, viewer: Option<String>) -> Vec<Effect> {
            self.browser.entry(member).viewer_override = viewer.clone();
            self.events.push(AppEvent::ViewerChanged {
                node: member,
                viewer: viewer.clone().unwrap_or_else(|| "auto".to_string()),
            });
            let mut effects = Vec::new();
            // Live (or in-flight) content respawns through the now-pinned
            // route, so the setting is seen applying (the Reload shape).
            if matches!(
                self.content.get(member),
                Some(
                    crate::content::NodeContent::Live | crate::content::NodeContent::Requested
                )
            ) && let Some(url) = self
                .canvas
                .graph()
                .nodes()
                .find(|(_, n)| n.id == member)
                .map(|(_, n)| n.url().to_string())
            {
                self.content.note_requested(member);
                self.events.push(AppEvent::ContentState {
                    node: member,
                    state: "requested".to_string(),
                });
                effects.push(Effect::CloseContent { node: member });
                effects.push(Effect::SpawnContent { node: member, url });
            }
            effects.push(Effect::SaveSession);
            effects.push(Effect::Redraw);
            effects
    }

    pub(super) fn toggle_node_content(&mut self) -> Vec<Effect> {
            // The flip targets the focused node; no focus, no-op (the
            // caption chip tells the user what would flip).
            // Resolve the node by MEMBER, not by URL round-trip: two
            // nodes may share a URL (the sample graph + an open), and
            // get_node_by_url picks arbitrarily between them.
            let Some(target) = self
                .canvas
                .focused_member()
                .zip(self.canvas.focused_url().map(str::to_string))
            else {
                return Vec::new();
            };
            let (node, url) = target;
            if self.content.flip_spawns(node) {
                self.content.note_requested(node);
                self.events.push(AppEvent::ContentState {
                    node,
                    state: "requested".to_string(),
                });
                vec![Effect::SpawnContent { node, url }, Effect::Redraw]
            } else {
                self.content.note_closed(node);
                self.events.push(AppEvent::ContentState {
                    node,
                    state: "closed".to_string(),
                });
                vec![Effect::CloseContent { node }, Effect::Redraw]
            }
    }

    pub(super) fn reload_focused(&mut self) -> Vec<Effect> {
            let Some(target) = self
                .canvas
                .focused_member()
                .zip(self.canvas.focused_url().map(str::to_string))
            else {
                return vec![Effect::Redraw];
            };
            let (node, url) = target;
            self.events.push(AppEvent::Reloaded(url.clone()));
            let mut effects = Vec::new();
            if fetch::is_fetchable(&url) {
                effects.push(Effect::FetchPage {
                    node,
                    url: url.clone(),
                });
            }
            // A live (or in-flight) session respawns fresh; a node
            // without content stays without (reload is not a spawn).
            if matches!(
                self.content.get(node),
                Some(
                    crate::content::NodeContent::Live | crate::content::NodeContent::Requested
                )
            ) {
                self.content.note_requested(node);
                self.events.push(AppEvent::ContentState {
                    node,
                    state: "requested".to_string(),
                });
                effects.push(Effect::CloseContent { node });
                effects.push(Effect::SpawnContent { node, url });
            }
            effects.push(Effect::Redraw);
            effects
    }

    pub(super) fn commit_omnibar(&mut self) -> Vec<Effect> {
            // Rename mode captures the whole line as the new name and
            // commits it, bypassing the find/go/actions lanes.
            if let crate::ui::OmnibarMode::RenameSession(id) = self.omnibar.mode {
                let name = self.omnibar.text.clone();
                self.omnibar = OmnibarState::default();
                if self.focus == FocusTarget::Chrome {
                    self.focus = FocusTarget::Canvas;
                }
                let mut fx = self.update(Action::RenameSession { id, name });
                fx.push(Effect::Redraw);
                return fx;
            }
            // Commit always ends with the omnibar closed, so chrome hands
            // focus back to the canvas. (A committed OpenAddress may later
            // spawn content; routing focus onto it is slice B.)
            if self.focus == FocusTarget::Chrome {
                self.focus = FocusTarget::Canvas;
            }
            let committed = self.omnibar.selection().cloned().or_else(|| {
                normalize_address(self.omnibar.text.trim()).map(|url| Suggestion::Go { url })
            });
            if let Some(s) = committed.as_ref() {
                self.events
                    .push(AppEvent::OmnibarCommitted(crate::observe::suggestion_line(
                        s,
                    )));
            }
            let mut effects = match committed {
                Some(Suggestion::Node { url, .. }) => {
                    // Find lane: select the existing node; never refetch.
                    self.canvas.select_by_url(&url);
                    vec![Effect::Redraw]
                }
                Some(Suggestion::Go { url }) => {
                    self.omnibar = OmnibarState::default();
                    return {
                        let mut fx = self.update(Action::OpenAddress(url));
                        fx.push(Effect::Redraw);
                        fx
                    };
                }
                Some(Suggestion::Act { action, .. }) => {
                    // The actions lane: the committed registry entry is
                    // an ordinary Action; lower it through the same
                    // spine everything else uses.
                    self.omnibar = OmnibarState::default();
                    return {
                        let mut fx = self.update(action);
                        fx.push(Effect::Redraw);
                        fx
                    };
                }
                Some(Suggestion::Hint(_)) | None => vec![Effect::Redraw],
            };
            self.omnibar = OmnibarState::default();
            effects.push(Effect::Redraw);
            effects
    }
}
