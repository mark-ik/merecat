//! Merecat's core state and the two update functions — the middle of the
//! spine: `Action -> update -> Effects` and `Update -> apply_update ->
//! Effects`. Holds data, never handles: the ports (actors, stores, the
//! window) live in the shell, which runs the effects this module returns.

use std::path::PathBuf;

use mere::canvas::Canvas;

use frisket::{FrisketLayout, GraphId, InsertSide, PaneContent, PaneId, PaneNode};

use crate::action::{Action, Effect, PaneKind, Update};
use crate::content::ContentStates;
use crate::observe::AppEvent;
use crate::surface::FocusTarget;
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

/// The `frisket::PaneContent` a summonable `PaneKind` maps to. The mapping
/// lives here (not in `action`) so the vocabulary module stays free of the
/// pane-model crate. Slice C summons these as placeholders; slice D gives each
/// its real content.
fn pane_content(kind: PaneKind) -> PaneContent {
    match kind {
        PaneKind::Roster => PaneContent::Roster,
        PaneKind::Trail => PaneContent::Trail,
        PaneKind::Gloss => PaneContent::Gloss,
        PaneKind::Inspector => PaneContent::Inspector,
        PaneKind::Steward => PaneContent::Steward,
        PaneKind::Comms => PaneContent::Comms,
        PaneKind::Apparatus => PaneContent::Apparatus,
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
    /// Per-node content lifecycle (rung 4). Data only: the live session
    /// handles live in the shell's content port, keyed by the same ids.
    pub content: ContentStates,
    /// Which surface receives semantic input (rung 5 slice A). The explicit
    /// replacement for the old `omnibar.open` routing boolean: a third surface
    /// class (panes) joins by adding a `FocusTarget` variant rather than
    /// threading another bool through the shell. `omnibar.open` stays the
    /// omnibar's own display state; opening/closing it keeps this in sync.
    pub focus: FocusTarget,
    /// The pane tree (rung 5 slice C): frisket's split tree of `PaneContent`
    /// leaves. The Orrery leaf is the graph canvas; summoning a pane splits it.
    /// Persisted to `frame.json` through the session port.
    pub frisket: FrisketLayout,
    /// The active pane — the anchor a summon splits from and a close removes.
    /// `None` means the canvas (the Orrery leaf).
    pub active_pane: Option<PaneId>,
    /// A maximized pane takes the whole pane area (a host view state; frisket
    /// has no maximize op). Not persisted; resets on restart.
    pub maximized: Option<PaneId>,
    /// Next pane id to mint. Kept above every id in the layout so a summon after
    /// a restore never collides with a persisted pane.
    next_pane_id: u64,
    /// Semantic events since the last drain (the observation pair's stream
    /// half; the shell drains each frame). Data, like everything else here.
    events: Vec<AppEvent>,
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
            let key = canvas.visit(url);
            if fetch::is_fetchable(url)
                && let Some(node) = canvas.graph().get_node(key).map(|n| n.id)
            {
                effects.push(Effect::FetchPage {
                    node,
                    url: url.to_string(),
                });
            }
        }
        // A bare FIRST launch opens the omnibar by itself, so the app is
        // discoverable without documentation; a bare relaunch restores the
        // canvas quietly (Ctrl+L / Ctrl+K summon).
        let mut omnibar = OmnibarState::default();
        let mut focus = FocusTarget::Canvas;
        if first_run {
            omnibar.open = true;
            focus = FocusTarget::Chrome;
            recompute_suggestions(&mut omnibar, &canvas);
        }
        // Restore the pane layout (rung 5 slice C), else start on the default
        // single-pane (Orrery) tree. `next_pane_id` clears every restored id so a
        // later summon cannot collide with a persisted pane.
        let frisket = session::load_frisket_layout(&data_root).unwrap_or_default();
        let next_pane_id = frisket.iter_leaves().map(|(id, _, _)| id.0).max().unwrap_or(0) + 1;
        (
            Self {
                canvas,
                omnibar,
                data_root,
                content: ContentStates::default(),
                focus,
                frisket,
                active_pane: None,
                maximized: None,
                next_pane_id,
                events: Vec::new(),
            },
            effects,
        )
    }

    /// Drain the semantic events emitted since the last call (the shell
    /// hands them to the scenario's log, diagnostics, or drops them).
    pub fn take_events(&mut self) -> Vec<AppEvent> {
        std::mem::take(&mut self.events)
    }

    /// Consume one app intent. Never blocks; anything slow leaves as an effect.
    pub fn update(&mut self, action: Action) -> Vec<Effect> {
        match action {
            Action::OpenAddress(url) => {
                self.events.push(AppEvent::AddressOpened(url.clone()));
                let key = self.canvas.visit(&url);
                let mut effects = vec![Effect::Redraw];
                if fetch::is_fetchable(&url)
                    && let Some(node) = self.canvas.graph().get_node(key).map(|n| n.id)
                {
                    effects.push(Effect::FetchPage { node, url });
                }
                effects
            }
            Action::ReseedLayout => {
                if self.canvas.reseed() {
                    self.events.push(AppEvent::LayoutReseeded);
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
            Action::ToggleNodeContent => {
                // The flip targets the focused node; no focus, no-op (the
                // caption chip tells the user what would flip).
                // Resolve the node by MEMBER, not by URL round-trip: two
                // nodes may share a URL (the sample graph + an open), and
                // get_node_by_url picks arbitrarily between them.
                let Some(target) = self.canvas.focused_member().zip(
                    self.canvas.focused_url().map(str::to_string),
                ) else {
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
            Action::OmnibarOpen { command } => {
                self.omnibar = OmnibarState {
                    open: true,
                    text: if command { ">".to_string() } else { String::new() },
                    ..OmnibarState::default()
                };
                self.omnibar.cursor = self.omnibar.text.len();
                self.focus = FocusTarget::Chrome;
                recompute_suggestions(&mut self.omnibar, &self.canvas);
                self.events.push(AppEvent::OmnibarOpened);
                vec![Effect::Redraw]
            }
            Action::OmnibarClose => {
                self.omnibar = OmnibarState::default();
                // Chrome relinquishes focus back to the canvas. Content focus
                // is slice B (content takes input); slice A only distinguishes
                // canvas from chrome.
                if self.focus == FocusTarget::Chrome {
                    self.focus = FocusTarget::Canvas;
                }
                self.events.push(AppEvent::OmnibarClosed);
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
                // Commit always ends with the omnibar closed, so chrome hands
                // focus back to the canvas. (A committed OpenAddress may later
                // spawn content; routing focus onto it is slice B.)
                if self.focus == FocusTarget::Chrome {
                    self.focus = FocusTarget::Canvas;
                }
                let committed = self.omnibar.selection().cloned().or_else(|| {
                    normalize_address(self.omnibar.text.trim())
                        .map(|url| Suggestion::Go { url })
                });
                if let Some(s) = committed.as_ref() {
                    self.events.push(AppEvent::OmnibarCommitted(
                        crate::observe::suggestion_line(s),
                    ));
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
            // Pane tree ops (rung 5 slice C). Each mutates the frisket layout and
            // persists it (SaveSession writes frame.json), so the arrangement
            // survives a restart. Maximize is view state, not persisted.
            Action::SummonPane(kind) => {
                let content = pane_content(kind);
                let id = PaneId(self.next_pane_id);
                // Anchor on the active pane, else the Orrery (graph) leaf —
                // meerkat's fixed Right-split off the graph pane, generalized.
                let anchor = self.active_pane.or_else(|| {
                    self.frisket
                        .iter_leaves()
                        .find(|(_, c, _)| matches!(c, PaneContent::Orrery))
                        .map(|(id, _, _)| id)
                });
                let anchor_path = anchor
                    .and_then(|a| crate::pane::path_of(&self.frisket, a))
                    .unwrap_or_default();
                let new_leaf = PaneNode::Leaf {
                    pane_id: id,
                    content,
                    graph_id: GraphId::nil(),
                };
                if self.frisket.summon_leaf(&anchor_path, InsertSide::Right, new_leaf) {
                    self.next_pane_id += 1;
                    self.active_pane = Some(id);
                    self.events.push(AppEvent::PaneSummoned(kind.label()));
                    vec![Effect::SaveSession, Effect::Redraw]
                } else {
                    vec![Effect::Redraw]
                }
            }
            Action::CloseActivePane => {
                // The canvas (no active pane) has nothing to close.
                let Some(active) = self.active_pane else {
                    return vec![Effect::Redraw];
                };
                let Some(path) = crate::pane::path_of(&self.frisket, active) else {
                    return vec![Effect::Redraw];
                };
                if self.frisket.close_leaf(&path) {
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
            Action::SetActivePaneDivider(ratio) => {
                let Some(active) = self.active_pane else {
                    return vec![Effect::Redraw];
                };
                let Some(mut path) = crate::pane::path_of(&self.frisket, active) else {
                    return vec![Effect::Redraw];
                };
                // The active leaf's parent split holds the divider.
                path.pop();
                if self.frisket.set_split_ratio(&path, ratio) {
                    vec![Effect::SaveSession, Effect::Redraw]
                } else {
                    vec![Effect::Redraw]
                }
            }
            Action::ToggleMaximizePane => {
                if let Some(active) = self.active_pane {
                    self.maximized = (self.maximized != Some(active)).then_some(active);
                }
                vec![Effect::Redraw]
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn test_stub() -> Self {
        Self {
            canvas: Canvas::new(),
            omnibar: OmnibarState::default(),
            data_root: std::env::temp_dir().join("merecat-app-test"),
            content: ContentStates::default(),
            focus: FocusTarget::Canvas,
            frisket: FrisketLayout::default(),
            active_pane: None,
            maximized: None,
            next_pane_id: 1,
            events: Vec::new(),
        }
    }

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
            Update::ContentSpawned { node } => {
                self.content.note_live(node);
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

    /// The content flip lowers through the spine: focused node -> Requested +
    /// SpawnContent; the port's honest failure folds back; a failed node
    /// retries on the next flip.
    #[test]
    fn content_flip_lowers_and_fails_honestly() {
        use crate::content::NodeContent;
        let mut app = App::test_stub();
        assert!(
            app.update(Action::ToggleNodeContent).is_empty(),
            "no focus, no-op"
        );
        app.canvas.visit("https://example.com/page");
        let effects = app.update(Action::ToggleNodeContent);
        let Some(Effect::SpawnContent { node, url }) = effects
            .iter()
            .find(|e| matches!(e, Effect::SpawnContent { .. }))
            .cloned()
        else {
            panic!("flip on a focused node spawns: {effects:?}");
        };
        assert_eq!(url, "https://example.com/page");
        assert_eq!(app.content.get(node), Some(&NodeContent::Requested));
        assert!(
            !app.update(Action::ToggleNodeContent)
                .iter()
                .any(|e| matches!(e, Effect::SpawnContent { .. })),
            "flipping an in-flight node closes, never double-spawns"
        );
        app.content.note_requested(node);
        app.apply_update(Update::ContentFailed {
            node,
            error: "port not wired".into(),
        });
        assert!(
            matches!(app.content.get(node), Some(NodeContent::Failed(_))),
            "failure is a surfaced state"
        );
        assert!(
            app.update(Action::ToggleNodeContent)
                .iter()
                .any(|e| matches!(e, Effect::SpawnContent { .. })),
            "a failed node retries on the next flip"
        );
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
            !effects.iter().any(|e| matches!(e, Effect::FetchPage { .. })),
            "selecting an existing node must not refetch: {effects:?}"
        );
        assert!(!app.omnibar.open);
    }
}
