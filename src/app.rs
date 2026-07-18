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
        PaneKind::Workbench => PaneContent::Workbench,
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
    /// The visit-history cursor (the r3-owed nav row): every opened address
    /// records here; Back/Forward move the cursor and re-select without
    /// refetching. chrome's `History` — the mere vocabulary, direct-dep'd.
    pub history: chrome::nav::History,
    /// The active pane — the anchor a summon splits from and a close removes.
    /// `None` means the canvas (the Orrery leaf).
    pub active_pane: Option<PaneId>,
    /// The node-tiling model INSIDE the Workbench pane leaf (rung 5 slice E):
    /// platen's `Workbench` — the split tree of tab-stacks, the active tab per
    /// stack, every mutator. App truth (data, no handles); persisted as the
    /// canonical `(Arrangement, geometry)` pair beside `graph.json`.
    pub workbench: mere::platen::Workbench,
    /// The browser-state sidecar (rung 6): per-node browser handling (viewer
    /// override, compat mode, content-on), persisted at `browser_nodes.json`.
    /// The graph stays correct without it (the sidecar's charter).
    pub browser: session_runtime::browser_node_state::BrowserNodeStates,
    /// A maximized pane takes the whole pane area (a host view state; frisket
    /// has no maximize op). Not persisted; resets on restart.
    pub maximized: Option<PaneId>,
    /// How many windows are open (rung 7). A MIRROR like `roster_tab`: the
    /// shell owns the platform windows and copies the count here so
    /// observation (and a scenario) can see it.
    pub window_count: usize,
    /// Each lens window's pane space (rung 7 depth: windows are pane HOSTS,
    /// not canvas-only): a frisket tree over the one App, indexed by the lens
    /// ordinal the shell's window records carry. `None` = that lens closed
    /// (tombstoned so ordinals stay stable). The primary window's space stays
    /// `frisket` above. Not persisted yet — window records at restart are the
    /// rung's remaining depth.
    pub lenses: Vec<Option<FrisketLayout>>,
    /// Which Roster tab is showing. A MIRROR, not the truth: cambium's tab strip
    /// owns its selection (the widget's state, in the shell's runner), and the
    /// shell copies it here after each dispatch so observation can see it — the
    /// inverse of `content`, where the app holds the data and the shell holds the
    /// live handle. Not persisted yet; restoring a pane's tab wants this on the
    /// frisket leaf rather than on App, once a second pane grows tabs.
    pub roster_tab: usize,
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
        // Restore the workbench tiling, pruned to the live graph's members (a
        // tile whose node vanished between sessions collapses away).
        let present = canvas.graph().nodes().map(|(_, n)| n.id).collect();
        let workbench = session::load_workbench(&data_root, &present);
        // The history seeds from wherever the session opens (the focused
        // node's url, or an empty sentinel Back can never step past).
        let history = chrome::nav::History::new(
            canvas.focused_url().map(str::to_string).unwrap_or_default(),
        );
        // Restore WHERE the user was (rung 6): a restored session boots with
        // nothing selected, which silently hid restored live content (the
        // inset composes for the FOCUSED node). Graph truth already records
        // visits, so re-select the most recently visited node still present.
        if canvas.focused_member().is_none()
            && let Some(last) = canvas.graph().recent_visited(1).into_iter().next()
        {
            canvas.select_by_url(&last.url);
        }
        // The browser-state sidecar, and the content-state restore (rung 6):
        // every restored node whose content was ON respawns its session
        // through the ordinary port, so `Live` after a restart is the same
        // spawned truth as before it — never a painted memory.
        let browser = session::load_browser_nodes(&data_root);
        let mut content = ContentStates::default();
        for (_, node) in canvas.graph().nodes() {
            if browser.get(node.id).is_some_and(|b| b.content_on) {
                content.note_requested(node.id);
                effects.push(Effect::SpawnContent {
                    node: node.id,
                    url: node.url().to_string(),
                });
            }
        }
        (
            Self {
                canvas,
                omnibar,
                data_root,
                content,
                focus,
                frisket,
                history,
                active_pane: None,
                workbench,
                browser,
                maximized: None,
                window_count: 1,
                lenses: Vec::new(),
                roster_tab: 0,
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

    /// Refresh the browser-state sidecar from live truth before a save
    /// (rung 6): each graph node's `content_on` mirrors its content
    /// lifecycle (live or in flight), and entries for vanished nodes drop.
    pub fn refresh_browser_states(&mut self) {
        use crate::content::NodeContent;
        let present: std::collections::HashSet<uuid::Uuid> =
            self.canvas.graph().nodes().map(|(_, n)| n.id).collect();
        let stale: Vec<uuid::Uuid> = self
            .browser
            .nodes
            .keys()
            .copied()
            .filter(|id| !present.contains(id))
            .collect();
        for id in stale {
            self.browser.remove(id);
        }
        for id in present {
            let on = matches!(
                self.content.get(id),
                Some(NodeContent::Live | NodeContent::Requested)
            );
            if on || self.browser.get(id).is_some() {
                self.browser.entry(id).content_on = on;
            }
        }
    }

    /// Seed a new lens window's pane space: a lone Orrery leaf with a freshly
    /// minted pane id (globally unique across every window's tree, so surface
    /// keys and the active-pane anchor never collide). Returns its ordinal.
    fn seed_lens_space(&mut self) -> usize {
        let pane_id = PaneId(self.next_pane_id);
        self.next_pane_id += 1;
        let ordinal = self.lenses.len();
        self.lenses.push(Some(FrisketLayout {
            id: frisket::FrisketId::new(format!("lens-{ordinal}")),
            label: format!("lens {ordinal}"),
            root: PaneNode::Leaf {
                pane_id,
                content: PaneContent::Orrery,
                graph_id: GraphId::nil(),
            },
        }));
        ordinal
    }

    /// Note a semantic event from outside `update` — the shell's own divergence
    /// (an interaction that missed, an affordance not yet wired) joins the same
    /// drained stream the update path feeds, so automation reads one channel.
    pub fn note(&mut self, event: AppEvent) {
        self.events.push(event);
    }

    /// Consume one app intent. Never blocks; anything slow leaves as an effect.
    pub fn update(&mut self, action: Action) -> Vec<Effect> {
        match action {
            Action::OpenAddress(url) => {
                self.events.push(AppEvent::AddressOpened(url.clone()));
                let key = self.canvas.visit(&url);
                self.history.visit(url.clone());
                let mut effects = vec![Effect::Redraw];
                if fetch::is_fetchable(&url)
                    && let Some(node) = self.canvas.graph().get_node(key).map(|n| n.id)
                {
                    effects.push(Effect::FetchPage { node, url });
                }
                effects
            }
            // The nav pair: move the history cursor and RE-SELECT (never a
            // refetch — the find lane's discipline). A remembered address
            // whose node was deleted re-mints it via visit, without touching
            // the cursor again.
            Action::NavBack => {
                let Some(url) = self.history.back().map(str::to_string) else {
                    return vec![Effect::Redraw];
                };
                self.events.push(AppEvent::NavigatedBack(url.clone()));
                if !url.is_empty() && !self.canvas.select_by_url(&url) {
                    self.canvas.visit(&url);
                }
                vec![Effect::Redraw]
            }
            Action::NavForward => {
                let Some(url) = self.history.forward().map(str::to_string) else {
                    return vec![Effect::Redraw];
                };
                self.events.push(AppEvent::NavigatedForward(url.clone()));
                if !self.canvas.select_by_url(&url) {
                    self.canvas.visit(&url);
                }
                vec![Effect::Redraw]
            }
            Action::Reload => {
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
                    Some(crate::content::NodeContent::Live | crate::content::NodeContent::Requested)
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
            Action::NewWindow => {
                let ordinal = self.seed_lens_space();
                self.events.push(AppEvent::WindowOpened);
                vec![Effect::OpenWindow { ordinal }, Effect::Redraw]
            }
            // The tear-out trichotomy's LEAF arm: the active pane's frisket
            // leaf leaves this window's tree and joins the newest lens's
            // (spawning one when none is open). The pane's retained runner is
            // untouched — in the surface-compositor shape, identity across
            // windows is a property of the RUNNER staying put while the leaf
            // changes trees, which is exactly what the forest dom exists to
            // buy the one-shared-DOM shape.
            Action::TearOutActivePane => {
                let Some(active) = self.active_pane else {
                    return vec![Effect::Redraw];
                };
                // Read the leaf wholesale (id + content + graph binding), then
                // remove it from the primary tree.
                let Some((pane_id, content, graph_id)) = self
                    .frisket
                    .iter_leaves()
                    .find(|(id, _, _)| *id == active)
                    .map(|(id, c, g)| (id, c.clone(), g))
                else {
                    return vec![Effect::Redraw];
                };
                let Some(path) = crate::pane::path_of(&self.frisket, active) else {
                    return vec![Effect::Redraw];
                };
                if !self.frisket.close_leaf(&path) {
                    return vec![Effect::Redraw];
                }
                if self.maximized == Some(active) {
                    self.maximized = None;
                }
                self.active_pane = None;
                let mut effects = Vec::new();
                // Land in the newest live lens, else spawn one for the pane.
                let target = self.lenses.iter().rposition(Option::is_some);
                let ordinal = match target {
                    Some(ordinal) => ordinal,
                    None => {
                        let ordinal = self.seed_lens_space();
                        self.events.push(AppEvent::WindowOpened);
                        effects.push(Effect::OpenWindow { ordinal });
                        ordinal
                    }
                };
                if let Some(Some(lens)) = self.lenses.get_mut(ordinal) {
                    // Anchor on the lens tree's LAST leaf (a summon needs a
                    // leaf path; the root path only names a leaf while the
                    // tree is a lone Orrery).
                    let anchor_path = lens
                        .iter_leaves()
                        .last()
                        .map(|(id, _, _)| id)
                        .and_then(|id| crate::pane::path_of(lens, id))
                        .unwrap_or_default();
                    lens.summon_leaf(
                        &anchor_path,
                        InsertSide::Right,
                        PaneNode::Leaf {
                            pane_id,
                            content: content.clone(),
                            graph_id,
                        },
                    );
                }
                self.events.push(AppEvent::PaneTornOut(content.tag().to_string()));
                effects.push(Effect::Redraw);
                effects
            }
            Action::SetViewerOverride { member, viewer } => {
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
                    Some(crate::content::NodeContent::Live | crate::content::NodeContent::Requested)
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
            Action::SetNodeSprite { member, data_uri, hull } => {
                self.canvas.set_node_sprite(member, data_uri);
                // The traced collider: the node collides at its picture. Under
                // 3 points the tracer found no opaque region — keep the
                // silhouette collider rather than installing a degenerate one.
                if hull.len() >= 3 {
                    self.canvas.set_node_sprite_hull(member, hull);
                }
                self.events.push(AppEvent::NodeSpriteSet(member));
                vec![Effect::SaveSession, Effect::Redraw]
            }
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
            Action::SetSplitRatio { path, ratio } => {
                self.frisket.set_split_ratio(&path, ratio);
                vec![Effect::Redraw]
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
            // Workbench ops (rung 5 slice E). Platen owns the model and every
            // mutator; these arms lower intents onto it and persist. The
            // Workbench PANE (the frisket leaf) is where the tiling shows;
            // opening a tile summons it if absent, through the same summon
            // path as a palette summon (one spine, no side door).
            Action::OpenInWorkbench => {
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
            Action::WorkbenchActivate(member) => {
                if self.workbench.activate(member) {
                    vec![Effect::SaveSession, Effect::Redraw]
                } else {
                    vec![Effect::Redraw]
                }
            }
            Action::CloseWorkbenchTile => {
                let Some(member) = self.canvas.focused_member() else {
                    return vec![Effect::Redraw];
                };
                if self.workbench.close_tile(member) {
                    self.events.push(AppEvent::WorkbenchTileClosed);
                    vec![Effect::SaveSession, Effect::Redraw]
                } else {
                    vec![Effect::Redraw]
                }
            }
            Action::WorkbenchStackOnto { dragged, target } => {
                if self.workbench.move_to_slot_of(dragged, target) {
                    self.events.push(AppEvent::WorkbenchStacked);
                    vec![Effect::SaveSession, Effect::Redraw]
                } else {
                    vec![Effect::Redraw]
                }
            }
            Action::WorkbenchSplitBeside {
                dragged,
                target,
                axis,
                after,
            } => {
                // The app vocabulary's axis maps onto pelt's at the platen
                // call (the one place the tile contract is named).
                let axis = match axis {
                    crate::action::WbAxis::Row => pelt_core::tile::SplitAxis::Row,
                    crate::action::WbAxis::Column => pelt_core::tile::SplitAxis::Column,
                };
                if self.workbench.split_beside_axis(dragged, target, axis, after) {
                    self.events.push(AppEvent::WorkbenchSplit);
                    vec![Effect::SaveSession, Effect::Redraw]
                } else {
                    vec![Effect::Redraw]
                }
            }
            Action::WorkbenchSplitOut { dragged, axis, after } => {
                let axis = match axis {
                    crate::action::WbAxis::Row => pelt_core::tile::SplitAxis::Row,
                    crate::action::WbAxis::Column => pelt_core::tile::SplitAxis::Column,
                };
                if self.workbench.split_out(dragged, axis, after) {
                    self.events.push(AppEvent::WorkbenchSplit);
                    vec![Effect::SaveSession, Effect::Redraw]
                } else {
                    vec![Effect::Redraw]
                }
            }
            Action::WorkbenchSetFractions { path, fractions } => {
                self.workbench.set_split_fractions(&path, &fractions);
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
            history: chrome::nav::History::new(""),
            active_pane: None,
            workbench: mere::platen::Workbench::new(),
            browser: session_runtime::browser_node_state::BrowserNodeStates::new(),
            maximized: None,
            window_count: 1,
            lenses: Vec::new(),
            roster_tab: 0,
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

    /// The tear-out leaf arm (rung 7 depth): the active pane's leaf leaves
    /// the primary tree and joins a lens space — SAME pane id (the retained
    /// runner never moves; identity is structural). No lens open spawns one.
    #[test]
    fn tear_out_moves_the_leaf_and_keeps_its_id() {
        let mut app = App::test_stub();
        app.update(Action::SummonPane(PaneKind::Roster));
        let roster_id = app
            .frisket
            .iter_leaves()
            .find(|(_, c, _)| matches!(c, PaneContent::Roster))
            .map(|(id, _, _)| id)
            .expect("summoned");
        let effects = app.update(Action::TearOutActivePane);
        // Departure: the primary tree no longer holds a Roster leaf.
        assert!(
            !app.frisket
                .iter_leaves()
                .any(|(_, c, _)| matches!(c, PaneContent::Roster)),
            "the roster left the primary tree"
        );
        // Arrival: a lens space spawned (no lens was open) and holds the SAME
        // pane id — the leaf moved, nothing was recreated.
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::OpenWindow { .. })),
            "tearing out with no lens spawns one: {effects:?}"
        );
        let lens = app.lenses[0].as_ref().expect("lens space seeded");
        let moved = lens
            .iter_leaves()
            .find(|(_, c, _)| matches!(c, PaneContent::Roster))
            .expect("the roster landed in the lens");
        assert_eq!(moved.0, roster_id, "same pane id across the move");
        // A second tear-out lands in the SAME lens (no window spam).
        app.update(Action::SummonPane(PaneKind::Trail));
        let effects = app.update(Action::TearOutActivePane);
        assert!(
            !effects
                .iter()
                .any(|e| matches!(e, Effect::OpenWindow { .. })),
            "an open lens is reused"
        );
        let lens = app.lenses[0].as_ref().unwrap();
        assert!(
            lens.iter_leaves()
                .any(|(_, c, _)| matches!(c, PaneContent::Trail)),
            "the trail joined the existing lens"
        );
    }

    /// The nav row (r3 owed): Back re-selects without refetching, Forward
    /// redoes, a new open truncates the forward branch, and Reload refetches
    /// the focused node and respawns its live content.
    #[test]
    fn back_forward_and_reload_flow_through_the_spine() {
        let mut app = App::test_stub();
        app.update(Action::OpenAddress("https://example.com/a".to_string()));
        app.update(Action::OpenAddress("https://example.com/b".to_string()));
        // Back: the previous node re-selects, with NO fetch effect.
        let effects = app.update(Action::NavBack);
        assert!(
            !effects.iter().any(|e| matches!(e, Effect::FetchPage { .. })),
            "Back never refetches: {effects:?}"
        );
        assert_eq!(app.canvas.focused_url(), Some("https://example.com/a"));
        // Forward redoes.
        app.update(Action::NavForward);
        assert_eq!(app.canvas.focused_url(), Some("https://example.com/b"));
        // Back then a new open: the forward branch truncates.
        app.update(Action::NavBack);
        app.update(Action::OpenAddress("https://example.com/c".to_string()));
        assert!(!app.history.can_forward(), "a new open truncates forward");
        assert!(app.history.can_back());
        // Reload: a fetch effect for the focused node; with live content, a
        // close + respawn pair.
        let node = app.canvas.focused_member().unwrap();
        app.apply_update(Update::ContentSpawned { node, facts: None });
        let effects = app.update(Action::Reload);
        assert!(effects.iter().any(|e| matches!(e, Effect::FetchPage { .. })));
        assert!(effects.iter().any(|e| matches!(e, Effect::CloseContent { .. })));
        assert!(effects.iter().any(|e| matches!(e, Effect::SpawnContent { .. })));
        let described: Vec<String> = app.take_events().iter().map(
            crate::observe::AppEvent::describe,
        ).collect();
        assert!(described.iter().any(|e| e.starts_with("nav-back ")));
        assert!(described.iter().any(|e| e.starts_with("nav-forward ")));
        assert!(described.iter().any(|e| e.starts_with("reloaded ")));
    }

    /// The workbench lane end to end at the App tier: opening the focused
    /// node tiles it, summons the Workbench pane, and spawns its content;
    /// stacking collapses cells; closing empties honestly.
    #[test]
    fn workbench_actions_flow_through_the_spine() {
        let mut app = App::test_stub();
        app.update(Action::OpenAddress("mere://alpha".to_string()));
        let a = app.canvas.focused_member().unwrap();
        let effects = app.update(Action::OpenInWorkbench);
        assert!(app.workbench.is_tiled());
        assert_eq!(app.workbench.tile_count(), 1);
        assert!(
            app.frisket
                .iter_leaves()
                .any(|(_, c, _)| matches!(c, PaneContent::Workbench)),
            "opening a tile summons the Workbench pane"
        );
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::SpawnContent { .. })),
            "a tile wants live content: {effects:?}"
        );
        // Re-opening the same node adds nothing.
        app.update(Action::OpenInWorkbench);
        assert_eq!(app.workbench.tile_count(), 1);
        // A second node tiles beside it; stacking collapses to one cell.
        app.update(Action::OpenAddress("mere://beta".to_string()));
        let b = app.canvas.focused_member().unwrap();
        app.update(Action::OpenInWorkbench);
        assert_eq!(app.workbench.slot_count(), 2);
        app.update(Action::WorkbenchStackOnto { dragged: b, target: a });
        assert_eq!(app.workbench.slot_count(), 1);
        assert_eq!(app.workbench.tile_count(), 2);
        // Activate the buried tab; close the focused (beta) tile.
        app.update(Action::WorkbenchActivate(a));
        app.update(Action::CloseWorkbenchTile);
        assert_eq!(app.workbench.tile_count(), 1);
        assert!(app.workbench.has_tile(a));
    }

    /// The browser-state sidecar (rung 6): content-on mirrors live truth at
    /// refresh, prunes vanished nodes, and round-trips through the store.
    #[test]
    fn browser_states_refresh_and_round_trip() {
        let mut app = App::test_stub();
        app.update(Action::OpenAddress("https://example.com/a".to_string()));
        let a = app.canvas.focused_member().unwrap();
        app.apply_update(Update::ContentSpawned { node: a, facts: None });
        app.update(Action::OpenAddress("https://example.com/b".to_string()));
        app.refresh_browser_states();
        assert!(app.browser.get(a).is_some_and(|b| b.content_on));
        assert!(
            app.browser.get(app.canvas.focused_member().unwrap()).is_none(),
            "a node without content stays out of the sidecar"
        );
        // Round trip through the store.
        let dir = std::env::temp_dir().join(format!("merecat-bn-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        crate::session::save_browser_nodes(&dir, &app.browser);
        let restored = crate::session::load_browser_nodes(&dir);
        assert!(restored.get(a).is_some_and(|b| b.content_on));
        // Content off -> the refresh clears the flag.
        app.content.note_closed(a);
        app.refresh_browser_states();
        assert!(
            !app.browser.get(a).is_some_and(|b| b.content_on),
            "closed content clears the flag on the next refresh"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The workbench sidecar round-trips through the persistence port,
    /// pruned to present members (platen's canonical pair underneath).
    #[test]
    fn workbench_persists_and_restores_pruned() {
        let dir = std::env::temp_dir().join(format!("merecat-wb-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let (a, b) = (uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
        let mut wb = mere::platen::Workbench::new();
        wb.ensure_tiled();
        wb.open_tile(a);
        wb.open_tile(b);
        crate::session::save_workbench(&dir, &wb);
        // Both present: both tiles come back.
        let present: std::collections::HashSet<_> = [a, b].into_iter().collect();
        let restored = crate::session::load_workbench(&dir, &present);
        assert_eq!(restored.tile_count(), 2);
        // b's node vanished between sessions: its tile is reconciled away.
        let present: std::collections::HashSet<_> = [a].into_iter().collect();
        let restored = crate::session::load_workbench(&dir, &present);
        assert_eq!(restored.tile_count(), 1);
        assert!(restored.has_tile(a));
        let _ = std::fs::remove_dir_all(&dir);
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
