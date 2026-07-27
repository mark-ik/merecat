//! The effect runner and session save: the one place effects meet ports.
//!
//! `App::update` returns effects; every port call the app asked for happens
//! here, and every answer comes back as a typed `Update`. Nothing else in the
//! shell talks to a port.

use std::sync::mpsc::Receiver;

use fetch::{FetchCommand, FetchUpdate};
use inker::{SessionClick, SessionSpawnRequest};

use crate::action::{Action, Effect, Update};
use crate::browse;
use crate::panes::PaneContent;
use crate::session;

use super::Shell;

impl Shell {
    /// The effect runner: the one place effects meet ports.
    pub(super) fn run_effects(&mut self, effects: Vec<Effect>) {
        for effect in effects {
            if let Some(command) = browse::fetch_command_for(&effect, &mut self.pending_fetches) {
                self.fetch_handle.command(command);
                continue;
            }
            match effect {
                Effect::SaveSession => self.save_session(),
                Effect::StoreImage { hex, bytes } => {
                    session::save_image_blob(&self.app.session_dir(), &hex, &bytes);
                }
                // The bin port: stage the record; the actor answers with the
                // refreshed list (folded on the next wake).
                Effect::RecordDeleted { record } => {
                    self.bin_handle
                        .command(crate::recycle::BinCommand::Record(record));
                }
                Effect::EmptyRecycleBin => {
                    self.bin_handle.command(crate::recycle::BinCommand::Empty);
                }
                // The session switch (rung 6's second half). Ordering is the
                // point of this being an EFFECT: the departing session saves
                // under ITS directory while it is still the live state, the
                // ports tear down (live document sessions die with their
                // windows; lens windows close), and only then does the app
                // adopt the target — whose own effects (content respawns,
                // window reopens) run through the same loop.
                // The close path (overmap O3): release the bin store (its
                // open files block the dir rename on Windows), trash the
                // closing session's directory whole, then adopt the target
                // WITHOUT the departing save — a post-trash save would
                // resurrect the closed session as a zombie directory.
                Effect::TrashSession { closing, next } => {
                    let (ack_tx, ack_rx) = std::sync::mpsc::sync_channel(1);
                    self.bin_handle
                        .command(crate::recycle::BinCommand::Release(ack_tx));
                    if ack_rx
                        .recv_timeout(std::time::Duration::from_millis(1500))
                        .is_err()
                    {
                        tracing::warn!(
                            "bin release ack timed out; attempting the trash move anyway"
                        );
                    }
                    self.app.apply_trash(closing);
                    self.content_sessions.clear();
                    self.lens_windows.clear();
                    self.pending_lens_capture = None;
                    self.lens_divider_drag = None;
                    self.pending_windows.clear();
                    let fx = self.app.adopt_session(next);
                    self.bin_handle.command(crate::recycle::BinCommand::Reopen(
                        crate::recycle::bin_dir(&self.app.session_dir()),
                    ));
                    self.run_effects(fx);
                    self.request_redraw();
                }
                Effect::SwitchSession { id } => {
                    self.save_session();
                    self.content_sessions.clear();
                    self.lens_windows.clear();
                    self.pending_lens_capture = None;
                    self.lens_divider_drag = None;
                    self.pending_windows.clear();
                    let fx = self.app.adopt_session(id);
                    // Re-point the bin actor at the adopted session's store;
                    // it answers with THAT bin's list (the app cleared its
                    // mirror in adopt_session).
                    self.bin_handle.command(crate::recycle::BinCommand::Reopen(
                        crate::recycle::bin_dir(&self.app.session_dir()),
                    ));
                    self.run_effects(fx);
                    self.request_redraw();
                }
                // The content port (rung 4, live since genet-documents
                // landed): route the address to an engine id, spawn through
                // the registry, hold the session keyed by node id. Every
                // failure — unroutable id, spawn error — surfaces as
                // ContentFailed; a Requested node never silently spins.
                Effect::SpawnContent { node, url } => {
                    let request = inker::EngineRouteRequest {
                        workspace_id: inker::WorkspaceRouteId::new("turnstone"),
                        view: None,
                        node: None,
                        address: url.clone(),
                        content_type: None,
                        // The settings row: a sidecar viewer override pins the
                        // route, so a respawn lands on the chosen lane.
                        pinned_engine: self
                            .app
                            .browser
                            .get(node)
                            .and_then(|b| b.viewer_override.clone()),
                    };
                    let decision = self.route_policy.route(&request);
                    let spawn = SessionSpawnRequest::new(&url)
                        .with_viewport(self.width.max(1), self.height.max(1));
                    let update = match self.content_engines.spawn(&decision.engine_id, &spawn) {
                        Ok(session) => {
                            tracing::info!(%node, %url, engine = %decision.engine_id, "content session live");
                            // Mirror the spawn-time facts into app truth (the
                            // adapter conversion): the engine id plus the
                            // structural read through the trait accessor —
                            // None stays None (a lane without introspection
                            // is reported, not synthesized).
                            let facts = crate::content::ContentFacts {
                                engine: decision.engine_id.clone(),
                                structure: session.inspect().map(|r| {
                                    crate::content::StructureFacts {
                                        title: r.title,
                                        headings: r.headings.len(),
                                        links: r.links.len(),
                                        outline: r
                                            .outline
                                            .into_iter()
                                            .map(|e| crate::content::OutlineFact {
                                                depth: e.depth,
                                                role: e.role,
                                                name: e.name,
                                            })
                                            .collect(),
                                    }
                                }),
                            };
                            self.content_sessions.insert(node, session);
                            Update::ContentSpawned {
                                node,
                                facts: Some(facts),
                            }
                        }
                        Err(err) => {
                            tracing::warn!(%node, %url, engine = %decision.engine_id, %err, "content spawn failed");
                            Update::ContentFailed {
                                node,
                                error: format!("{} ({})", err, decision.engine_id),
                            }
                        }
                    };
                    let effects = self.app.apply_update(update);
                    self.run_effects(effects);
                }
                Effect::CloseContent { node } => {
                    if self.content_sessions.remove(&node).is_some() {
                        tracing::info!(%node, "content session closed");
                    }
                }
                Effect::Redraw => self.request_redraw(),
                // Window creation needs the ActiveEventLoop; note the request
                // and let the event handler in scope drain it.
                Effect::OpenWindow { ordinal } => self.pending_windows.push(ordinal),
                // Fetch-shaped effects were consumed above.
                Effect::FetchPage { .. } | Effect::FetchFavicon { .. } => {}
            }
        }
    }

