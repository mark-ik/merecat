//! Merecat's core state and the two update functions — the middle of the
//! spine: `Action -> update -> Effects` and `Update -> apply_update ->
//! Effects`. Holds data, never handles: the ports (actors, stores, the
//! window) live in the shell, which runs the effects this module returns.

use std::path::PathBuf;

use mere::canvas::Canvas;

use crate::action::{Action, Effect, Update};
use crate::ui::{OmnibarState, Suggestion, normalize_address, recompute_suggestions};
use crate::{browse, session};

/// The at-rest "where am I" caption: the focused node's display label (and
/// host, when it adds information), or `None` with nothing focused.
pub fn focused_caption(canvas: &Canvas) -> Option<String> {
    let url = canvas.focused_url()?.to_string();
    let graph = canvas.graph();
    let (key, node) = graph.get_node_by_url(&url)?;
    let label = graph.node_display_label(key);
    match node.cached_host.as_deref() {
        Some(host) if !label.contains(host) => Some(format!("{label}  \u{00b7}  {host}")),
        _ => Some(label),
    }
}

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
                self.omnibar = OmnibarState {
                    open: true,
                    text: if command { ">".to_string() } else { String::new() },
                    ..OmnibarState::default()
                };
                self.omnibar.cursor = self.omnibar.text.len();
                recompute_suggestions(&mut self.omnibar, &self.canvas);
                vec![Effect::Redraw]
            }
            Action::OmnibarClose => {
                self.omnibar = OmnibarState::default();
                vec![Effect::Redraw]
            }
            Action::OmnibarChar(c) => {
                self.omnibar.insert_str(c.encode_utf8(&mut [0u8; 4]));
                self.omnibar.selected = 0;
                recompute_suggestions(&mut self.omnibar, &self.canvas);
                vec![Effect::Redraw]
            }
            Action::OmnibarInsert(s) => {
                self.omnibar.insert_str(&s);
                self.omnibar.selected = 0;
                recompute_suggestions(&mut self.omnibar, &self.canvas);
                vec![Effect::Redraw]
            }
            Action::OmnibarBackspace => {
                if self.omnibar.backspace() {
                    self.omnibar.selected = 0;
                    recompute_suggestions(&mut self.omnibar, &self.canvas);
                }
                vec![Effect::Redraw]
            }
            Action::OmnibarDelete => {
                if self.omnibar.delete_forward() {
                    self.omnibar.selected = 0;
                    recompute_suggestions(&mut self.omnibar, &self.canvas);
                }
                vec![Effect::Redraw]
            }
            Action::OmnibarCaret(m) => {
                // Caret motion never changes the text, so the suggestion
                // list (and the highlight) stays put.
                self.omnibar.move_caret(m);
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
    }

    #[cfg(test)]
    pub(crate) fn test_stub() -> Self {
        Self {
            canvas: Canvas::new(),
            omnibar: OmnibarState::default(),
            data_root: std::env::temp_dir().join("merecat-app-test"),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Committing a `>` registry row lowers the registry Action through the
    /// same spine as everything else, and the palette closes.
    #[test]
    fn committing_an_action_row_runs_the_action_and_closes() {
        let mut app = App::test_stub();
        for action in [
            Action::OmnibarOpen { command: true },
            Action::OmnibarChar('i'),
            Action::OmnibarChar('s'),
            Action::OmnibarChar('o'),
        ] {
            app.update(action);
        }
        assert!(!app.canvas.is_isometric());
        let effects = app.update(Action::OmnibarCommit);
        assert!(app.canvas.is_isometric(), "the committed toggle ran");
        assert!(!app.omnibar.open, "the palette closed on commit");
        assert!(effects.contains(&Effect::Redraw));
    }

    /// Committing a find-lane node row selects without fetching.
    #[test]
    fn committing_a_node_row_selects_without_fetch_effects() {
        let mut app = App::test_stub();
        app.canvas.visit("https://example.com/meerkats");
        app.update(Action::OmnibarOpen { command: false });
        for c in "meer".chars() {
            app.update(Action::OmnibarChar(c));
        }
        let effects = app.update(Action::OmnibarCommit);
        assert!(
            !effects.iter().any(|e| matches!(e, Effect::FetchPage(_))),
            "selecting an existing node must not refetch: {effects:?}"
        );
        assert!(!app.omnibar.open);
    }
}
