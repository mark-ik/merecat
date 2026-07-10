//! Merecat's core state and the two update functions — the middle of the
//! spine: `Action -> update -> Effects` and `Update -> apply_update ->
//! Effects`. Holds data, never handles: the ports (actors, stores, the
//! window) live in the shell, which runs the effects this module returns.

use std::path::PathBuf;

use mere::canvas::Canvas;

use crate::action::{Action, Effect, Update};
use crate::{session, web};

/// The application state: the hosted canvas (which owns the graph) and where
/// the session persists.
pub struct App {
    pub canvas: Canvas,
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
        let mut canvas = match (restored, address) {
            (Some(graph), _) => Canvas::with_graph(graph),
            (None, Some(url)) => {
                tracing::info!(%url, "fresh graph seeded from the address");
                Canvas::new()
            }
            (None, None) => {
                tracing::info!("no session graph; starting on the sample graph");
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
        (Self { canvas, data_root }, effects)
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
        }
    }

    /// Fold one typed service answer into state.
    pub fn apply_update(&mut self, update: Update) -> Vec<Effect> {
        match update {
            Update::Fetch(fetch::FetchUpdate::Page(outcome)) => {
                web::apply_page_outcome(&mut self.canvas, outcome)
            }
            Update::Fetch(fetch::FetchUpdate::Favicon { owner_url, bytes }) => {
                web::apply_favicon(&mut self.canvas, &owner_url, &bytes)
            }
            // Subresources arrive with the content lane (obviation rung 4).
            Update::Fetch(fetch::FetchUpdate::Subresource(_)) => Vec::new(),
        }
    }
}
