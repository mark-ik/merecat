//! Merecat's core state and the two update functions — the middle of the
//! spine: `Action -> update -> Effects` and `Update -> apply_update ->
//! Effects`. Holds data, never handles: the ports (actors, stores, the
//! window) live in the shell, which runs the effects this module returns.

use std::path::PathBuf;

use mere::canvas::Canvas;

use crate::action::{Action, Effect, Update};
use crate::ui::{OmnibarState, Suggestion, normalize_address, recompute_suggestions};
use crate::{browse, session};

/// The application state: the hosted canvas (which owns the graph), the
/// chrome state, and where the session persists.
pub struct App {
    pub canvas: Canvas,
    /// The summonable omnibar (rung 3): find over graph truth, go through
    /// OpenAddress, `>` for the actions lane.
    pub omnibar: OmnibarState,
    /// The per-user data root; the session graph persists at its flat
    /// `graph.json` (single-session shape; sessions/<id>/ arrives with
    /// multi-session).
    pub data_root: PathBuf,
}

impl App {
    /// Boot the app state: restore the persisted session graph if one exists,
    /// else seed from the launch address, else show the sample graph. Returns
    /// the state plus the boot effects (the seed address's fetch).
    pub fn boot(address: Option<&str>) -> (Self, Vec<Effect>) {
        let data_root = session::default_merecat_root();
        let _ = std::fs::create_dir_all(&data_root);
        let restored = session::load_session_graph(&data_root);
        let mut first_run = false;
        let mut canvas = match (restored, address) {
            (Some(graph), _) => Canvas::with_graph(graph),
            (None, Some(url)) => {
                tracing::info!(%url, "fresh graph seeded from the address");
                Canvas::new()
            }
            (None, None) => {
                tracing::info!("no session graph; starting on the sample graph");
                first_run = true;
                Canvas::with_sample_graph()
            }
        };
        let mut effects = Vec::new();
        if let Some(url) = address {
            canvas.visit(url);
            if fetch::is_fetchable(url) {
                effects.push(Effect::FetchPage(url.to_string()));
            }
        }
        // A bare FIRST launch opens the omnibar by itself, so the app is
        // discoverable without documentation; a bare relaunch restores the
        // canvas quietly (Ctrl+L / Ctrl+K summon).
        let mut omnibar = OmnibarState::default();
        if first_run {
            omnibar.open = true;
            recompute_suggestions(&mut omnibar, &canvas);
        }
        (
            Self {
                canvas,
                omnibar,
                data_root,
            },
            effects,
        )
    }

    /// Consume one app intent. Never blocks; anything slow leaves as an effect.
    pub fn update(&mut self, action: Action) -> Vec<Effect> {
        match action {
            Action::OpenAddress(url) => {
                self.canvas.visit(&url);
                let mut effects = vec![Effect::Redraw];
                if fetch::is_fetchable(&url) {
                    effects.push(Effect::FetchPage(url));
                }
                effects
            }
            Action::ReseedLayout => {
                if self.canvas.reseed() {
                    vec![Effect::Redraw]
                } else {
                    Vec::new()
                }
            }
            Action::ToggleIsometric => {
                let on = !self.canvas.is_isometric();
                self.canvas.set_isometric(on);
                vec![Effect::Redraw]
            }
            Action::OrbitBy(delta) => {
                self.canvas.orbit_by(delta);
                vec![Effect::Redraw]
            }
            Action::TiltBy(delta) => {
                self.canvas.set_tilt(self.canvas.tilt() + delta);
                vec![Effect::Redraw]
            }
            Action::ToggleHeightByDegree => {
                let on = !self.canvas.height_by_degree();
                self.canvas.set_height_by_degree(on);
                vec![Effect::Redraw]
            }
            Action::SaveSession => vec![Effect::SaveSession],
            Action::OmnibarOpen { command } => {
                self.omnibar.open = true;
                self.omnibar.text = if command { ">".to_string() } else { String::new() };
                self.omnibar.selected = 0;
                recompute_suggestions(&mut self.omnibar, &self.canvas);
                vec![Effect::Redraw]
            }
            Action::OmnibarClose => {
                self.omnibar = OmnibarState::default();
                vec![Effect::Redraw]
            }
            Action::OmnibarChar(c) => {
                self.omnibar.text.push(c);
                self.omnibar.selected = 0;
                recompute_suggestions(&mut self.omnibar, &self.canvas);
                vec![Effect::Redraw]
            }
            Action::OmnibarBackspace => {
                self.omnibar.text.pop();
                self.omnibar.selected = 0;
                recompute_suggestions(&mut self.omnibar, &self.canvas);
                vec![Effect::Redraw]
            }
            Action::OmnibarMove(delta) => {
                let len = self.omnibar.suggestions.len();
                if len > 0 {
                    let cur = self.omnibar.selected as i32;
                    self.omnibar.selected = (cur + delta).rem_euclid(len as i32) as usize;
                }
                vec![Effect::Redraw]
            }
            Action::OmnibarCommit => {
                let committed = self.omnibar.selection().cloned().or_else(|| {
                    normalize_address(self.omnibar.text.trim())
                        .map(|url| Suggestion::Go { url })
                });
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
                    Some(Suggestion::Hint(_)) | None => vec![Effect::Redraw],
                };
                self.omnibar = OmnibarState::default();
                effects.push(Effect::Redraw);
                effects
            }
        }
    }

    /// Fold one typed service answer into state.
    pub fn apply_update(&mut self, update: Update) -> Vec<Effect> {
        match update {
            Update::PageFetched { url, result } => {
                browse::apply_page(&mut self.canvas, url, result)
            }
            Update::FaviconFetched { owner_url, bytes } => {
                browse::apply_favicon(&mut self.canvas, &owner_url, &bytes)
            }
        }
    }
}
