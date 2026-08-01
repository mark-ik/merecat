//! The port answers: every `Update` a port hands back, folded into app truth.
//!
//! The other half of the spine from `update`. Ports never touch state; they
//! answer with one of these and the fold happens here.

use crate::action::{Effect, Update};
use crate::browse;
use crate::observe::AppEvent;

use super::App;

impl App {
    /// Fold one typed service answer into state.
    pub fn apply_update(&mut self, update: Update) -> Vec<Effect> {
        match update {
            Update::PageFetched { node, url, result } => {
                browse::apply_page(&mut self.canvas, node, url, result)
            }
            Update::FaviconFetched {
                node,
                owner_url,
                bytes,
            } => browse::apply_favicon(&mut self.canvas, node, &owner_url, &bytes),
            Update::ContentSpawned { node, facts } => {
                self.content.note_live(node, facts);
                self.events.push(AppEvent::ContentState {
                    node,
                    state: "live".to_string(),
                });
                vec![Effect::Redraw]
            }
            Update::ContentFailed { node, error } => {
                tracing::warn!(%node, %error, "content spawn failed");
                self.events.push(AppEvent::ContentState {
                    node,
                    state: format!("failed: {error}"),
                });
                self.content.note_failed(node, error);
                vec![Effect::Redraw]
            }
            Update::BinListed { records } => {
                // The bin mirror replaces wholesale — the actor's answer IS
                // the store's truth (never merged with a hand-kept copy).
                self.removed = records;
                vec![Effect::Redraw]
            }
            Update::BinFailed { error } => {
                // Loud and attributable: the Removed section going quiet
                // because the store broke must be visible divergence, not an
                // empty list pretending nothing was deleted.
                tracing::warn!(%error, "recycle bin failed");
                self.events.push(AppEvent::BinFailed(error));
                vec![Effect::Redraw]
            }
            Update::PlaceCommandDone {
                session,
                generation,
                request: _,
                result,
            } => {
                if session != self.session_id || self.place.generation() != Some(generation) {
                    return Vec::new();
                }
                match result {
                    // Authoring changed what the place projects, so the answer
                    // carries the re-folded snapshot rather than leaving the
                    // app with a stale view of state it just changed.
                    Ok(snapshot) => {
                        if let Some(binding) = self.place.binding().cloned() {
                            self.place = crate::place::PlaceState::Offline {
                                binding,
                                generation,
                                snapshot,
                            };
                        }
                    }
                    // A refusal changes no state. The place is still open and
                    // still whatever it was; the command simply did not happen.
                    Err(error) => {
                        tracing::warn!(%error, "place command refused");
                        self.events.push(AppEvent::PlaceRefused(error));
                    }
                }
                vec![Effect::Redraw]
            }
            Update::PlaceJoined {
                session,
                generation,
                result,
            } => {
                if session != self.session_id || self.place.generation() != Some(generation) {
                    return Vec::new();
                }
                self.place = match result {
                    Ok((binding, snapshot)) => crate::place::PlaceState::Offline {
                        binding,
                        generation,
                        snapshot,
                    },
                    // A refused invitation is not a degraded place, it is no
                    // place. Landing in `Degraded` would leave app state
                    // holding a binding admission never granted.
                    Err(error) => {
                        tracing::warn!(%error, "invitation refused");
                        crate::place::PlaceState::Failed { error }
                    }
                };
                vec![Effect::Redraw]
            }
            Update::PlaceOpened {
                session,
                generation,
                result,
            } => {
                if session != self.session_id || self.place.generation() != Some(generation) {
                    return Vec::new();
                }
                let Some(binding) = self.place.binding().cloned() else {
                    return Vec::new();
                };
                self.place = match result {
                    Ok(snapshot) => crate::place::PlaceState::Offline {
                        binding,
                        generation,
                        snapshot,
                    },
                    Err(error) => {
                        tracing::warn!(%error, "place cache failed to open");
                        crate::place::PlaceState::Degraded {
                            binding,
                            generation,
                            error,
                        }
                    }
                };
                vec![Effect::Redraw]
            }
        }
    }
}