    /// Persist the live session's whole sidecar set under ITS directory
    /// (`sessions/<id>/`) — the SaveSession effect's body, shared by the
    /// session switch (which must save the DEPARTING session first).
    pub(super) fn save_session(&mut self) {
        let sdir = self.app.session_dir();
        session::save_session_graph(&sdir, self.app.canvas.graph());
        let swept = session::gc_orphan_image_blobs(&sdir, self.app.canvas.graph());
        if swept > 0 {
            tracing::info!(swept, "reclaimed orphaned session image blobs");
        }
        // The pane layout persists to frame.json alongside the graph
        // (rung 5 slice C), so summon/close/divider survive a restart.
        session::save_frisket_layout(&sdir, &self.app.frisket);
        // The workbench tiling persists as platen's canonical pair
        // (rung 5 slice E), so tiles/stacks/fractions survive too.
        session::save_workbench(&sdir, &self.app.workbench);
        // The lens-window spaces (rung 7 depth): torn-out panes
        // survive a restart as windows again.
        session::save_lens_spaces(&sdir, &self.app.lenses);
        // Browser state (rung 6): content-on refreshed from live truth, so a
        // restart respawns what was showing; then the whole live state lands
        // in the facet store (arrangement.* + scene.* + web.*) via the shared
        // refresh (the fork's facet-carry reads the same refreshed store).
        self.app.refresh_browser_states();
        self.app.refresh_facets();
        if let Err(error) = crate::content_classes::reconcile(&mut self.app.canvas) {
            tracing::warn!(%error, "content-class reconciliation failed");
        }
        session::save_node_facets(&sdir, self.app.canvas.facets());
        if let Some(score) = self.app.canvas.projection_score() {
            session::save_projection_score(&sdir, score);
        }
        // Stamp a derived display name the first time the session has content
        // to name it after (unset -> "Example Domain"), then bump recency so
        // the switcher orders by last-used. Derive before the mutable borrow.
        let id = self.app.session_id;
        let derived = self
            .app
            .sessions
            .get(id)
            .is_some_and(|m| m.display_name.is_none())
            .then(|| self.app.derive_session_name())
            .flatten();
        if self.app.sessions.update(id, |m| {
            if m.display_name.is_none()
                && let Some(name) = derived.clone()
            {
                m.display_name = Some(name);
            }
            m.touch();
        }) {
            let _ = self.app.sessions.flush_dirty();
        }
    }
}
